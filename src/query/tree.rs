use std::collections::BTreeMap;

use crate::output::{
    Continuation, ContinuationTool, TreeContinuation, TreeData, TreeNode, TreeOutput,
};
use serde_json::json;

use super::{
    ApiError, Profile, RenderState, TreeTruncation, check_budget, envelope, percent,
    tree_reason_values,
};

pub fn tree<'p>(
    profile: &'p Profile,
    root_node_id: [u32; 2],
    expected_fingerprint: Option<&str>,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
) -> Result<TreeOutput<'p>, ApiError> {
    check_budget(max_depth, max_nodes, min_scope_percent)?;
    if root_node_id != [profile.cct.root, 0] {
        expected_fingerprint.ok_or_else(|| {
            ApiError::new(
                "profile_changed",
                "Tree continuation requires a profile fingerprint",
                json!({"cursor_node_id":root_node_id}),
                "Copy a returned continuation unchanged, or restart without continuation.",
            )
        })?;
    }
    if let Some(expected) = expected_fingerprint
        && expected != profile.source.fingerprint
    {
        return Err(ApiError::new(
            "profile_changed",
            "Profile fingerprint no longer matches this tree continuation",
            json!({"expected_fingerprint":expected,"current_fingerprint":profile.source.fingerprint}),
            "Restart the query without continuation.",
        ));
    }
    let root = profile
        .cct
        .nodes
        .get(root_node_id[0] as usize)
        .ok_or_else(|| {
            ApiError::new(
                "invalid_node_id",
                format!("Unknown tree continuation cursor node: {root_node_id:?}"),
                json!({"cursor_node_id":root_node_id}),
                "Restart without continuation, or copy a returned continuation unchanged.",
            )
        })?;
    if (root_node_id[0] == profile.cct.root && root_node_id[1] != 0)
        || (root_node_id[0] != profile.cct.root && root_node_id[1] >= root.label_len)
    {
        return Err(ApiError::new(
            "invalid_node_id",
            "Tree continuation label offset is out of range",
            json!({"cursor_node_id":root_node_id}),
            "Restart without continuation, or copy a returned continuation unchanged.",
        ));
    }
    let mut budget = max_nodes;
    let mut reason_stats = BTreeMap::new();
    let mut continuations = Vec::new();
    let mut continuation_count = 0usize;
    let mut render = RenderState::new(
        root.total_weight,
        max_depth,
        min_scope_percent,
        &mut budget,
        &mut reason_stats,
        &mut continuations,
        &mut continuation_count,
    );
    let (node, _) = render_cct(profile, root_node_id, 0, &mut render)?;
    let continuations_truncated = continuation_count > continuations.len();
    let truncation_reasons =
        tree_reason_values(&reason_stats, max_depth, max_nodes, min_scope_percent);
    Ok(envelope(
        profile,
        root.total_weight,
        truncation_reasons,
        Vec::new(),
        TreeData {
            root: node,
            continuations_truncated,
            continuation_limit: 128,
            continuations_available: continuation_count,
            continuations_omitted: continuation_count.saturating_sub(continuations.len()),
            continuations,
        },
    ))
}

fn render_cct<'p>(
    profile: &'p Profile,
    node_id: [u32; 2],
    depth: usize,
    state: &mut RenderState<'_, TreeContinuation<'p>>,
) -> Result<(TreeNode<'p>, bool), ApiError> {
    *state.budget -= 1;
    let node = &profile.cct.nodes[node_id[0] as usize];
    let frame = if node_id[0] == profile.cct.root {
        None
    } else {
        Some(profile.stacks.frames[(node.label_offset + node_id[1]) as usize])
    };
    if let Some(id) = frame {
        state.bytes.row_name(profile.frame_name(id))?;
    }
    let next = [node_id[0], node_id[1] + 1];
    let virtual_child = node_id[0] != profile.cct.root && next[1] < node.label_len;
    let structural_children = if virtual_child {
        &[][..]
    } else {
        profile.cct.children(node_id[0])
    };
    let children = virtual_child
        .then_some(next)
        .into_iter()
        .chain(structural_children.iter().map(|&id| [id, 0]));
    let mut rendered = Vec::new();
    let mut omitted_count = 0;
    let mut omitted_weight = 0;
    let mut truncated = false;
    let mut self_weight = node.total_weight;
    for child_id in children {
        let child = &profile.cct.nodes[child_id[0] as usize];
        self_weight -= child.total_weight;
        let reason = if depth >= state.max_depth {
            Some(TreeTruncation::DepthLimit)
        } else if percent(child.total_weight, state.scope) < state.min_percent {
            Some(TreeTruncation::MinScopePercent)
        } else if *state.budget == 0 {
            Some(TreeTruncation::NodeBudget)
        } else {
            None
        };
        if let Some(reason) = reason {
            state.note_truncation(reason, child.total_weight);
            *state.continuation_count += 1;
            omitted_count += 1;
            omitted_weight += child.total_weight;
            truncated = true;
            if state.continuations.len() < state.continuation_limit {
                let frame_id = profile.stacks.frames[(child.label_offset + child_id[1]) as usize];
                state.bytes.row_name(profile.frame_name(frame_id))?;
                state.bytes.json(&profile.source.fingerprint)?;
                state.continuations.push(TreeContinuation {
                    node_id: child_id,
                    frame_id,
                    name: profile.frame_name(frame_id),
                    reason,
                    continuation: Continuation {
                        profile: String::new(),
                        fingerprint: profile.source.fingerprint.clone(),
                        tool: ContinuationTool::Tree,
                        cursor: child_id.to_vec(),
                    },
                    total_weight: child.total_weight,
                    profile_percent: percent(child.total_weight, profile.total_weight),
                    scope_percent: percent(child.total_weight, state.scope),
                });
            }
        } else {
            let (value, child_truncated) = render_cct(profile, child_id, depth + 1, state)?;
            truncated |= child_truncated;
            rendered.push(value);
        }
    }
    let name = frame.map(|id| profile.frame_name(id));
    Ok((
        TreeNode {
            node_id: Some(node_id),
            frame_id: frame,
            name,
            self_weight,
            total_weight: node.total_weight,
            profile_percent: percent(node.total_weight, profile.total_weight),
            scope_percent: percent(node.total_weight, state.scope),
            omitted_children: omitted_count,
            omitted_weight,
            children: rendered,
        },
        truncated,
    ))
}

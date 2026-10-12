use std::collections::BTreeMap;

use crate::output::{
    Continuation, ContinuationTool, ResponseBudget, TreeContinuation, TreeData, TreeNode,
    TreeOutput,
};
use serde_json::json;

use super::{
    ApiError, NodeId, Profile, RenderState, TreeTruncation, check_budget, envelope, percent,
    tree_reason_values,
};

pub(crate) fn tree(
    profile: &Profile,
    root_node_id: NodeId,
    expected_fingerprint: Option<&str>,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
) -> Result<TreeOutput, ApiError> {
    check_budget(max_depth, max_nodes, min_scope_percent)?;
    if root_node_id != profile.cct.root {
        let expected = expected_fingerprint.ok_or_else(|| {
            ApiError::new(
                "profile_changed",
                "Tree continuation requires a profile fingerprint",
                json!({"cursor_node_id":root_node_id}),
                "Copy a returned continuation unchanged, or restart without continuation.",
            )
        })?;
        if expected != profile.source.fingerprint {
            return Err(ApiError::new(
                "profile_changed",
                "Profile fingerprint no longer matches this tree continuation",
                json!({"expected_fingerprint":expected,"current_fingerprint":profile.source.fingerprint}),
                "Restart the query without continuation.",
            ));
        }
    }
    let root = profile
        .cct
        .nodes
        .get(root_node_id as usize)
        .ok_or_else(|| {
            ApiError::new(
                "invalid_node_id",
                format!("Unknown tree continuation cursor node: {root_node_id}"),
                json!({"cursor_node_id":root_node_id}),
                "Restart without continuation, or copy a returned continuation unchanged.",
            )
        })?;
    let mut budget = max_nodes;
    let mut reason_stats = BTreeMap::new();
    let mut continuations = Vec::new();
    let mut continuation_count = 0usize;
    let mut render = RenderState {
        scope: root.total_weight,
        max_depth,
        min_percent: min_scope_percent,
        budget: &mut budget,
        reason_stats: &mut reason_stats,
        continuations: &mut continuations,
        continuation_count: &mut continuation_count,
        continuation_limit: 128,
        frame_path: Vec::new(),
        bytes: ResponseBudget::new(),
    };
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

fn render_cct(
    profile: &Profile,
    node_id: NodeId,
    depth: usize,
    state: &mut RenderState<'_, TreeContinuation>,
) -> Result<(TreeNode, bool), ApiError> {
    *state.budget -= 1;
    let node = &profile.cct.nodes[node_id as usize];
    if let Some(id) = node.frame {
        state.bytes.row_name(profile.frame_name(id))?;
    }
    let children = profile.cct.children(node_id);
    let mut rendered = Vec::new();
    let mut omitted_count = 0;
    let mut omitted_weight = 0;
    let mut truncated = false;
    for &child_id in children {
        let child = &profile.cct.nodes[child_id as usize];
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
                let frame_id = child.frame.expect("non-root child has a frame");
                state.bytes.row_name(profile.frame_name(frame_id))?;
                state.bytes.json(&profile.source.fingerprint)?;
                state.continuations.push(TreeContinuation {
                    node_id: child_id,
                    frame_id,
                    name: profile.frame_name(frame_id).to_owned(),
                    reason,
                    continuation: Continuation {
                        profile: String::new(),
                        fingerprint: profile.source.fingerprint.clone(),
                        tool: ContinuationTool::Tree,
                        cursor: vec![child_id],
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
    let name = node.frame.map(|id| profile.frame_name(id));
    Ok((
        TreeNode {
            node_id: Some(node_id),
            frame_id: node.frame,
            name: name.map(str::to_owned),
            self_weight: node.self_weight,
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

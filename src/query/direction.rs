//! Directional caller/callee trees with stateless `node_path` continuations.

use std::collections::BTreeMap;

use hashbrown::HashMap;
use serde_json::{Value, json};

use super::{
    ApiError, FrameId, FrameSelector, Profile, RenderState, check_budget, envelope, frame_row,
    percent, resolve_selector, tree_reason_values,
};

pub fn callers(
    profile: &Profile,
    selector: &FrameSelector,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
    continuation: Option<(&[u32], &str)>,
) -> Result<Value, ApiError> {
    directional(
        profile,
        selector,
        max_depth,
        max_nodes,
        min_scope_percent,
        true,
        continuation,
    )
}
pub fn callees(
    profile: &Profile,
    selector: &FrameSelector,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
    continuation: Option<(&[u32], &str)>,
) -> Result<Value, ApiError> {
    directional(
        profile,
        selector,
        max_depth,
        max_nodes,
        min_scope_percent,
        false,
        continuation,
    )
}

fn directional(
    profile: &Profile,
    selector: &FrameSelector,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
    callers: bool,
    continuation: Option<(&[u32], &str)>,
) -> Result<Value, ApiError> {
    check_budget(max_depth, max_nodes, min_scope_percent)?;
    let frame = resolve_selector(profile, selector)?;
    let mut root = TempNode::new(frame);
    let mut scope = 0;
    for stack_id in &profile.frame_to_stacks[frame as usize] {
        let stack = &profile.stacks[*stack_id as usize];
        let positions: Vec<_> = stack
            .frames
            .iter()
            .enumerate()
            .filter_map(|(index, id)| (*id == frame).then_some(index))
            .collect();
        let anchor = if callers {
            *positions.last().expect("index points to frame")
        } else {
            positions[0]
        };
        scope += stack.weight;
        root.total_weight += stack.weight;
        let walk: Vec<_> = if callers {
            (0..anchor).rev().map(|index| stack.frames[index]).collect()
        } else {
            ((anchor + 1)..stack.frames.len())
                .map(|index| stack.frames[index])
                .collect()
        };
        if walk.is_empty() {
            root.self_weight += stack.weight;
        } else {
            root.insert(&walk, stack.weight);
        }
    }
    // Resolve the continuation target before rendering. The temp tree is a
    // deterministic function of (profile fingerprint, frame, direction), so a
    // node_path from an earlier response addresses the same subtree here.
    let mut render_root: &TempNode = &root;
    if let Some((node_path, expected_fingerprint)) = continuation {
        if node_path.is_empty() || node_path.len() > 4096 {
            return Err(ApiError::new(
                "invalid_node_id",
                "continuation.node_path must contain between 1 and 4096 frame ids",
                json!({"node_path_len":node_path.len()}),
                "Use a node_path returned by this direction query's continuations.",
            ));
        }
        if expected_fingerprint != profile.source.fingerprint {
            return Err(ApiError::new(
                "profile_changed",
                "Profile fingerprint no longer matches this direction continuation",
                json!({"expected_fingerprint":expected_fingerprint,"current_fingerprint":profile.source.fingerprint}),
                "Restart from the anchor frame without continuation.",
            ));
        }
        let mut current = &root;
        for frame_id in node_path {
            match current.children.get(frame_id) {
                Some(child) => current = child,
                None => {
                    return Err(ApiError::new(
                        "invalid_node_id",
                        format!("Unknown continuation node_path at frame id {frame_id}"),
                        json!({"node_path":node_path}),
                        "Restart from the anchor frame; the omitted subtree shape changed.",
                    ));
                }
            }
        }
        render_root = current;
        scope = current.total_weight;
    }
    let mut budget = max_nodes;
    let mut reason_stats = BTreeMap::new();
    let mut continuations = Vec::new();
    let mut continuation_count = 0usize;
    let mut render = RenderState {
        scope,
        max_depth,
        min_percent: min_scope_percent,
        budget: &mut budget,
        reason_stats: &mut reason_stats,
        continuations: &mut continuations,
        continuation_count: &mut continuation_count,
        continuation_limit: 128,
        frame_path: continuation
            .map(|(path, _)| path.to_vec())
            .unwrap_or_default(),
    };
    let (node, _) = render_temp(profile, render_root, 0, &mut render);
    let truncation_reasons =
        tree_reason_values(&reason_stats, max_depth, max_nodes, min_scope_percent);
    Ok(envelope(
        profile,
        scope,
        truncation_reasons,
        Vec::new(),
        json!({
            "frame":frame_row(profile, frame, &profile.frame_stats[frame as usize], scope),
            "root":node,
            "continuations":continuations,
            "continuations_truncated":continuation_count > continuations.len(),
            "continuation_limit":128,
            "continuations_available":continuation_count,
            "continuations_omitted":continuation_count.saturating_sub(continuations.len())
        }),
    ))
}

#[derive(Default)]
struct TempNode {
    frame: FrameId,
    self_weight: u64,
    total_weight: u64,
    children: HashMap<FrameId, TempNode>,
}

impl TempNode {
    fn new(frame: FrameId) -> Self {
        Self {
            frame,
            ..Self::default()
        }
    }
    fn insert(&mut self, frames: &[FrameId], weight: u64) {
        let frame = frames[0];
        let child = self
            .children
            .entry(frame)
            .or_insert_with(|| TempNode::new(frame));
        child.total_weight += weight;
        if frames.len() == 1 {
            child.self_weight += weight;
        } else {
            child.insert(&frames[1..], weight);
        }
    }
}

fn ordered_children<'a>(
    profile: &Profile,
    children: impl Iterator<Item = (FrameId, &'a TempNode)>,
) -> Vec<(FrameId, &'a TempNode)> {
    let mut children: Vec<_> = children.collect();
    children.sort_by(|(a, an), (b, bn)| {
        bn.total_weight
            .cmp(&an.total_weight)
            .then_with(|| profile.frame_name(*a).cmp(profile.frame_name(*b)))
            .then(a.cmp(b))
    });
    children
}
fn render_temp(
    profile: &Profile,
    node: &TempNode,
    depth: usize,
    state: &mut RenderState<'_>,
) -> (Value, bool) {
    *state.budget -= 1;
    let children = ordered_children(profile, node.children.iter().map(|(id, node)| (*id, node)));
    let mut rendered = Vec::new();
    let mut omitted_count = 0;
    let mut omitted_weight = 0;
    let mut truncated = false;
    for (id, child) in children {
        let reason = if depth >= state.max_depth {
            Some("depth_limit")
        } else if percent(child.total_weight, state.scope) < state.min_percent {
            Some("min_scope_percent")
        } else if *state.budget == 0 {
            Some("node_budget")
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
                let mut node_path = state.frame_path.clone();
                node_path.push(id);
                state.continuations.push(json!({
                    "node_path":node_path,
                    "frame_id":id,
                    "name":profile.frame_name(id),
                    "reason":reason,
                    "profile_fingerprint":profile.source.fingerprint,
                    "total_weight":child.total_weight,
                    "profile_percent":percent(child.total_weight,profile.total_weight),
                    "scope_percent":percent(child.total_weight,state.scope)
                }));
            }
        } else {
            state.frame_path.push(id);
            let (value, child_truncated) = render_temp(profile, child, depth + 1, state);
            state.frame_path.pop();
            truncated |= child_truncated;
            rendered.push(value);
        }
    }
    (
        json!({"node_id":Value::Null,"frame_id":node.frame,"name":profile.frame_name(node.frame),"self_weight":node.self_weight,"total_weight":node.total_weight,"profile_percent":percent(node.total_weight,profile.total_weight),"scope_percent":percent(node.total_weight,state.scope),"omitted_children":omitted_count,"omitted_weight":omitted_weight,"children":rendered}),
        truncated,
    )
}

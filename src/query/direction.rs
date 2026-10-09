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
    let prefix = continuation.map(|(path, _)| path).unwrap_or_default();
    if let Some((node_path, expected_fingerprint)) = continuation {
        if node_path.is_empty() || node_path.len() > 4096 {
            return Err(ApiError::new(
                "invalid_node_id",
                "Continuation cursor must contain an anchor and between 1 and 4096 path frame ids",
                json!({"cursor_path_length":node_path.len()}),
                "Copy a returned continuation unchanged, or restart with frame and no continuation.",
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
    }
    let mut root = TempNode::new(prefix.last().copied().unwrap_or(frame));
    let mut work_remaining = MAX_DIRECTION_NODES - 1;
    for stack_id in &profile.frame_to_stacks[frame as usize] {
        let stack = &profile.stacks[*stack_id as usize];
        let anchor = if callers {
            stack.frames.iter().rposition(|id| *id == frame)
        } else {
            stack.frames.iter().position(|id| *id == frame)
        }
        .expect("index points to frame");
        let walk_len = if callers {
            anchor
        } else {
            stack.frames.len() - anchor - 1
        };
        let frame_at = |offset: usize| {
            stack.frames[if callers {
                anchor - offset - 1
            } else {
                anchor + offset + 1
            }]
        };
        if prefix
            .iter()
            .enumerate()
            .any(|(offset, id)| offset >= walk_len || frame_at(offset) != *id)
        {
            continue;
        }
        root.total_weight += stack.weight;
        let remaining_len = walk_len - prefix.len();
        if remaining_len == 0 {
            root.self_weight += stack.weight;
        } else {
            // One boundary layer keeps omitted weights and continuation paths exact.
            let walk: Vec<_> = (prefix.len()..walk_len)
                .take(max_depth + 1)
                .map(frame_at)
                .collect();
            root.insert(
                &walk,
                stack.weight,
                remaining_len == walk.len(),
                &mut work_remaining,
            )?;
        }
    }
    if root.total_weight == 0 {
        return Err(ApiError::new(
            "invalid_node_id",
            "Unknown continuation cursor path",
            json!({"cursor_path":prefix}),
            "Restart with frame and no continuation; the omitted subtree shape changed.",
        ));
    }
    let scope = root.total_weight;
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
    let (node, _) = render_temp(profile, &root, 0, &mut render);
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

const MAX_DIRECTION_NODES: usize = 100_000;

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
    fn insert(
        &mut self,
        frames: &[FrameId],
        weight: u64,
        complete: bool,
        work_remaining: &mut usize,
    ) -> Result<(), ApiError> {
        let frame = frames[0];
        let child = match self.children.entry(frame) {
            hashbrown::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            hashbrown::hash_map::Entry::Vacant(entry) => {
                if *work_remaining == 0 {
                    return Err(ApiError::new(
                        "query_too_large",
                        "Direction query exceeds its working node budget",
                        json!({"resource":"direction_nodes", "limit":MAX_DIRECTION_NODES}),
                        "Use a shallower max_depth or a narrower continuation subtree.",
                    ));
                }
                *work_remaining -= 1;
                entry.insert(TempNode::new(frame))
            }
        };
        child.total_weight += weight;
        if frames.len() == 1 {
            if complete {
                child.self_weight += weight;
            }
        } else {
            child.insert(&frames[1..], weight, complete, work_remaining)?;
        }
        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_budget() {
        let mut root = TempNode::new(0);
        let mut budget = 2;
        root.insert(&[1, 2], 7, false, &mut budget).unwrap();
        let child = &root.children[&1].children[&2];
        assert_eq!((child.total_weight, child.self_weight), (7, 0));
        assert!(child.children.is_empty());
        root.insert(&[1, 2], 3, true, &mut budget).unwrap();
        let child = &root.children[&1].children[&2];
        assert_eq!((child.total_weight, child.self_weight), (10, 3));
        assert_eq!(budget, 0);
        let error = root.insert(&[1, 3], 1, true, &mut budget).unwrap_err();
        assert_eq!(error.code, "query_too_large");
        assert!(!root.children[&1].children.contains_key(&3));
    }
}

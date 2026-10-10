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
    let mut tree = TempTree::new(prefix.last().copied().unwrap_or(frame));
    let mut work_remaining = MAX_DIRECTION_NODES - 1;
    for stack_id in &profile.frame_to_stacks[frame as usize] {
        let stack = profile.stacks.stack(*stack_id);
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
        tree.nodes[0].total_weight += stack.weight;
        let remaining_len = walk_len - prefix.len();
        if remaining_len == 0 {
            tree.nodes[0].self_weight += stack.weight;
        } else {
            // One boundary layer keeps omitted weights and continuation paths exact.
            let mut walk = [0u32; 17];
            let len = remaining_len.min(max_depth + 1);
            for (offset, slot) in walk[..len].iter_mut().enumerate() {
                *slot = frame_at(prefix.len() + offset);
            }
            tree.insert(
                &walk[..len],
                stack.weight,
                remaining_len == len,
                &mut work_remaining,
            )?;
        }
    }
    if tree.nodes[0].total_weight == 0 {
        return Err(ApiError::new(
            "invalid_node_id",
            "Unknown continuation cursor path",
            json!({"cursor_path":prefix}),
            "Restart with frame and no continuation; the omitted subtree shape changed.",
        ));
    }
    let scope = tree.nodes[0].total_weight;
    drop(tree.edges);
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
    let (node, _) = render_temp(profile, &tree.nodes, 0, 0, &mut render);
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

const NO_CHILD: u32 = u32::MAX;

struct TempNode {
    frame: FrameId,
    self_weight: u64,
    total_weight: u64,
    first_child: u32,
    next_sibling: u32,
}

impl TempNode {
    fn new(frame: FrameId, next_sibling: u32) -> Self {
        Self {
            frame,
            self_weight: 0,
            total_weight: 0,
            first_child: NO_CHILD,
            next_sibling,
        }
    }
}

struct TempTree {
    nodes: Vec<TempNode>,
    edges: HashMap<u64, u32>,
}

impl TempTree {
    fn new(frame: FrameId) -> Self {
        Self {
            nodes: vec![TempNode::new(frame, NO_CHILD)],
            edges: HashMap::new(),
        }
    }

    fn insert(
        &mut self,
        frames: &[FrameId],
        weight: u64,
        complete: bool,
        work_remaining: &mut usize,
    ) -> Result<(), ApiError> {
        let mut parent = 0;
        for &frame in frames {
            let key = (u64::from(parent) << 32) | u64::from(frame);
            let first_child = self.nodes[parent as usize].first_child;
            let child = if first_child != NO_CHILD
                && self.nodes[first_child as usize].frame == frame
            {
                first_child
            } else {
                match self.edges.entry(key) {
                    hashbrown::hash_map::Entry::Occupied(entry) => *entry.get(),
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
                        let child = self.nodes.len() as u32;
                        self.nodes.push(TempNode::new(
                            frame,
                            self.nodes[parent as usize].first_child,
                        ));
                        self.nodes[parent as usize].first_child = child;
                        entry.insert(child);
                        child
                    }
                }
            };
            self.nodes[child as usize].total_weight += weight;
            parent = child;
        }
        if complete {
            self.nodes[parent as usize].self_weight += weight;
        }
        Ok(())
    }
}

fn ordered_children(profile: &Profile, nodes: &[TempNode], node: &TempNode) -> Vec<u32> {
    let mut children = Vec::new();
    let mut child = node.first_child;
    while child != NO_CHILD {
        children.push(child);
        child = nodes[child as usize].next_sibling;
    }
    children.sort_unstable_by(|a, b| {
        let an = &nodes[*a as usize];
        let bn = &nodes[*b as usize];
        bn.total_weight
            .cmp(&an.total_weight)
            .then_with(|| {
                profile
                    .frame_name(an.frame)
                    .cmp(profile.frame_name(bn.frame))
            })
            .then(an.frame.cmp(&bn.frame))
    });
    children
}
fn render_temp(
    profile: &Profile,
    nodes: &[TempNode],
    node_id: u32,
    depth: usize,
    state: &mut RenderState<'_>,
) -> (Value, bool) {
    *state.budget -= 1;
    let node = &nodes[node_id as usize];
    let children = ordered_children(profile, nodes, node);
    let mut rendered = Vec::new();
    let mut omitted_count = 0;
    let mut omitted_weight = 0;
    let mut truncated = false;
    for child_id in children {
        let child = &nodes[child_id as usize];
        let id = child.frame;
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
            let (value, child_truncated) = render_temp(profile, nodes, child_id, depth + 1, state);
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
        let mut tree = TempTree::new(0);
        let mut budget = 2;
        tree.insert(&[1, 2], 7, false, &mut budget).unwrap();
        let child = &tree.nodes[2];
        assert_eq!((child.total_weight, child.self_weight), (7, 0));
        assert_eq!(child.first_child, NO_CHILD);
        tree.insert(&[1, 2], 3, true, &mut budget).unwrap();
        let child = &tree.nodes[2];
        assert_eq!((child.total_weight, child.self_weight), (10, 3));
        assert_eq!(budget, 0);
        let error = tree.insert(&[1, 3], 1, true, &mut budget).unwrap_err();
        assert_eq!(error.code, "query_too_large");
        assert_eq!(tree.nodes.len(), 3);
        assert!(!tree.edges.contains_key(&((1u64 << 32) | 3)));
    }

    #[test]
    fn arena_edges_keep_parent_context_and_siblings() {
        let mut tree = TempTree::new(0);
        let mut budget = 10;
        tree.insert(&[1, 3], 7, true, &mut budget).unwrap();
        tree.insert(&[2, 3], 5, true, &mut budget).unwrap();
        tree.insert(&[1, 3], 2, true, &mut budget).unwrap();
        assert_eq!(tree.nodes.len(), 5);
        assert_eq!(tree.nodes[0].first_child, 3);
        assert_eq!(tree.nodes[3].next_sibling, 1);
        assert_eq!(tree.nodes[1].next_sibling, NO_CHILD);
        assert_eq!(tree.edges[&((1u64 << 32) | 3)], 2);
        assert_eq!(tree.edges[&((3u64 << 32) | 3)], 4);
        assert_eq!(tree.nodes[2].self_weight, 9);
        assert_eq!(tree.nodes[4].self_weight, 5);
        assert_eq!(budget, 6);
    }

    #[test]
    fn packed_edges_cover_full_frame_id_range() {
        let mut tree = TempTree::new(0);
        let mut budget = 10;
        tree.insert(&[u32::MAX, u32::MAX], 7, true, &mut budget)
            .unwrap();
        tree.insert(&[0, u32::MAX], 5, true, &mut budget).unwrap();
        assert_eq!(tree.edges[&u64::from(u32::MAX)], 1);
        assert_eq!(tree.edges[&((1u64 << 32) | u64::from(u32::MAX))], 2);
        assert_eq!(tree.edges[&((3u64 << 32) | u64::from(u32::MAX))], 4);
        assert_eq!(tree.nodes[2].self_weight, 7);
        assert_eq!(tree.nodes[4].self_weight, 5);
    }
}

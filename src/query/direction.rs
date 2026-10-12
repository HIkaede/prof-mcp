//! Directional caller/callee trees with stateless `node_path` continuations.

use std::collections::BTreeMap;

use crate::output::{
    Continuation, ContinuationTool, DirectionContinuation, DirectionData, DirectionOutput,
    ResponseBudget, TreeNode,
};
use hashbrown::HashMap;
use serde_json::json;

use super::{
    ApiError, FrameId, FrameSelector, Profile, RenderState, TreeTruncation, check_budget, envelope,
    frame_row, percent, resolve_selector, tree_reason_values,
};

pub fn callers<'p>(
    profile: &'p Profile,
    selector: &FrameSelector,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
    continuation: Option<(&[u32], &str)>,
) -> Result<DirectionOutput<'p>, ApiError> {
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
pub fn callees<'p>(
    profile: &'p Profile,
    selector: &FrameSelector,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
    continuation: Option<(&[u32], &str)>,
) -> Result<DirectionOutput<'p>, ApiError> {
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

fn directional<'p>(
    profile: &'p Profile,
    selector: &FrameSelector,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
    callers: bool,
    continuation: Option<(&[u32], &str)>,
) -> Result<DirectionOutput<'p>, ApiError> {
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
    let positions = profile
        .frame_to_stacks
        .constant_positions_pair(frame as usize);
    let variable_positions = if positions.is_none() {
        let offsets = profile.frame_to_stacks.position_offsets();
        &profile.frame_to_stacks.variable_positions()
            [offsets[frame as usize] as usize..offsets[frame as usize + 1] as usize]
    } else {
        &[][..]
    };
    let mut walk = Walk {
        profile,
        anchor: frame,
        callers,
        prefix_len: prefix.len(),
        ranked: profile.frame_to_stacks.ranked(frame as usize),
        variable_positions,
        representative_weights: Vec::new(),
        constant_anchor: positions
            .map(|(first, last)| usize::from(if callers { last } else { first })),
        work_remaining: MAX_DIRECTION_NODES - 1,
        terminal: if callers {
            positions.is_some_and(|(_, last)| last == 0)
        } else if profile.frame_stats[frame as usize].self_weight
            == profile.frame_stats[frame as usize].inclusive_weight
        {
            positions.map_or_else(
                || {
                    profile
                        .frame_to_stacks
                        .ranked(frame as usize)
                        .iter()
                        .enumerate()
                        .all(|(rank, _)| {
                            let (first, last) =
                                profile.frame_to_stacks.positions(frame as usize, rank);
                            first == last
                        })
                },
                |(first, last)| first == last,
            )
        } else {
            false
        },
    };
    let mut filtered = Vec::new();
    let mut partial = false;
    let scope = if prefix.is_empty() {
        profile.frame_stats[frame as usize].inclusive_weight
    } else {
        let mut scope = 0;
        for rank in 0..walk.ranked.len() {
            let matches = prefix
                .iter()
                .enumerate()
                .all(|(offset, &id)| walk.next(rank as u32, offset).0 == Some(id));
            if matches {
                scope += profile.stacks.stack(walk.ranked[rank]).weight;
                if partial {
                    filtered.push(rank as u32);
                }
            } else if !partial {
                filtered.extend(0..rank as u32);
                partial = true;
            }
        }
        scope
    };
    if scope == 0 {
        return Err(ApiError::new(
            "invalid_node_id",
            "Unknown continuation cursor path",
            json!({"cursor_path":prefix}),
            "Restart with frame and no continuation; the omitted subtree shape changed.",
        ));
    }
    let members = if partial {
        Members::Indices(&mut filtered)
    } else {
        Members::Ranks(0..walk.ranked.len())
    };
    let mut representatives = walk.collapse_callers(&members, max_depth + 1, max_nodes);
    let members = if let Some(ranks) = &mut representatives {
        Members::Indices(ranks)
    } else {
        members
    };
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
        bytes: ResponseBudget::new(),
        frame_path: continuation
            .map(|(path, _)| path.to_vec())
            .unwrap_or_default(),
    };
    render.bytes.row_name(profile.frame_name(frame))?;
    let root = Node {
        frame: prefix.last().copied().unwrap_or(frame),
        weight: scope,
        terminal: walk.terminal,
    };
    let node = walk.render(members, root, 0, &mut render, None)?;
    let truncation_reasons =
        tree_reason_values(&reason_stats, max_depth, max_nodes, min_scope_percent);
    Ok(envelope(
        profile,
        scope,
        truncation_reasons,
        Vec::new(),
        DirectionData {
            frame: frame_row(profile, frame, &profile.frame_stats[frame as usize], scope),
            root: node,
            continuations_truncated: continuation_count > continuations.len(),
            continuation_limit: 128,
            continuations_available: continuation_count,
            continuations_omitted: continuation_count.saturating_sub(continuations.len()),
            continuations,
        },
    ))
}

const MAX_DIRECTION_NODES: usize = 100_000;

// Original scope ranks preserve variable anchor positions after partitioning.
enum Members<'a> {
    Ranks(std::ops::Range<usize>),
    Indices(&'a mut [u32]),
}
impl Members<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Ranks(range) => range.len(),
            Self::Indices(indices) => indices.len(),
        }
    }
    fn rank(&self, index: usize) -> u32 {
        match self {
            Self::Ranks(range) => (range.start + index) as u32,
            Self::Indices(indices) => indices[index],
        }
    }
}

#[derive(Clone, Copy)]
struct Node {
    frame: FrameId,
    terminal: bool,
    weight: u64,
}
struct Child {
    node: Node,
    start: usize,
    end: usize,
}
struct Walk<'p> {
    profile: &'p Profile,
    anchor: FrameId,
    callers: bool,
    prefix_len: usize,
    ranked: &'p [u32],
    constant_anchor: Option<usize>,
    variable_positions: &'p [u32],
    representative_weights: Vec<(u32, u64)>,
    work_remaining: usize,
    terminal: bool,
}
impl<'p> Walk<'p> {
    fn collapse_callers(
        &mut self,
        members: &Members<'_>,
        depth: usize,
        max_nodes: usize,
    ) -> Option<Vec<u32>> {
        let limit = max_nodes.min(members.len() / 32);
        if !self.callers || self.constant_anchor.is_some() || limit == 0 {
            return None;
        }
        // Compare borrowed reverse-walk windows, including the omitted-child boundary.
        let mut paths = HashMap::<&[FrameId], (u32, u64)>::new();
        for index in 0..members.len() {
            let rank = members.rank(index);
            let stack = self.profile.stacks.stack(self.ranked[rank as usize]);
            let end = self.anchor_position(rank) - self.prefix_len;
            let key = &stack.frames[end - end.min(depth)..end];
            if paths.len() == limit && !paths.contains_key(key) {
                return None;
            }
            match paths.entry(key) {
                hashbrown::hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().1 += stack.weight
                }
                hashbrown::hash_map::Entry::Vacant(entry) => {
                    entry.insert((rank, stack.weight));
                }
            }
        }
        self.representative_weights = paths.into_values().collect();
        self.representative_weights
            .sort_unstable_by_key(|&(rank, _)| rank);
        Some(
            self.representative_weights
                .iter()
                .map(|&(rank, _)| rank)
                .collect(),
        )
    }
    fn anchor_position(&self, rank: u32) -> usize {
        self.constant_anchor.unwrap_or_else(|| {
            let packed = self.variable_positions[rank as usize];
            if self.callers {
                (packed >> 16) as usize
            } else {
                usize::from(packed as u16)
            }
        })
    }
    fn next(&self, rank: u32, offset: usize) -> (Option<FrameId>, u64, bool) {
        let stack = self.profile.stacks.stack(self.ranked[rank as usize]);
        let anchor = self.anchor_position(rank);
        let position = if self.callers {
            anchor.checked_sub(offset + 1)
        } else {
            Some(anchor + offset + 1)
        };
        let frame = position.and_then(|position| stack.frames.get(position).copied());
        let terminal = frame.is_some()
            && position.is_some_and(|position| {
                if self.callers {
                    position == 0
                } else {
                    position + 1 == stack.frames.len()
                }
            });
        let weight = if self.representative_weights.is_empty() {
            stack.weight
        } else {
            let index = self
                .representative_weights
                .binary_search_by_key(&rank, |&(rank, _)| rank)
                .expect("member rank is a weighted representative");
            self.representative_weights[index].1
        };
        (frame, weight, terminal)
    }
    // Compare a bounded shared prefix once, then render it without rescanning members.
    fn common_segment(
        &self,
        members: &Members<'_>,
        offset: usize,
        limit: usize,
        frames: &mut [FrameId; 17],
    ) -> (usize, bool) {
        let rank = members.rank(0);
        let first = self.profile.stacks.stack(self.ranked[rank as usize]);
        let anchor = self.anchor_position(rank);
        let available = if self.callers {
            anchor
        } else {
            first.frames.len() - anchor - 1
        };
        let mut max_remaining = available.saturating_sub(offset);
        let mut len = limit.min(max_remaining);
        for (position, frame) in frames[..len].iter_mut().enumerate() {
            *frame = first.frames[if self.callers {
                anchor - offset - position - 1
            } else {
                anchor + offset + position + 1
            }];
        }
        if len == 0 {
            return (0, false);
        }
        for index in 1..members.len() {
            let rank = members.rank(index);
            let stack = self.profile.stacks.stack(self.ranked[rank as usize]);
            let anchor = self.anchor_position(rank);
            let available = if self.callers {
                anchor
            } else {
                stack.frames.len() - anchor - 1
            };
            let remaining = available.saturating_sub(offset);
            max_remaining = max_remaining.max(remaining);
            len = len.min(remaining);
            for (position, &frame) in frames[..len].iter().enumerate() {
                let position_in_stack = if self.callers {
                    anchor - offset - position - 1
                } else {
                    anchor + offset + position + 1
                };
                if stack.frames[position_in_stack] != frame {
                    len = position;
                    break;
                }
            }
            if len == 0 {
                break;
            }
        }
        (len, len > 0 && max_remaining == len)
    }
    fn child_reason(
        &self,
        child: Node,
        depth: usize,
        state: &RenderState<'_, DirectionContinuation<'p>>,
    ) -> Option<TreeTruncation> {
        if depth >= state.max_depth {
            Some(TreeTruncation::DepthLimit)
        } else if percent(child.weight, state.scope) < state.min_percent {
            Some(TreeTruncation::MinScopePercent)
        } else if *state.budget == 0 {
            Some(TreeTruncation::NodeBudget)
        } else {
            None
        }
    }
    fn omit(
        &self,
        child: Node,
        reason: TreeTruncation,
        state: &mut RenderState<'_, DirectionContinuation<'p>>,
    ) -> Result<(), ApiError> {
        state.note_truncation(reason, child.weight);
        *state.continuation_count += 1;
        if state.continuations.len() < state.continuation_limit {
            state.bytes.row_name(self.profile.frame_name(child.frame))?;
            state.bytes.json(&state.frame_path)?;
            state.bytes.json(&self.profile.source.fingerprint)?;
            let mut cursor = Vec::with_capacity(state.frame_path.len() + 2);
            cursor.push(self.anchor);
            cursor.extend_from_slice(&state.frame_path);
            cursor.push(child.frame);
            state.continuations.push(DirectionContinuation {
                frame_id: child.frame,
                name: self.profile.frame_name(child.frame),
                reason,
                continuation: Continuation {
                    profile: String::new(),
                    fingerprint: self.profile.source.fingerprint.clone(),
                    tool: if self.callers {
                        ContinuationTool::Callers
                    } else {
                        ContinuationTool::Callees
                    },
                    cursor,
                },
                total_weight: child.weight,
                profile_percent: percent(child.weight, self.profile.total_weight),
                scope_percent: percent(child.weight, state.scope),
            });
        }
        Ok(())
    }
    fn charge(&mut self) -> Result<(), ApiError> {
        if self.work_remaining == 0 {
            return Err(ApiError::new(
                "query_too_large",
                "Direction query exceeds its working node budget",
                json!({"resource":"direction_nodes", "limit":MAX_DIRECTION_NODES}),
                "Use a shallower max_depth or a narrower continuation subtree.",
            ));
        }
        self.work_remaining -= 1;
        Ok(())
    }
    fn render(
        &mut self,
        members: Members<'_>,
        node: Node,
        depth: usize,
        state: &mut RenderState<'_, DirectionContinuation<'p>>,
        known_common: Option<(&[FrameId], bool)>,
    ) -> Result<TreeNode<'p>, ApiError> {
        let terminal = node.terminal;
        let mut common_frames = [0u32; 17];
        let (common, ends_here) = if terminal {
            (&[][..], true)
        } else if let Some(common) = known_common {
            common
        } else {
            let (len, ends_here) = self.common_segment(
                &members,
                self.prefix_len + depth,
                (state.max_depth - depth + 1).min(*state.budget),
                &mut common_frames,
            );
            (&common_frames[..len], ends_here)
        };
        *state.budget -= 1;
        state.bytes.row_name(self.profile.frame_name(node.frame))?;
        let mut output = TreeNode {
            node_id: None,
            frame_id: Some(node.frame),
            name: Some(self.profile.frame_name(node.frame)),
            self_weight: if terminal { node.weight } else { 0 },
            total_weight: node.weight,
            profile_percent: percent(node.weight, self.profile.total_weight),
            scope_percent: percent(node.weight, state.scope),
            omitted_children: 0,
            omitted_weight: 0,
            children: Vec::new(),
        };
        if terminal {
            return Ok(output);
        }
        if let Some((&frame, rest)) = common.split_first() {
            self.charge()?;
            let child = Node {
                frame,
                weight: node.weight,
                terminal: rest.is_empty() && ends_here,
            };
            if let Some(reason) = self.child_reason(child, depth, state) {
                self.omit(child, reason, state)?;
                output.omitted_children = 1;
                output.omitted_weight = child.weight;
            } else {
                state.frame_path.push(frame);
                output.children.push(self.render(
                    members,
                    child,
                    depth + 1,
                    state,
                    Some((rest, ends_here)),
                )?);
                state.frame_path.pop();
            }
            return Ok(output);
        }
        let offset = self.prefix_len + depth;
        let mut groups = HashMap::new();
        let mut children = Vec::<Child>::new();
        for index in 0..members.len() {
            let rank = members.rank(index);
            let (next, weight, terminal) = self.next(rank, offset);
            if let Some(next) = next {
                let index = if children
                    .first()
                    .is_some_and(|child| child.node.frame == next)
                {
                    0
                } else {
                    match groups.entry(next) {
                        hashbrown::hash_map::Entry::Occupied(entry) => *entry.get(),
                        hashbrown::hash_map::Entry::Vacant(entry) => {
                            self.charge()?;
                            let index = children.len();
                            children.push(Child {
                                node: Node {
                                    frame: next,
                                    weight: 0,
                                    terminal: true,
                                },
                                start: 0,
                                end: 0,
                            });
                            entry.insert(index);
                            index
                        }
                    }
                };
                let child = &mut children[index];
                child.node.weight += weight;
                child.node.terminal &= terminal;
                child.end += 1;
            } else {
                output.self_weight += weight;
            }
        }
        let selected = children.iter().any(|child| {
            !child.node.terminal && self.child_reason(child.node, depth, state).is_none()
        });
        let mut owned = Vec::new();
        let indices = if selected {
            let indices = match members {
                Members::Ranks(range) => {
                    owned.extend(range.map(|rank| rank as u32));
                    owned.as_mut_slice()
                }
                Members::Indices(indices) => indices,
            };
            let mut start = 0;
            for child in &mut children {
                child.start = start;
                child.end += start;
                start = child.end;
            }
            let mut cursors: Vec<_> = children.iter().map(|child| child.start).collect();
            cursors.push(start);
            for index in 0..=children.len() {
                let end = children.get(index).map_or(indices.len(), |child| child.end);
                while cursors[index] < end {
                    let target =
                        self.next(indices[cursors[index]], offset)
                            .0
                            .map_or(children.len(), |id| {
                                if children.first().is_some_and(|child| child.node.frame == id) {
                                    0
                                } else {
                                    groups[&id]
                                }
                            });
                    if target == index {
                        cursors[index] += 1;
                    } else {
                        indices.swap(cursors[index], cursors[target]);
                        cursors[target] += 1;
                    }
                }
            }
            indices
        } else {
            &mut [][..]
        };
        drop(groups);
        children.sort_unstable_by(|a, b| {
            b.node
                .weight
                .cmp(&a.node.weight)
                .then(a.node.frame.cmp(&b.node.frame))
        });
        for child in children {
            if let Some(reason) = self.child_reason(child.node, depth, state) {
                self.omit(child.node, reason, state)?;
                output.omitted_children += 1;
                output.omitted_weight += child.node.weight;
            } else {
                state.frame_path.push(child.node.frame);
                let members = if child.node.terminal {
                    Members::Ranks(0..0)
                } else {
                    Members::Indices(&mut indices[child.start..child.end])
                };
                output
                    .children
                    .push(self.render(members, child.node, depth + 1, state, None)?);
                state.frame_path.pop();
            }
        }
        Ok(output)
    }
}

#[cfg(test)]
#[path = "direction/oracle.rs"]
mod oracle;
#[cfg(test)]
#[path = "direction/tests.rs"]
mod tests;

use crate::error::{ApiError, ProfileError};

use super::model::{ContextCallTree, ContextNode, StackTable};

struct PendingNode {
    // total_weight stores the cumulative start until this subtree closes.
    node: ContextNode,
    parent: u32,
    depth: u32,
}

fn offset_overflow() -> ApiError {
    ApiError::new(
        "profile_model_too_large",
        "Profile exceeds the compact index address space",
        serde_json::json!({"resource":"index_entries", "limit":u32::MAX}),
        "Split the profile into smaller inputs.",
    )
}

fn compact(value: usize) -> Result<u32, ProfileError> {
    u32::try_from(value).map_err(|_| offset_overflow().into())
}

// Lexical stacks let us close whole subtrees and split only necessary LCP boundaries.
pub(crate) fn build_cct(
    stacks: &StackTable,
    total_weight: u64,
) -> Result<ContextCallTree, ProfileError> {
    let mut nodes = vec![PendingNode {
        node: ContextNode {
            label_offset: 0,
            label_len: 0,
            total_weight,
        },
        parent: 0,
        depth: 0,
    }];
    let mut active = vec![0usize];
    let mut previous = &[][..];
    let mut completed_weight = 0u64;
    for (sid, stack) in stacks.iter().enumerate() {
        let common = previous
            .iter()
            .zip(stack.frames)
            .take_while(|(a, b)| a == b)
            .count();
        let mut last_closed = None;
        while nodes[*active.last().unwrap()].depth as usize > common {
            let id = active.pop().unwrap();
            nodes[id].node.total_weight = completed_weight - nodes[id].node.total_weight;
            last_closed = Some(id);
        }
        let mut parent = *active.last().unwrap();
        if (nodes[parent].depth as usize) < common {
            let child = last_closed.expect("LCP inside a previously visited edge");
            let length = common - nodes[parent].depth as usize;
            let branch = nodes.len();
            nodes.push(PendingNode {
                node: ContextNode {
                    label_offset: nodes[child].node.label_offset,
                    label_len: compact(length)?,
                    total_weight: completed_weight - nodes[child].node.total_weight,
                },
                parent: compact(parent)?,
                depth: compact(common)?,
            });
            nodes[child].parent = compact(branch)?;
            nodes[child].node.label_offset = nodes[child]
                .node
                .label_offset
                .checked_add(compact(length)?)
                .ok_or_else(offset_overflow)?;
            nodes[child].node.label_len -= compact(length)?;
            active.push(branch);
            parent = branch;
        }
        let id = nodes.len();
        nodes.push(PendingNode {
            node: ContextNode {
                label_offset: stacks.offsets()[sid]
                    .checked_add(compact(common)?)
                    .ok_or_else(offset_overflow)?,
                label_len: compact(stack.frames.len() - common)?,
                total_weight: completed_weight,
            },
            parent: compact(parent)?,
            depth: compact(stack.frames.len())?,
        });
        active.push(id);
        completed_weight = completed_weight
            .checked_add(stack.weight)
            .ok_or_else(offset_overflow)?;
        previous = stack.frames;
    }
    while active.len() > 1 {
        let id = active.pop().unwrap();
        nodes[id].node.total_weight = completed_weight - nodes[id].node.total_weight;
    }
    debug_assert_eq!(completed_weight, total_weight);
    let mut offsets = vec![0u32; nodes.len() + 1];
    for node in &nodes[1..] {
        let count = &mut offsets[node.parent as usize + 1];
        *count = count.checked_add(1).ok_or_else(offset_overflow)?;
    }
    for i in 1..offsets.len() {
        offsets[i] = offsets[i]
            .checked_add(offsets[i - 1])
            .ok_or_else(offset_overflow)?;
    }
    let mut cursors = offsets[..nodes.len()].to_vec();
    let mut children = vec![0u32; nodes.len() - 1];
    for (id, node) in nodes.iter().enumerate().skip(1) {
        let cursor = &mut cursors[node.parent as usize];
        children[*cursor as usize] = compact(id)?;
        *cursor += 1;
    }
    drop(cursors);
    for range in offsets.windows(2) {
        children[range[0] as usize..range[1] as usize].sort_unstable_by(|a, b| {
            let an = &nodes[*a as usize].node;
            let bn = &nodes[*b as usize].node;
            bn.total_weight
                .cmp(&an.total_weight)
                .then_with(|| {
                    stacks.frames()[an.label_offset as usize]
                        .cmp(&stacks.frames()[bn.label_offset as usize])
                })
                .then(a.cmp(b))
        });
    }
    Ok(ContextCallTree {
        nodes: nodes
            .into_iter()
            .map(|n| n.node)
            .collect::<Vec<_>>()
            .into_boxed_slice(),
        child_offsets: offsets.into_boxed_slice(),
        children: children.into_boxed_slice(),
        root: 0,
    })
}

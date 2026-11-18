use std::{cmp::Reverse, ops::Range, sync::OnceLock};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub type FrameId = u32;
pub type StackId = u32;
pub type NodeId = u32;

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(deny_unknown_fields)]
pub struct ContextRef {
    pub node_id: NodeId,
    pub offset: u32,
}

impl ContextRef {
    pub const ROOT: Self = Self {
        node_id: 0,
        offset: 0,
    };
}

#[derive(Debug)]
pub struct FrameTable {
    pub(crate) name_offsets: Box<[u32]>,
    pub(crate) name_text: Box<str>,
}

impl FrameTable {
    pub fn len(&self) -> usize {
        self.name_offsets.len() - 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn name(&self, id: FrameId) -> &str {
        let index = id as usize;
        &self.name_text[self.name_offsets[index] as usize..self.name_offsets[index + 1] as usize]
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &str> {
        self.name_offsets
            .windows(2)
            .map(|range| &self.name_text[range[0] as usize..range[1] as usize])
    }
}

#[derive(Clone, Copy, Debug)]
pub struct StackRecord<'a> {
    pub frames: &'a [FrameId],
    pub weight: u64,
}

#[derive(Debug)]
pub struct StackTable {
    pub(crate) offsets: Box<[u32]>,
    pub(crate) frames: Box<[FrameId]>,
    pub(crate) weights: Box<[u64]>,
}

impl StackTable {
    pub fn len(&self) -> usize {
        self.weights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.weights.is_empty()
    }

    pub fn stack(&self, id: StackId) -> StackRecord<'_> {
        let index = id as usize;
        StackRecord {
            frames: &self.frames[self.offsets[index] as usize..self.offsets[index + 1] as usize],
            weight: self.weights[index],
        }
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = StackRecord<'_>> {
        self.offsets
            .windows(2)
            .zip(self.weights.iter())
            .map(|(range, &weight)| StackRecord {
                frames: &self.frames[range[0] as usize..range[1] as usize],
                weight,
            })
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ScopeDescriptor {
    pub(crate) start: u32,
    pub(crate) count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeKind {
    All,
    One,
    General,
}

#[derive(Debug, Default)]
pub struct PostingTable {
    pub(crate) scopes: Box<[ScopeDescriptor]>,
    pub(crate) ranked_stacks: Box<[StackId]>,
    pub(crate) entries: Box<[StackId]>,
    pub(crate) position_offsets: Box<[u32]>,
    pub(crate) constant_positions: Box<[u32]>,
    pub(crate) variable_positions: Box<[u32]>,
}

impl PostingTable {
    pub fn constant_positions_pair(&self, frame: usize) -> Option<(u16, u16)> {
        if self.position_offsets[frame] != self.position_offsets[frame + 1] {
            return None;
        }
        let packed = self.constant_positions[frame];
        Some((packed as u16, (packed >> 16) as u16))
    }
    pub fn positions(&self, frame: usize, posting_index: usize) -> (u16, u16) {
        let start = self.position_offsets[frame];
        let packed = if start == self.position_offsets[frame + 1] {
            self.constant_positions[frame]
        } else {
            self.variable_positions[start as usize + posting_index]
        };
        (packed as u16, (packed >> 16) as u16)
    }
    pub fn position_offsets(&self) -> &[u32] {
        &self.position_offsets
    }
    pub fn constant_positions(&self) -> &[u32] {
        &self.constant_positions
    }
    pub fn variable_positions(&self) -> &[u32] {
        &self.variable_positions
    }
    pub fn scope_kind(&self, frame: usize) -> ScopeKind {
        let scope = &self.scopes[frame];
        if scope.count as usize == self.ranked_stacks.len() {
            ScopeKind::All
        } else if scope.count == 1 {
            ScopeKind::One
        } else {
            ScopeKind::General
        }
    }
    pub fn ranked(&self, frame: usize) -> &[StackId] {
        let scope = &self.scopes[frame];
        match self.scope_kind(frame) {
            ScopeKind::All => &self.ranked_stacks,
            ScopeKind::One => std::slice::from_ref(&scope.start),
            ScopeKind::General => {
                &self.entries[scope.start as usize..scope.start as usize + scope.count as usize]
            }
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = &[StackId]> {
        (0..self.scopes.len()).map(|frame| self.ranked(frame))
    }
    pub fn ranked_stacks(&self) -> &[StackId] {
        &self.ranked_stacks
    }
}

#[derive(Clone, Debug, Default)]
pub struct FrameStats {
    pub self_weight: u64,
    pub inclusive_weight: u64,
    pub stack_count: u32,
}

#[derive(Clone, Debug)]
pub struct ContextNode {
    pub(crate) label_offset: u32,
    pub(crate) label_len: u32,
    pub(crate) total_weight: u64,
}

#[derive(Debug)]
pub struct ContextCallTree {
    pub(crate) nodes: Box<[ContextNode]>,
    pub(crate) child_offsets: Box<[u32]>,
    pub(crate) children: Box<[NodeId]>,
    // Indexed by physical node ID minus one. The virtual root spans every stack.
    pub(crate) stack_ranges: Box<[[StackId; 2]]>,
    pub(crate) root: NodeId,
}

impl ContextCallTree {
    pub fn children(&self, node: NodeId) -> &[NodeId] {
        let index = node as usize;
        &self.children[self.child_offsets[index] as usize..self.child_offsets[index + 1] as usize]
    }
}

#[derive(Clone, Debug)]
pub struct SourceMeta {
    pub(crate) fingerprint: String,
    pub(crate) snapshot_id: OnceLock<String>,
}

#[derive(Clone, Debug, Default)]
pub struct RecursionStats {
    pub frame: FrameId,
    pub max_occurrences: u32,
    pub affected_weight: u64,
}

#[derive(Debug)]
pub struct Profile {
    pub(crate) source: SourceMeta,
    pub(crate) total_weight: u64,
    pub(crate) max_depth: usize,
    pub(crate) frames: FrameTable,
    pub(crate) stacks: StackTable,
    pub(crate) frame_stats: Box<[FrameStats]>,
    pub(crate) frame_to_stacks: PostingTable,
    pub(crate) cct: ContextCallTree,
    pub(crate) top_self: Box<[FrameId]>,
    pub(crate) top_inclusive: Box<[FrameId]>,
    pub(crate) recursive_frames: Box<[RecursionStats]>,
    pub(crate) heaviest_stack_weights: [u64; 2],
    // Computed on the first quality query. Shared contexts can overlap, so a
    // union of multiple unknown labels needs a scan of the immutable stacks.
    pub(crate) unknown_coverage: OnceLock<(usize, u64, u64)>,
}

#[derive(Clone, Debug, Default)]
pub struct MemoryBreakdown {
    pub frame_name_bytes: usize,
    pub frame_offsets_bytes: usize,
    pub stack_offsets_bytes: usize,
    pub stack_frames_bytes: usize,
    pub stack_weights_bytes: usize,
    pub frame_stats_bytes: usize,
    pub posting_offsets_bytes: usize,
    pub posting_entries_bytes: usize,
    pub posting_positions_bytes: usize,
    pub cct_nodes_bytes: usize,
    pub cct_offsets_bytes: usize,
    pub cct_edges_bytes: usize,
    pub cct_ranges_bytes: usize,
    pub top_rankings_bytes: usize,
    pub recursion_bytes: usize,
    pub metadata_bytes: usize,
    pub total_bytes: usize,
}

impl Profile {
    /// Conservative retained-allocation estimate, including container spare capacity.
    pub fn estimated_size_bytes(&self) -> usize {
        self.memory_breakdown().total_bytes
    }

    pub fn memory_breakdown(&self) -> MemoryBreakdown {
        let mut result = MemoryBreakdown {
            frame_name_bytes: self.frames.name_text.len(),
            frame_offsets_bytes: std::mem::size_of_val(&*self.frames.name_offsets),
            stack_offsets_bytes: std::mem::size_of_val(&*self.stacks.offsets),
            stack_frames_bytes: std::mem::size_of_val(&*self.stacks.frames),
            stack_weights_bytes: std::mem::size_of_val(&*self.stacks.weights),
            frame_stats_bytes: std::mem::size_of_val(&*self.frame_stats),
            posting_offsets_bytes: std::mem::size_of_val(&*self.frame_to_stacks.scopes),
            posting_entries_bytes: std::mem::size_of_val(&*self.frame_to_stacks.entries)
                + std::mem::size_of_val(&*self.frame_to_stacks.ranked_stacks),
            posting_positions_bytes: std::mem::size_of_val(&*self.frame_to_stacks.position_offsets)
                + std::mem::size_of_val(&*self.frame_to_stacks.constant_positions)
                + std::mem::size_of_val(&*self.frame_to_stacks.variable_positions),
            cct_nodes_bytes: std::mem::size_of_val(&*self.cct.nodes),
            cct_offsets_bytes: std::mem::size_of_val(&*self.cct.child_offsets),
            cct_edges_bytes: std::mem::size_of_val(&*self.cct.children),
            cct_ranges_bytes: std::mem::size_of_val(&*self.cct.stack_ranges),
            top_rankings_bytes: std::mem::size_of_val(&*self.top_self)
                + std::mem::size_of_val(&*self.top_inclusive),
            recursion_bytes: std::mem::size_of_val(&*self.recursive_frames),
            metadata_bytes: std::mem::size_of::<Self>() + self.source.fingerprint.capacity() + 64,
            total_bytes: 0,
        };
        result.total_bytes = [
            result.frame_name_bytes,
            result.frame_offsets_bytes,
            result.stack_offsets_bytes,
            result.stack_frames_bytes,
            result.stack_weights_bytes,
            result.frame_stats_bytes,
            result.posting_offsets_bytes,
            result.posting_entries_bytes,
            result.posting_positions_bytes,
            result.cct_nodes_bytes,
            result.cct_offsets_bytes,
            result.cct_edges_bytes,
            result.cct_ranges_bytes,
            result.top_rankings_bytes,
            result.recursion_bytes,
            result.metadata_bytes,
        ]
        .into_iter()
        .sum();
        result
    }

    pub fn frame_id(&self, name: &str) -> Option<FrameId> {
        let mut low = 0;
        let mut high = self.frames.len();
        while low < high {
            let middle = low + (high - low) / 2;
            match self.frame_name(middle as u32).cmp(name) {
                std::cmp::Ordering::Less => low = middle + 1,
                std::cmp::Ordering::Greater => high = middle,
                std::cmp::Ordering::Equal => return Some(middle as u32),
            }
        }
        None
    }
    pub fn frame_name(&self, id: FrameId) -> &str {
        self.frames.name(id)
    }
    pub fn root(&self) -> &ContextNode {
        &self.cct.nodes[self.cct.root as usize]
    }
}

impl FrameTable {
    pub fn name_offsets(&self) -> &[u32] {
        &self.name_offsets
    }
    pub fn name_text(&self) -> &str {
        &self.name_text
    }
}
impl StackTable {
    pub fn offsets(&self) -> &[u32] {
        &self.offsets
    }
    pub fn frames(&self) -> &[FrameId] {
        &self.frames
    }
    pub fn weights(&self) -> &[u64] {
        &self.weights
    }
}
impl PostingTable {
    pub fn entries(&self) -> &[StackId] {
        &self.entries
    }
}
impl ContextNode {
    pub fn label_offset(&self) -> u32 {
        self.label_offset
    }
    pub fn label_len(&self) -> u32 {
        self.label_len
    }
    pub fn total_weight(&self) -> u64 {
        self.total_weight
    }
}
impl ContextCallTree {
    pub fn nodes(&self) -> &[ContextNode] {
        &self.nodes
    }
    pub fn child_offsets(&self) -> &[u32] {
        &self.child_offsets
    }
    pub fn child_ids(&self) -> &[NodeId] {
        &self.children
    }
    pub fn root_id(&self) -> NodeId {
        self.root
    }
    pub fn self_weight(&self, node: NodeId) -> u64 {
        self.nodes[node as usize].total_weight
            - self
                .children(node)
                .iter()
                .map(|&id| self.nodes[id as usize].total_weight)
                .sum::<u64>()
    }
}
impl Profile {
    pub fn total_weight(&self) -> u64 {
        self.total_weight
    }
    pub fn max_depth(&self) -> usize {
        self.max_depth
    }
    pub fn frames(&self) -> &FrameTable {
        &self.frames
    }
    pub fn stacks(&self) -> &StackTable {
        &self.stacks
    }
    pub fn frame_stats(&self) -> &[FrameStats] {
        &self.frame_stats
    }
    pub fn postings(&self) -> &PostingTable {
        &self.frame_to_stacks
    }
    pub fn cct(&self) -> &ContextCallTree {
        &self.cct
    }
    pub fn top_self(&self) -> &[FrameId] {
        &self.top_self
    }
    pub fn top_inclusive(&self) -> &[FrameId] {
        &self.top_inclusive
    }
    pub fn recursive_frames(&self) -> &[RecursionStats] {
        &self.recursive_frames
    }
    pub fn heaviest_stack_weights(&self) -> [u64; 2] {
        self.heaviest_stack_weights
    }
    pub fn fingerprint(&self) -> &str {
        &self.source.fingerprint
    }

    pub fn snapshot_id(&self) -> &str {
        self.source.snapshot_id.get_or_init(|| {
            let mut hash = blake3::Hasher::new();
            // Bump the version if normalization or snapshot-local IDs change.
            hash.update(b"prof-mcp/folded-v1\0");
            hash.update(&(self.frames.len() as u64).to_le_bytes());
            for name in self.frames.iter() {
                hash.update(&(name.len() as u64).to_le_bytes());
                hash.update(name.as_bytes());
            }
            hash.update(&(self.stacks.len() as u64).to_le_bytes());
            for stack in self.stacks.iter() {
                hash.update(&(stack.frames.len() as u64).to_le_bytes());
                for frame in stack.frames {
                    hash.update(&frame.to_le_bytes());
                }
                hash.update(&stack.weight.to_le_bytes());
            }
            hash.finalize().to_hex().to_string()
        })
    }

    fn context_node(&self, context: ContextRef) -> Option<&ContextNode> {
        let node = self.cct.nodes.get(context.node_id as usize)?;
        (context == ContextRef::ROOT || context.offset < node.label_len).then_some(node)
    }

    pub fn context_range(&self, context: ContextRef) -> Option<Range<StackId>> {
        self.context_node(context)?;
        if context == ContextRef::ROOT {
            return Some(0..self.stacks.len() as StackId);
        }
        let [begin, end] = self.cct.stack_ranges[context.node_id as usize - 1];
        Some(begin..end)
    }

    pub(crate) fn context_weight(&self, context: ContextRef) -> Option<u64> {
        self.context_node(context).map(|node| node.total_weight)
    }

    pub fn context_prefix(&self, context: ContextRef) -> Option<&[FrameId]> {
        if context == ContextRef::ROOT {
            return Some(&[]);
        }
        let node = self.context_node(context)?;
        // The edge label points into the first stack in this subtree.
        let [begin, _] = self.cct.stack_ranges[context.node_id as usize - 1];
        let start = self.stacks.offsets[begin as usize] as usize;
        let end = (node.label_offset + context.offset) as usize + 1;
        Some(&self.stacks.frames[start..end])
    }

    pub fn context_depth(&self, context: ContextRef) -> Option<usize> {
        self.context_prefix(context).map(<[FrameId]>::len)
    }

    pub fn context_frame(&self, context: ContextRef) -> Option<FrameId> {
        self.context_prefix(context)?.last().copied()
    }

    pub fn context_children(
        &self,
        context: ContextRef,
    ) -> Option<impl Iterator<Item = ContextRef> + '_> {
        let node = self.context_node(context)?;
        let next = (context.offset + 1 < node.label_len).then_some(ContextRef {
            node_id: context.node_id,
            offset: context.offset + 1,
        });
        let children = if next.is_some() {
            &[]
        } else {
            self.cct.children(context.node_id)
        };
        Some(
            next.into_iter().chain(
                children
                    .iter()
                    .map(|&node_id| ContextRef { node_id, offset: 0 }),
            ),
        )
    }

    pub fn context_for_prefix(&self, mut prefix: &[FrameId]) -> Option<ContextRef> {
        let mut context = ContextRef::ROOT;
        while let Some(&frame) = prefix.first() {
            let node_id = self.context_child_for_frame(context, frame)?;
            let node = &self.cct.nodes[node_id as usize];
            let start = node.label_offset as usize;
            let label = &self.stacks.frames[start..start + node.label_len as usize];
            if label.starts_with(prefix) {
                return Some(ContextRef {
                    node_id,
                    offset: prefix.len() as u32 - 1,
                });
            }
            prefix = prefix.strip_prefix(label)?;
            context = ContextRef {
                node_id,
                offset: node.label_len - 1,
            };
        }
        Some(context)
    }

    fn context_child_for_frame(&self, parent: ContextRef, frame: FrameId) -> Option<NodeId> {
        const LEADING_CHILDREN: usize = 8;
        const MIN_WIDE_TAIL: usize = 64;

        let children = self.cct.children(parent.node_id);
        let first_frame = |id: NodeId| {
            let node = &self.cct.nodes[id as usize];
            self.stacks.frames[node.label_offset as usize]
        };
        let find = |ids: &[NodeId]| ids.iter().copied().find(|&id| first_frame(id) == frame);
        if children.len() < LEADING_CHILDREN + MIN_WIDE_TAIL {
            return find(children);
        }
        let (leading, tail) = children.split_at(LEADING_CHILDREN);
        if let Some(child) = find(leading) {
            return Some(child);
        }

        let depth = self.context_depth(parent)?;
        let range = self.context_range(parent)?;
        let offsets = self.stacks.offsets();
        let mut begin = range.start as usize;
        // The only stack ending at the parent sorts first and has no child frame.
        if (offsets[begin + 1] - offsets[begin]) as usize == depth {
            begin += 1;
        }
        let offsets = &offsets[begin..range.end as usize];
        let frame_at_depth = |&offset: &u32| self.stacks.frames[offset as usize + depth];
        let first = offsets.partition_point(|offset| frame_at_depth(offset) < frame);
        if offsets.get(first).map(frame_at_depth) != Some(frame) {
            return None;
        }
        let after = first
            + 1
            + offsets[first + 1..].partition_point(|offset| frame_at_depth(offset) == frame);
        if after - first > tail.len() {
            return find(tail);
        }

        // The lexical slice gives the child's weight for the existing CCT sort key.
        let weight: u64 = self.stacks.weights()[begin + first..begin + after]
            .iter()
            .sum();
        let index = tail
            .binary_search_by_key(&(Reverse(weight), frame), |&id| {
                (
                    Reverse(self.cct.nodes[id as usize].total_weight),
                    first_frame(id),
                )
            })
            .ok()?;
        Some(tail[index])
    }
}

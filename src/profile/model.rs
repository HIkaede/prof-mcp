pub type FrameId = u32;
pub type StackId = u32;
pub type NodeId = u32;

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

#[derive(Debug, Default)]
pub struct PostingTable {
    pub(crate) offsets: Box<[u32]>,
    pub(crate) entries: Box<[StackId]>,
    pub(crate) position_offsets: Box<[u32]>,
    pub(crate) constant_positions: Box<[u32]>,
    pub(crate) variable_positions: Box<[u32]>,
}

impl PostingTable {
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
    pub fn iter(&self) -> impl Iterator<Item = &[StackId]> {
        self.offsets
            .windows(2)
            .map(|range| &self.entries[range[0] as usize..range[1] as usize])
    }
}

impl std::ops::Index<usize> for PostingTable {
    type Output = [StackId];

    fn index(&self, frame: usize) -> &Self::Output {
        &self.entries[self.offsets[frame] as usize..self.offsets[frame + 1] as usize]
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
    pub(crate) frame: Option<FrameId>,
    pub(crate) total_weight: u64,
}

#[derive(Debug)]
pub struct ContextCallTree {
    pub(crate) nodes: Box<[ContextNode]>,
    pub(crate) child_offsets: Box<[u32]>,
    pub(crate) children: Box<[NodeId]>,
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
            posting_offsets_bytes: std::mem::size_of_val(&*self.frame_to_stacks.offsets),
            posting_entries_bytes: std::mem::size_of_val(&*self.frame_to_stacks.entries),
            posting_positions_bytes: std::mem::size_of_val(&*self.frame_to_stacks.position_offsets)
                + std::mem::size_of_val(&*self.frame_to_stacks.constant_positions)
                + std::mem::size_of_val(&*self.frame_to_stacks.variable_positions),
            cct_nodes_bytes: std::mem::size_of_val(&*self.cct.nodes),
            cct_offsets_bytes: std::mem::size_of_val(&*self.cct.child_offsets),
            cct_edges_bytes: std::mem::size_of_val(&*self.cct.children),
            top_rankings_bytes: std::mem::size_of_val(&*self.top_self)
                + std::mem::size_of_val(&*self.top_inclusive),
            recursion_bytes: std::mem::size_of_val(&*self.recursive_frames),
            metadata_bytes: std::mem::size_of::<Self>() + self.source.fingerprint.capacity(),
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
    pub fn offsets(&self) -> &[u32] {
        &self.offsets
    }
    pub fn entries(&self) -> &[StackId] {
        &self.entries
    }
}
impl ContextNode {
    pub fn frame(&self) -> Option<FrameId> {
        self.frame
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
}

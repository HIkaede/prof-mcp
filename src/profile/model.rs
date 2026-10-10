use std::path::PathBuf;

pub type FrameId = u32;
pub type StackId = u32;
pub type NodeId = u32;

#[derive(Clone, Debug)]
pub struct FrameTable {
    pub name_offsets: Box<[u32]>,
    pub name_text: Box<str>,
    pub name_order: Box<[FrameId]>,
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

#[derive(Clone, Debug)]
pub struct StackTable {
    pub offsets: Box<[u32]>,
    pub frames: Box<[FrameId]>,
    pub weights: Box<[u64]>,
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

#[derive(Clone, Debug, Default)]
pub struct PostingTable {
    pub offsets: Box<[u32]>,
    pub stack_ids: Box<[StackId]>,
}

impl PostingTable {
    pub fn iter(&self) -> impl Iterator<Item = &[StackId]> {
        self.offsets
            .windows(2)
            .map(|range| &self.stack_ids[range[0] as usize..range[1] as usize])
    }
}

impl std::ops::Index<usize> for PostingTable {
    type Output = [StackId];

    fn index(&self, frame: usize) -> &Self::Output {
        &self.stack_ids[self.offsets[frame] as usize..self.offsets[frame + 1] as usize]
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
    pub frame: Option<FrameId>,
    pub self_weight: u64,
    pub total_weight: u64,
}

#[derive(Clone, Debug)]
pub struct ContextCallTree {
    pub nodes: Box<[ContextNode]>,
    pub child_offsets: Box<[u32]>,
    pub children: Box<[NodeId]>,
    pub root: NodeId,
}

impl ContextCallTree {
    pub fn children(&self, node: NodeId) -> &[NodeId] {
        let index = node as usize;
        &self.children[self.child_offsets[index] as usize..self.child_offsets[index + 1] as usize]
    }
}

#[derive(Clone, Debug)]
pub struct SourceMeta {
    pub canonical_path: PathBuf,
    pub fingerprint: String,
    pub byte_len: u64,
    pub modified_unix_ms: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct RecursionStats {
    pub frame: FrameId,
    pub max_occurrences: u32,
    pub affected_weight: u64,
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub source: SourceMeta,
    pub total_weight: u64,
    pub max_depth: usize,
    pub frames: FrameTable,
    pub stacks: StackTable,
    pub frame_stats: Vec<FrameStats>,
    pub frame_to_stacks: PostingTable,
    pub cct: ContextCallTree,
    pub top_self: Box<[FrameId]>,
    pub top_inclusive: Box<[FrameId]>,
    pub recursive_frames: Box<[RecursionStats]>,
    pub heaviest_stack_weights: [u64; 2],
}

impl Profile {
    /// Conservative retained-allocation estimate, including container spare capacity.
    pub fn estimated_size_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.source.canonical_path.as_os_str().len()
            + self.source.fingerprint.capacity()
            + std::mem::size_of_val(&*self.frames.name_offsets)
            + self.frames.name_text.len()
            + std::mem::size_of_val(&*self.frames.name_order)
            + std::mem::size_of_val(&*self.stacks.offsets)
            + std::mem::size_of_val(&*self.stacks.frames)
            + std::mem::size_of_val(&*self.stacks.weights)
            + self.frame_stats.capacity() * std::mem::size_of::<FrameStats>()
            + std::mem::size_of_val(&*self.frame_to_stacks.offsets)
            + std::mem::size_of_val(&*self.frame_to_stacks.stack_ids)
            + std::mem::size_of_val(&*self.cct.nodes)
            + std::mem::size_of_val(&*self.cct.child_offsets)
            + std::mem::size_of_val(&*self.cct.children)
            + std::mem::size_of_val(&*self.top_self)
            + std::mem::size_of_val(&*self.top_inclusive)
            + std::mem::size_of_val(&*self.recursive_frames)
    }

    pub fn frame_id(&self, name: &str) -> Option<FrameId> {
        self.frames
            .name_order
            .binary_search_by(|id| self.frame_name(*id).cmp(name))
            .ok()
            .map(|index| self.frames.name_order[index])
    }
    pub fn frame_name(&self, id: FrameId) -> &str {
        self.frames.name(id)
    }
    pub fn root(&self) -> &ContextNode {
        &self.cct.nodes[self.cct.root as usize]
    }
}

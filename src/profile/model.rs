use std::path::PathBuf;

use hashbrown::HashMap;

pub type FrameId = u32;
pub type StackId = u32;
pub type NodeId = u32;

#[derive(Clone, Debug)]
pub struct Frame {
    pub name: Box<str>,
}

#[derive(Clone, Debug)]
pub struct StackRecord {
    pub frames: Box<[FrameId]>,
    pub weight: u64,
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
    pub parent: Option<NodeId>,
    pub self_weight: u64,
    pub total_weight: u64,
    pub children: HashMap<FrameId, NodeId>,
}

#[derive(Clone, Debug)]
pub struct ContextCallTree {
    pub nodes: Vec<ContextNode>,
    pub root: NodeId,
}

#[derive(Clone, Debug)]
pub struct SourceMeta {
    pub canonical_path: PathBuf,
    pub fingerprint: String,
    pub byte_len: u64,
    pub modified_unix_ms: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub source: SourceMeta,
    pub total_weight: u64,
    pub max_depth: usize,
    pub frames: Vec<Frame>,
    pub frame_by_name: HashMap<Box<str>, FrameId>,
    pub stacks: Vec<StackRecord>,
    pub frame_stats: Vec<FrameStats>,
    pub frame_to_stacks: Vec<Vec<StackId>>,
    pub cct: ContextCallTree,
}

impl Profile {
    /// Conservative retained-allocation estimate, including container spare capacity.
    pub fn estimated_size_bytes(&self) -> usize {
        fn map_bytes<K, V>(map: &HashMap<K, V>) -> usize {
            map.capacity() * 2 * (std::mem::size_of::<(K, V)>() + 1)
        }
        std::mem::size_of::<Self>()
            + self.source.canonical_path.as_os_str().len()
            + self.source.fingerprint.capacity()
            + self.frames.capacity() * std::mem::size_of::<Frame>()
            + self
                .frames
                .iter()
                .map(|frame| frame.name.len())
                .sum::<usize>()
            + map_bytes(&self.frame_by_name)
            + self
                .frame_by_name
                .keys()
                .map(|name| name.len())
                .sum::<usize>()
            + self.stacks.capacity() * std::mem::size_of::<StackRecord>()
            + self
                .stacks
                .iter()
                .map(|stack| std::mem::size_of_val(&*stack.frames))
                .sum::<usize>()
            + self.frame_stats.capacity() * std::mem::size_of::<FrameStats>()
            + self.frame_to_stacks.capacity() * std::mem::size_of::<Vec<StackId>>()
            + self
                .frame_to_stacks
                .iter()
                .map(|ids| ids.capacity() * std::mem::size_of::<StackId>())
                .sum::<usize>()
            + self.cct.nodes.capacity() * std::mem::size_of::<ContextNode>()
            + self
                .cct
                .nodes
                .iter()
                .map(|node| map_bytes(&node.children))
                .sum::<usize>()
    }

    pub fn frame_id(&self, name: &str) -> Option<FrameId> {
        self.frame_by_name.get(name).copied()
    }
    pub fn frame_name(&self, id: FrameId) -> &str {
        &self.frames[id as usize].name
    }
    pub fn root(&self) -> &ContextNode {
        &self.cct.nodes[self.cct.root as usize]
    }
}

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

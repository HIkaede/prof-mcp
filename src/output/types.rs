use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::query::{DiffSort, MatchMode, TopSort};

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct WeightSemantics {
    pub unit: &'static str,
    pub basis: &'static str,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct OutputProfile<'a> {
    pub fingerprint: &'a str,
    pub alias: &'a str,
    pub weight_semantics: WeightSemantics,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TruncationReason {
    RowLimit {
        limit: usize,
        returned: usize,
        available: usize,
        omitted: usize,
    },
    DepthLimit {
        max_depth: usize,
        omitted_children: usize,
        omitted_weight: u64,
    },
    NodeBudget {
        max_nodes: usize,
        omitted_children: usize,
        omitted_weight: u64,
    },
    MinScopePercent {
        threshold: f64,
        omitted_children: usize,
        omitted_weight: u64,
    },
    FrameWindow {
        mode: FrameWindowMode,
        cropped_paths: usize,
        total_omitted_before: usize,
        total_omitted_after: usize,
    },
    TotalFrameBudget(TotalFrameBudget),
    RegistryProfileLimit {
        limit: usize,
        returned: usize,
        available: usize,
        omitted: usize,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameWindowMode {
    Head,
    Tail,
    AroundTarget,
}

// Keep the previous alphabetical kind order in grouped truncation reasons.
#[derive(
    Clone, Copy, Debug, Deserialize, JsonSchema, Serialize, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum TreeTruncation {
    DepthLimit,
    MinScopePercent,
    NodeBudget,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct OutputEnvelope<'a, T> {
    pub schema_version: &'static str,
    pub profile: OutputProfile<'a>,
    pub scope_weight: u64,
    pub truncated: bool,
    pub truncation_reasons: Vec<TruncationReason>,
    pub warnings: Vec<&'static str>,
    pub data: T,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct SummaryData<'a> {
    pub total_weight: u64,
    pub frame_count: usize,
    pub unique_stack_count: usize,
    pub max_depth: usize,
    pub stack_concentration: StackConcentration,
    pub recursion_detected: Vec<RecursionRow<'a>>,
    pub top_self: Vec<crate::output::FrameRow<'a>>,
    pub top_inclusive: Vec<crate::output::FrameRow<'a>>,
    pub registry: RegistryOutput,
}
pub type SummaryOutput<'a> = OutputEnvelope<'a, SummaryData<'a>>;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct StackConcentration {
    pub top_10_stacks_percent: f64,
    pub top_50_stacks_percent: f64,
    pub unique_stack_count: usize,
}
#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct RecursionRow<'a> {
    pub frame_id: u32,
    pub name: &'a str,
    pub max_occurrences_per_stack: usize,
    pub affected_weight: u64,
    pub affected_weight_percent: f64,
}
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct RegistryOutput {
    pub registry_root: String,
    pub active: String,
    pub profile_count: usize,
    pub profiles: Vec<crate::registry::RegistryProfile>,
}
#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct FindData<'a> {
    pub query: &'a str,
    pub mode: MatchMode,
    pub matches: Vec<crate::output::FrameRow<'a>>,
}
pub type FindOutput<'a> = OutputEnvelope<'a, FindData<'a>>;

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct TopData<'a> {
    pub metric: TopSort,
    pub frame: Option<u32>,
    pub rows: Vec<crate::output::FrameRow<'a>>,
}

pub type TopOutput<'a> = OutputEnvelope<'a, TopData<'a>>;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct TotalFrameBudget {
    pub limit: usize,
    pub available: usize,
    pub returned: usize,
    pub omitted: usize,
    pub selected_paths: usize,
    pub returned_paths: usize,
    pub omitted_paths: usize,
    pub cropped_paths: usize,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct PathRow<'a> {
    pub frames: Vec<&'a str>,
    pub weight: u64,
    pub profile_percent: f64,
    pub scope_percent: f64,
    pub target_positions: Vec<usize>,
    pub display_target_positions: Vec<usize>,
    pub total_depth: usize,
    #[serde(default)]
    pub requested_frame_start: usize,
    #[serde(default)]
    pub requested_frame_end: usize,
    pub frame_start: usize,
    pub frame_end: usize,
    #[serde(default)]
    pub omitted_before: usize,
    #[serde(default)]
    pub omitted_after: usize,
    #[serde(default)]
    pub budget_omitted_before: usize,
    #[serde(default)]
    pub budget_omitted_after: usize,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct PathsData<'a> {
    pub frame: u32,
    pub paths: Vec<PathRow<'a>>,
    pub total_frame_budget: TotalFrameBudget,
}
pub type PathsOutput<'a> = OutputEnvelope<'a, PathsData<'a>>;

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct TreeNode<'a> {
    pub node_id: Option<[u32; 2]>,
    pub frame_id: Option<u32>,
    pub name: Option<&'a str>,
    pub self_weight: u64,
    pub total_weight: u64,
    pub profile_percent: f64,
    pub scope_percent: f64,
    pub omitted_children: usize,
    pub omitted_weight: u64,
    pub children: Vec<TreeNode<'a>>,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct TreeContinuation<'a> {
    pub node_id: [u32; 2],
    pub frame_id: u32,
    pub name: &'a str,
    pub reason: TreeTruncation,
    pub continuation: Continuation,
    pub total_weight: u64,
    pub profile_percent: f64,
    pub scope_percent: f64,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct TreeData<'a> {
    pub root: TreeNode<'a>,
    pub continuations: Vec<TreeContinuation<'a>>,
    pub continuations_truncated: bool,
    pub continuation_limit: usize,
    pub continuations_available: usize,
    pub continuations_omitted: usize,
}
pub type TreeOutput<'a> = OutputEnvelope<'a, TreeData<'a>>;

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct DirectionData<'a> {
    pub frame: crate::output::FrameRow<'a>,
    pub root: TreeNode<'a>,
    #[serde(default)]
    pub continuations: Vec<DirectionContinuation<'a>>,
    #[serde(default)]
    pub continuations_truncated: bool,
    #[serde(default)]
    pub continuation_limit: usize,
    #[serde(default)]
    pub continuations_available: usize,
    #[serde(default)]
    pub continuations_omitted: usize,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct DirectionContinuation<'a> {
    pub frame_id: u32,
    pub name: &'a str,
    pub reason: TreeTruncation,
    pub continuation: Continuation,
    pub total_weight: u64,
    pub profile_percent: f64,
    pub scope_percent: f64,
}
pub type DirectionOutput<'a> = OutputEnvelope<'a, DirectionData<'a>>;

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct DiffRow<'a> {
    pub name: &'a str,
    pub baseline_weight: u64,
    pub candidate_weight: u64,
    pub baseline_percent: f64,
    pub candidate_percent: f64,
    pub delta_pp: f64,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct DiffData<'a> {
    pub metric: TopSort,
    pub sort: DiffSort,
    pub rows: Vec<DiffRow<'a>>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct DiffScopeWeight {
    pub baseline: u64,
    pub candidate: u64,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct DiffOutput<'a> {
    pub schema_version: &'static str,
    pub baseline: OutputProfile<'a>,
    pub candidate: OutputProfile<'a>,
    pub scope_weight: DiffScopeWeight,
    pub truncated: bool,
    pub truncation_reasons: Vec<TruncationReason>,
    pub warnings: Vec<&'static str>,
    pub data: DiffData<'a>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Continuation {
    pub profile: String,
    pub fingerprint: String,
    pub tool: ContinuationTool,
    #[schemars(length(min = 2, max = 4097))]
    pub cursor: Vec<u32>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub enum ContinuationTool {
    #[serde(rename = "profile_tree")]
    Tree,
    #[serde(rename = "profile_callers")]
    Callers,
    #[serde(rename = "profile_callees")]
    Callees,
}

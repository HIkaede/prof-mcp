use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::query::{DiffSort, MatchMode, TopSort};

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct WeightSemantics {
    pub(crate) unit: String,
    pub(crate) basis: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct OutputProfile {
    pub(crate) fingerprint: String,
    pub(crate) alias: String,
    pub(crate) weight_semantics: WeightSemantics,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum TruncationReason {
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
pub(crate) enum FrameWindowMode {
    Head,
    Tail,
    AroundTarget,
}

// Keep the previous alphabetical kind order in grouped truncation reasons.
#[derive(
    Clone, Copy, Debug, Deserialize, JsonSchema, Serialize, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TreeTruncation {
    DepthLimit,
    MinScopePercent,
    NodeBudget,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct OutputEnvelope<T> {
    pub(crate) schema_version: String,
    pub(crate) profile: OutputProfile,
    pub(crate) scope_weight: u64,
    pub(crate) truncated: bool,
    pub(crate) truncation_reasons: Vec<TruncationReason>,
    pub(crate) warnings: Vec<String>,
    pub(crate) data: T,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct SummaryData {
    pub(crate) total_weight: u64,
    pub(crate) frame_count: usize,
    pub(crate) unique_stack_count: usize,
    pub(crate) max_depth: usize,
    pub(crate) stack_concentration: StackConcentration,
    pub(crate) recursion_detected: Vec<RecursionRow>,
    pub(crate) top_self: Vec<crate::output::FrameRow>,
    pub(crate) top_inclusive: Vec<crate::output::FrameRow>,
    pub(crate) registry: RegistryOutput,
}
pub(crate) type SummaryOutput = OutputEnvelope<SummaryData>;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct StackConcentration {
    pub(crate) top_10_stacks_percent: f64,
    pub(crate) top_50_stacks_percent: f64,
    pub(crate) unique_stack_count: usize,
}
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct RecursionRow {
    pub(crate) frame_id: u32,
    pub(crate) name: String,
    pub(crate) max_occurrences_per_stack: usize,
    pub(crate) affected_weight: u64,
    pub(crate) affected_weight_percent: f64,
}
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct RegistryOutput {
    pub(crate) registry_root: String,
    pub(crate) active: String,
    pub(crate) profile_count: usize,
    pub(crate) profiles: Vec<crate::registry::RegistryProfile>,
}
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct FindData {
    pub(crate) query: String,
    pub(crate) mode: MatchMode,
    pub(crate) matches: Vec<crate::output::FrameRow>,
}
pub(crate) type FindOutput = OutputEnvelope<FindData>;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct TopData {
    pub(crate) metric: TopSort,
    pub(crate) frame: Option<u32>,
    pub(crate) rows: Vec<crate::output::FrameRow>,
}

pub(crate) type TopOutput = OutputEnvelope<TopData>;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct TotalFrameBudget {
    pub(crate) limit: usize,
    pub(crate) available: usize,
    pub(crate) returned: usize,
    pub(crate) omitted: usize,
    pub(crate) selected_paths: usize,
    pub(crate) returned_paths: usize,
    pub(crate) omitted_paths: usize,
    pub(crate) cropped_paths: usize,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct PathRow {
    pub(crate) frames: Vec<String>,
    pub(crate) weight: u64,
    pub(crate) profile_percent: f64,
    pub(crate) scope_percent: f64,
    pub(crate) target_positions: Vec<usize>,
    pub(crate) display_target_positions: Vec<usize>,
    pub(crate) total_depth: usize,
    #[serde(default)]
    pub(crate) requested_frame_start: usize,
    #[serde(default)]
    pub(crate) requested_frame_end: usize,
    pub(crate) frame_start: usize,
    pub(crate) frame_end: usize,
    #[serde(default)]
    pub(crate) omitted_before: usize,
    #[serde(default)]
    pub(crate) omitted_after: usize,
    #[serde(default)]
    pub(crate) budget_omitted_before: usize,
    #[serde(default)]
    pub(crate) budget_omitted_after: usize,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct PathsData {
    pub(crate) frame: u32,
    pub(crate) paths: Vec<PathRow>,
    pub(crate) total_frame_budget: TotalFrameBudget,
}
pub(crate) type PathsOutput = OutputEnvelope<PathsData>;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct TreeNode {
    pub(crate) node_id: Option<u32>,
    pub(crate) frame_id: Option<u32>,
    pub(crate) name: Option<String>,
    pub(crate) self_weight: u64,
    pub(crate) total_weight: u64,
    pub(crate) profile_percent: f64,
    pub(crate) scope_percent: f64,
    pub(crate) omitted_children: usize,
    pub(crate) omitted_weight: u64,
    pub(crate) children: Vec<TreeNode>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct TreeContinuation {
    pub(crate) node_id: u32,
    pub(crate) frame_id: u32,
    pub(crate) name: String,
    pub(crate) reason: TreeTruncation,
    pub(crate) continuation: Continuation,
    pub(crate) total_weight: u64,
    pub(crate) profile_percent: f64,
    pub(crate) scope_percent: f64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct TreeData {
    pub(crate) root: TreeNode,
    pub(crate) continuations: Vec<TreeContinuation>,
    pub(crate) continuations_truncated: bool,
    pub(crate) continuation_limit: usize,
    pub(crate) continuations_available: usize,
    pub(crate) continuations_omitted: usize,
}
pub(crate) type TreeOutput = OutputEnvelope<TreeData>;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct DirectionData {
    pub(crate) frame: crate::output::FrameRow,
    pub(crate) root: TreeNode,
    #[serde(default)]
    pub(crate) continuations: Vec<DirectionContinuation>,
    #[serde(default)]
    pub(crate) continuations_truncated: bool,
    #[serde(default)]
    pub(crate) continuation_limit: usize,
    #[serde(default)]
    pub(crate) continuations_available: usize,
    #[serde(default)]
    pub(crate) continuations_omitted: usize,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct DirectionContinuation {
    pub(crate) frame_id: u32,
    pub(crate) name: String,
    pub(crate) reason: TreeTruncation,
    pub(crate) continuation: Continuation,
    pub(crate) total_weight: u64,
    pub(crate) profile_percent: f64,
    pub(crate) scope_percent: f64,
}
pub(crate) type DirectionOutput = OutputEnvelope<DirectionData>;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct DiffRow {
    pub(crate) name: String,
    pub(crate) baseline_weight: u64,
    pub(crate) candidate_weight: u64,
    pub(crate) baseline_percent: f64,
    pub(crate) candidate_percent: f64,
    pub(crate) delta_pp: f64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct DiffData {
    pub(crate) metric: TopSort,
    pub(crate) sort: DiffSort,
    pub(crate) rows: Vec<DiffRow>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct DiffScopeWeight {
    pub(crate) baseline: u64,
    pub(crate) candidate: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct DiffOutput {
    pub(crate) schema_version: String,
    pub(crate) baseline: OutputProfile,
    pub(crate) candidate: OutputProfile,
    pub(crate) scope_weight: DiffScopeWeight,
    pub(crate) truncated: bool,
    pub(crate) truncation_reasons: Vec<TruncationReason>,
    pub(crate) warnings: Vec<String>,
    pub(crate) data: DiffData,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct Continuation {
    pub profile: String,
    pub fingerprint: String,
    pub tool: ContinuationTool,
    #[schemars(length(min = 1, max = 4097))]
    pub cursor: Vec<u32>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
pub(crate) enum ContinuationTool {
    #[serde(rename = "profile_tree")]
    Tree,
    #[serde(rename = "profile_callers")]
    Callers,
    #[serde(rename = "profile_callees")]
    Callees,
}

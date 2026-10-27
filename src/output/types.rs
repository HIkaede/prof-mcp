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
    pub symbol_coverage: SymbolCoverage,
    pub recursion_detected: Vec<RecursionRow<'a>>,
    pub top_self: Vec<crate::output::FrameRow<'a>>,
    pub top_inclusive: Vec<crate::output::FrameRow<'a>>,
    pub registry: RegistryOutput,
}
pub type SummaryOutput<'a> = OutputEnvelope<'a, SummaryData<'a>>;

/// How much of the profile names a location instead of a symbol. perf prints
/// `[unknown@0x...]` when it cannot resolve a PC, and those names cannot be
/// attributed to a function.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct SymbolCoverage {
    pub unresolved_frames: usize,
    pub frame_count: usize,
    pub unresolved_self_percent: f64,
}

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
    pub matches: crate::output::FrameTable,
}
pub type FindOutput<'a> = OutputEnvelope<'a, FindData<'a>>;

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct TopData {
    pub metric: TopSort,
    pub frame: Option<u32>,
    pub rows: crate::output::FrameTable,
}

pub type TopOutput<'a> = OutputEnvelope<'a, TopData>;

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
    /// Zero weights and an empty omission count are left out: absent means 0.
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub self_weight: u64,
    pub total_weight: u64,
    pub profile_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_percent: Option<f64>,
    #[serde(skip_serializing_if = "is_zero_usize")]
    pub omitted_children: usize,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub omitted_weight: u64,
    pub children: Vec<TreeNode<'a>>,
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

fn is_zero_usize(value: &usize) -> bool {
    *value == 0
}

/// A `profile_tree` result is either the nested nodes or the rendered lines.
#[derive(Clone, Debug, JsonSchema, Serialize)]
#[serde(untagged)]
pub enum TreeBody<'a> {
    Nodes(TreeNode<'a>),
    Text(String),
}

impl TreeBody<'_> {
    /// The nested nodes. Text bodies are the same tree, so callers that ask
    /// for `format=text` get a string here instead; this is for the default.
    pub fn nodes(&self) -> &TreeNode<'_> {
        match self {
            Self::Nodes(node) => node,
            Self::Text(_) => panic!("tree body was rendered as text"),
        }
    }

    /// Replace the nested nodes with their rendered lines.
    pub(crate) fn into_text(self) -> Self {
        match self {
            Self::Nodes(node) => Self::Text(crate::output::text_tree(&node)),
            Self::Text(text) => Self::Text(text),
        }
    }

    fn root_name(&self) -> &str {
        match self {
            Self::Nodes(node) => node.name.unwrap_or("-"),
            Self::Text(text) => text.lines().next().unwrap_or("-"),
        }
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_percent: Option<f64>,
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct TreeData<'a> {
    pub root: TreeBody<'a>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_percent: Option<f64>,
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

impl TruncationReason {
    /// The `kind` tag serde emits for this variant.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::RowLimit { .. } => "row_limit",
            Self::DepthLimit { .. } => "depth_limit",
            Self::NodeBudget { .. } => "node_budget",
            Self::MinScopePercent { .. } => "min_scope_percent",
            Self::FrameWindow { .. } => "frame_window",
            Self::TotalFrameBudget(_) => "total_frame_budget",
            Self::RegistryProfileLimit { .. } => "registry_profile_limit",
        }
    }
}

/// One-line text for clients that read `content` and ignore `structured_content`.
///
/// Implemented on the typed outputs so the text cannot drift from the payload:
/// the compiler checks every field it reads.
pub(crate) trait FallbackText {
    fn fallback(&self) -> String;
}

fn fallback_line(
    alias: &str,
    truncated: bool,
    reasons: &[TruncationReason],
    detail: &str,
) -> String {
    let reasons = reasons
        .iter()
        .map(TruncationReason::kind)
        .collect::<Vec<_>>()
        .join(",");
    format!("profile={alias}; truncated={truncated}; reasons=[{reasons}]; {detail}")
}

impl FallbackText for SummaryOutput<'_> {
    fn fallback(&self) -> String {
        let top = self
            .data
            .top_self
            .iter()
            .take(3)
            .map(|row| format!("{}:{}", row.name, row.self_weight))
            .collect::<Vec<_>>()
            .join(", ");
        fallback_line(
            self.profile.alias,
            self.truncated,
            &self.truncation_reasons,
            &format!(
                "total_weight={}, frames={}, stacks={}, top_self=[{top}]",
                self.data.total_weight, self.data.frame_count, self.data.unique_stack_count
            ),
        )
    }
}

impl FallbackText for FindOutput<'_> {
    fn fallback(&self) -> String {
        let names = self.data.matches.names(5);
        fallback_line(
            self.profile.alias,
            self.truncated,
            &self.truncation_reasons,
            &format!("matches={} [{names}]", self.data.matches.len()),
        )
    }
}

impl FallbackText for TopOutput<'_> {
    fn fallback(&self) -> String {
        let names = self.data.rows.names(5);
        fallback_line(
            self.profile.alias,
            self.truncated,
            &self.truncation_reasons,
            &format!("rows={} [{names}]", self.data.rows.len()),
        )
    }
}

impl FallbackText for PathsOutput<'_> {
    fn fallback(&self) -> String {
        let window = self.data.paths.first().map_or_else(
            || "no paths".to_owned(),
            |path| {
                let display = path
                    .display_target_positions
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                format!(
                    "first_window={}..{}/{} target_display=[{display}]",
                    path.frame_start, path.frame_end, path.total_depth
                )
            },
        );
        fallback_line(
            self.profile.alias,
            self.truncated,
            &self.truncation_reasons,
            &format!("paths={} {window}", self.data.paths.len()),
        )
    }
}

impl FallbackText for TreeOutput<'_> {
    fn fallback(&self) -> String {
        fallback_line(
            self.profile.alias,
            self.truncated,
            &self.truncation_reasons,
            &format!(
                "tree_root={}, continuations={}",
                self.data.root.root_name(),
                self.data.continuations.len()
            ),
        )
    }
}

impl FallbackText for DirectionOutput<'_> {
    fn fallback(&self) -> String {
        fallback_line(
            self.profile.alias,
            self.truncated,
            &self.truncation_reasons,
            &format!(
                "frame={}, scope_weight={}, continuations={}",
                self.data.frame.name,
                self.scope_weight,
                self.data.continuations.len()
            ),
        )
    }
}

impl FallbackText for DiffOutput<'_> {
    fn fallback(&self) -> String {
        let changes = self
            .data
            .rows
            .iter()
            .take(5)
            .map(|row| format!("{}:{:+.2}pp", row.name, row.delta_pp))
            .collect::<Vec<_>>()
            .join(", ");
        fallback_line(
            self.candidate.alias,
            self.truncated,
            &self.truncation_reasons,
            &format!("rows={} changes=[{changes}]", self.data.rows.len()),
        )
    }
}

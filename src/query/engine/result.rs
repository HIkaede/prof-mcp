use schemars::JsonSchema;
use serde::Serialize;

use crate::{
    profile::{ContextRef, FrameId, StackId},
    query::{
        TopSort,
        selection::{MetricDefinition, Selection},
    },
};

use super::plan::{Cursor, Occurrence, PathRange, View, ViewKind};

pub const SCHEMA_VERSION: &str = "experimental-1";

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct QueryResult {
    pub schema_version: &'static str,
    pub selection: Selection,
    pub metric: MetricDefinition<'static>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_stream_id: Option<String>,
    pub scope: Scope,
    #[serde(flatten)]
    pub projection: Projection,
}

impl std::ops::Deref for QueryResult {
    type Target = Projection;

    fn deref(&self) -> &Self::Target {
        &self.projection
    }
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct ExploreResult {
    pub schema_version: &'static str,
    pub selection: Selection,
    pub metric: MetricDefinition<'static>,
    pub scope: Scope,
    pub quality: ExploreQuality,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<AnchorStats>,
    pub panels: Vec<Projection>,
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
pub struct ExploreQuality {
    pub stack_completeness: &'static str,
    pub lost_records: Option<u64>,
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
pub struct Scope {
    pub profile_weight: u64,
    pub selected_weight: u64,
    pub stack_shapes: usize,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct Projection {
    pub kind: ViewKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stat: Option<TopSort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_range: Option<PathRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<AnchorStats>,
    pub completeness: Completeness,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub totals: Option<Totals>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<Table>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<Aggregate>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub truncation: Vec<Truncation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_view: Option<View>,
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
pub struct Completeness {
    pub query_exact: bool,
    pub rows_complete: bool,
    pub paths_complete: bool,
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
pub struct AnchorStats {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<FrameId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<ContextRef>,
    pub weight: u64,
    pub self_weight: u64,
    pub scope_percent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurrence: Option<Occurrence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_neighbor_weight: Option<u64>,
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
pub struct Totals {
    pub rows_additive: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_weight: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returned_weight: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub omitted_weight: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filtered_weight: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack_union_weight: Option<u64>,
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
pub struct Aggregate {
    pub self_sum: u64,
    pub stack_union_weight: u64,
    pub fact_count: u64,
    pub stack_shapes: usize,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct Table {
    pub columns: &'static [&'static str],
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Row {
    Frame(FrameId, String, u64, Share, Share),
    Relation(FrameId, String, u64, Share, Share, Share),
    Context(ContextRef, FrameId, String, usize, u64, u64, Share, Share),
    Child(
        ContextRef,
        FrameId,
        String,
        usize,
        u64,
        u64,
        Share,
        Share,
        Share,
    ),
    Path(
        StackId,
        Vec<(FrameId, String)>,
        u64,
        Share,
        Share,
        usize,
        usize,
        usize,
        Option<usize>,
    ),
}

pub(super) type Share = Option<f64>;

pub(super) fn share(weight: u64, denominator: u64) -> Share {
    (denominator > 0).then(|| crate::output::display_percent(weight, denominator))
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Truncation {
    RowLimit {
        limit: usize,
        available: usize,
        returned: usize,
        offset: usize,
    },
    ResponseBytes {
        available: usize,
        returned: usize,
        offset: usize,
    },
    FrameWindow {
        cropped_paths: usize,
        omitted_frames: usize,
    },
}

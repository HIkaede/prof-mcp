//! Tool input contracts, frame selectors, and argument parsing.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::error::ApiError;
use crate::query::{
    DEFAULT_MAX_TOTAL_FRAMES, DiffSort, FrameSelector, FrameWindow, MatchMode, TopSort,
};

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(schema_with = "frame_selector_schema")]
pub(crate) enum FrameSelectorInput {
    ById(FrameIdInput),
    ByName(FrameNameInput),
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrameIdInput {
    pub frame_id: u32,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrameNameInput {
    pub frame_name: String,
}

fn frame_selector_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "oneOf":[
            {
                "type":"object",
                "additionalProperties":false,
                "properties":{"frame_id":{"type":"integer","minimum":0}},
                "required":["frame_id"]
            },
            {
                "type":"object",
                "additionalProperties":false,
                "properties":{"frame_name":{"type":"string"}},
                "required":["frame_name"]
            }
        ]
    })
}
#[allow(dead_code)]
#[derive(JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FindModeSchema {
    Contains,
    Regex,
}
#[allow(dead_code)]
#[derive(JsonSchema, Serialize)]
pub(crate) enum MetricSchema {
    #[serde(rename = "self")]
    SelfWeight,
    #[serde(rename = "inclusive")]
    Inclusive,
}
#[allow(dead_code)]
#[derive(JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DiffSortSchema {
    Regression,
    Improvement,
    Absolute,
}
impl From<FrameSelectorInput> for FrameSelector {
    fn from(value: FrameSelectorInput) -> Self {
        match value {
            FrameSelectorInput::ById(value) => Self {
                frame_id: Some(value.frame_id),
                frame_name: None,
            },
            FrameSelectorInput::ByName(value) => Self {
                frame_id: None,
                frame_name: Some(value.frame_name),
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileInput {
    #[serde(default)]
    pub profile: Option<String>,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindInput {
    #[serde(default)]
    pub profile: Option<String>,
    pub query: String,
    #[serde(default = "default_contains")]
    #[schemars(with = "FindModeSchema")]
    pub mode: String,
    #[serde(default = "default_find_limit")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: usize,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TopInput {
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default = "default_self")]
    #[schemars(with = "MetricSchema")]
    pub sort: String,
    #[serde(default = "default_top_limit")]
    #[schemars(range(min = 1, max = 200))]
    pub limit: usize,
    pub focus: Option<FrameSelectorInput>,
    pub name_regex: Option<String>,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TreeInput {
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub root_node_id: u32,
    pub profile_fingerprint: Option<String>,
    #[serde(default = "default_tree_depth")]
    #[schemars(range(min = 0, max = 16))]
    pub max_depth: usize,
    #[serde(default = "default_tree_nodes")]
    #[schemars(range(min = 1, max = 512))]
    pub max_nodes: usize,
    #[serde(default = "default_tree_percent")]
    #[schemars(range(min = 0.0, max = 100.0))]
    pub min_scope_percent: f64,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectionInput {
    #[serde(default)]
    pub profile: Option<String>,
    pub frame: FrameSelectorInput,
    #[serde(default = "default_direction_depth")]
    #[schemars(range(min = 0, max = 16))]
    pub max_depth: usize,
    #[serde(default = "default_tree_nodes")]
    #[schemars(range(min = 1, max = 512))]
    pub max_nodes: usize,
    #[serde(default = "default_tree_percent")]
    #[schemars(range(min = 0.0, max = 100.0))]
    pub min_scope_percent: f64,
    #[serde(default)]
    pub continuation: Option<DirectionContinuationInput>,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectionContinuationInput {
    #[serde(default)]
    #[schemars(length(min = 1, max = 4096))]
    pub node_path: Vec<u32>,
    pub profile_fingerprint: String,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PathsInput {
    #[serde(default)]
    pub profile: Option<String>,
    pub through: FrameSelectorInput,
    #[serde(default = "default_paths_limit")]
    #[schemars(range(min = 1, max = 50))]
    pub limit: usize,
    #[serde(default = "default_max_total_frames")]
    #[schemars(range(min = 1, max = 5000))]
    pub max_total_frames: usize,
    pub frame_window: Option<FrameWindowInput>,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum FrameWindowInput {
    Head {
        #[schemars(range(min = 1, max = 4096))]
        lines: usize,
    },
    Tail {
        #[schemars(range(min = 1, max = 4096))]
        lines: usize,
    },
    AroundTarget {
        #[schemars(range(min = 0, max = 4096))]
        before: usize,
        #[schemars(range(min = 0, max = 4096))]
        after: usize,
    },
}
impl From<FrameWindowInput> for FrameWindow {
    fn from(value: FrameWindowInput) -> Self {
        match value {
            FrameWindowInput::Head { lines } => Self::Head { lines },
            FrameWindowInput::Tail { lines } => Self::Tail { lines },
            FrameWindowInput::AroundTarget { before, after } => {
                Self::AroundTarget { before, after }
            }
        }
    }
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiffInput {
    pub baseline: String,
    pub candidate: String,
    #[serde(default = "default_self")]
    #[schemars(with = "MetricSchema")]
    pub metric: String,
    #[serde(default = "default_regression")]
    #[schemars(with = "DiffSortSchema")]
    pub sort: String,
    #[serde(default = "default_diff_limit")]
    #[schemars(range(min = 1, max = 200))]
    pub limit: usize,
    pub name_regex: Option<String>,
}
fn default_contains() -> String {
    "contains".into()
}
fn default_self() -> String {
    "self".into()
}
fn default_regression() -> String {
    "regression".into()
}
fn default_find_limit() -> usize {
    20
}
fn default_top_limit() -> usize {
    20
}
fn default_tree_depth() -> usize {
    4
}
fn default_direction_depth() -> usize {
    5
}
fn default_tree_nodes() -> usize {
    64
}
fn default_tree_percent() -> f64 {
    0.1
}
fn default_paths_limit() -> usize {
    10
}
fn default_max_total_frames() -> usize {
    DEFAULT_MAX_TOTAL_FRAMES
}
fn default_diff_limit() -> usize {
    30
}

pub(crate) fn parse_metric(value: &str) -> Result<TopSort, ApiError> {
    match value {
        "self" => Ok(TopSort::SelfWeight),
        "inclusive" => Ok(TopSort::Inclusive),
        _ => Err(ApiError::new(
            "invalid_budget",
            "sort/metric must be self or inclusive",
            json!({"value":value}),
            "Use one documented enum value.",
        )),
    }
}
pub(crate) fn parse_input<T: DeserializeOwned>(
    arguments: &serde_json::Map<String, Value>,
) -> Result<T, rmcp::ErrorData> {
    serde_json::from_value(Value::Object(arguments.clone())).map_err(|error| {
        rmcp::ErrorData::invalid_params(format!("invalid tool arguments: {error}"), None)
    })
}
pub(crate) fn parse_match(value: &str) -> Result<MatchMode, ApiError> {
    match value {
        "contains" => Ok(MatchMode::Contains),
        "regex" => Ok(MatchMode::Regex),
        _ => Err(ApiError::new(
            "invalid_budget",
            "mode must be contains or regex",
            json!({"mode":value}),
            "Use one documented enum value.",
        )),
    }
}
pub(crate) fn parse_diff_sort(value: &str) -> Result<DiffSort, ApiError> {
    match value {
        "regression" => Ok(DiffSort::Regression),
        "improvement" => Ok(DiffSort::Improvement),
        "absolute" => Ok(DiffSort::Absolute),
        _ => Err(ApiError::new(
            "invalid_budget",
            "diff sort must be regression, improvement, or absolute",
            json!({"sort":value}),
            "Use one documented enum value.",
        )),
    }
}

//! Tool input contracts, frame selectors, and argument parsing.

use schemars::JsonSchema;
use serde::Deserialize;

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
                "properties":{"frame_id":{"type":"integer","minimum":0,"maximum":4294967295u32}},
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
    pub mode: MatchMode,
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
    pub metric: TopSort,
    #[serde(default = "default_top_limit")]
    #[schemars(range(min = 1, max = 200))]
    pub limit: usize,
    /// Restrict to complete stacks containing this frame.
    pub frame: Option<FrameSelectorInput>,
    pub name_regex: Option<String>,
}
/// How a tree is rendered. `text` is one indented line per node: smaller and
/// easier to read, but not addressable field by field.
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TreeFormat {
    #[default]
    Json,
    Text,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TreeInput {
    #[serde(default)]
    pub profile: Option<String>,
    /// Copy a returned continuation unchanged; omit profile.
    pub continuation: Option<Continuation>,
    /// Edges below the current root.
    #[serde(default = "default_tree_depth")]
    #[schemars(range(min = 0, max = 16))]
    pub max_depth: usize,
    /// Includes the root.
    #[serde(default = "default_tree_nodes")]
    #[schemars(range(min = 1, max = 512))]
    pub max_nodes: usize,
    /// Percentage of the current scope.
    #[serde(default = "default_tree_percent")]
    #[schemars(range(min = 0.0, max = 100.0))]
    pub min_scope_percent: f64,
    /// `text` renders one indented line per node.
    #[serde(default)]
    pub format: TreeFormat,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(extend("oneOf" = [{"required":["frame"]}, {"required":["continuation"]}]))]
pub(crate) struct DirectionInput {
    #[serde(default)]
    pub profile: Option<String>,
    /// Exact frame; omit when resuming.
    pub frame: Option<FrameSelectorInput>,
    /// Edges below the current root.
    #[serde(default = "default_tree_depth")]
    #[schemars(range(min = 0, max = 16))]
    pub max_depth: usize,
    /// Includes the root.
    #[serde(default = "default_tree_nodes")]
    #[schemars(range(min = 1, max = 512))]
    pub max_nodes: usize,
    /// Percentage of the current scope.
    #[serde(default = "default_tree_percent")]
    #[schemars(range(min = 0.0, max = 100.0))]
    pub min_scope_percent: f64,
    /// Copy a returned continuation unchanged; omit profile and frame.
    pub continuation: Option<Continuation>,
}
pub(crate) use crate::output::{Continuation, ContinuationTool};

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PathsInput {
    #[serde(default)]
    pub profile: Option<String>,
    pub frame: FrameSelectorInput,
    #[serde(default = "default_paths_limit")]
    #[schemars(range(min = 1, max = 50))]
    pub limit: usize,
    #[serde(default = "default_max_total_frames")]
    #[schemars(range(min = 1, max = 5000))]
    /// Total displayed frames across all returned paths.
    pub max_total_frames: usize,
    /// Display crop; selection and weights stay unchanged.
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
    pub metric: TopSort,
    #[serde(default = "default_regression")]
    pub sort: DiffSort,
    #[serde(default = "default_diff_limit")]
    #[schemars(range(min = 1, max = 200))]
    pub limit: usize,
    pub name_regex: Option<String>,
}
fn default_contains() -> MatchMode {
    MatchMode::Contains
}
fn default_self() -> TopSort {
    TopSort::SelfWeight
}
fn default_regression() -> DiffSort {
    DiffSort::Regression
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

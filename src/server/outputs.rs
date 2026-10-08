//! Typed tool-output envelopes and JSON-Schema builders.

use std::{collections::BTreeMap, sync::Arc};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::error::ApiError;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct WeightSemantics {
    unit: String,
    basis: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sample_period_us: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct OutputProfile {
    canonical_path: String,
    fingerprint: String,
    byte_len: u64,
    modified_unix_ms: Option<u64>,
    alias: String,
    weight_semantics: WeightSemantics,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct TruncationReason {
    kind: String,
    #[serde(flatten)]
    #[schemars(flatten)]
    details: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct OutputEnvelope<T> {
    schema_version: String,
    profile: OutputProfile,
    scope_weight: u64,
    truncated: bool,
    truncation_reasons: Vec<TruncationReason>,
    warnings: Vec<String>,
    #[serde(default)]
    next_steps: Vec<String>,
    data: T,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct TopData {
    sort: String,
    focus: Option<u32>,
    rows: Vec<crate::output::FrameRow>,
    #[serde(default)]
    grouped_rows: Vec<GroupedFrameRow>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct GroupedFrameRow {
    normalized_name: String,
    member_count: usize,
    members: Vec<String>,
    self_weight: u64,
    inclusive_weight: u64,
    stack_count_sum: u32,
    profile_percent: f64,
    scope_percent: f64,
}
pub(crate) type TopOutput = OutputEnvelope<TopData>;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct TotalFrameBudget {
    limit: usize,
    available: usize,
    returned: usize,
    omitted: usize,
    selected_paths: usize,
    returned_paths: usize,
    omitted_paths: usize,
    cropped_paths: usize,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct PathRow {
    frames: Vec<String>,
    weight: u64,
    profile_percent: f64,
    scope_percent: f64,
    target_positions: Vec<usize>,
    display_target_positions: Vec<usize>,
    total_depth: usize,
    #[serde(default)]
    requested_frame_start: usize,
    #[serde(default)]
    requested_frame_end: usize,
    frame_start: usize,
    frame_end: usize,
    #[serde(default)]
    omitted_before: usize,
    #[serde(default)]
    omitted_after: usize,
    #[serde(default)]
    budget_omitted_before: usize,
    #[serde(default)]
    budget_omitted_after: usize,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct PathsData {
    through: u32,
    paths: Vec<PathRow>,
    total_frame_budget: TotalFrameBudget,
}
pub(crate) type PathsOutput = OutputEnvelope<PathsData>;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct TreeNode {
    node_id: Option<u32>,
    frame_id: Option<u32>,
    name: Option<String>,
    self_weight: u64,
    total_weight: u64,
    profile_percent: f64,
    scope_percent: f64,
    omitted_children: usize,
    omitted_weight: u64,
    children: Vec<TreeNode>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct Continuation {
    node_id: u32,
    frame_id: u32,
    name: String,
    reason: String,
    profile_fingerprint: String,
    total_weight: u64,
    profile_percent: f64,
    scope_percent: f64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct TreeData {
    root: TreeNode,
    continuations: Vec<Continuation>,
    continuations_truncated: bool,
    continuation_limit: usize,
    continuations_available: usize,
    continuations_omitted: usize,
}
pub(crate) type TreeOutput = OutputEnvelope<TreeData>;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct DirectionData {
    frame: crate::output::FrameRow,
    root: TreeNode,
    #[serde(default)]
    continuations: Vec<DirectionContinuation>,
    #[serde(default)]
    continuations_truncated: bool,
    #[serde(default)]
    continuation_limit: usize,
    #[serde(default)]
    continuations_available: usize,
    #[serde(default)]
    continuations_omitted: usize,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct DirectionContinuation {
    node_path: Vec<u32>,
    frame_id: u32,
    name: String,
    reason: String,
    profile_fingerprint: String,
    total_weight: u64,
    profile_percent: f64,
    scope_percent: f64,
}
pub(crate) type DirectionOutput = OutputEnvelope<DirectionData>;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct DiffRow {
    name: String,
    baseline_weight: u64,
    candidate_weight: u64,
    baseline_percent: f64,
    candidate_percent: f64,
    delta_pp: f64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct DiffData {
    metric: String,
    sort: String,
    #[serde(default)]
    total_weight_ratio: f64,
    rows: Vec<DiffRow>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct DiffScopeWeight {
    baseline: u64,
    candidate: u64,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub(crate) struct DiffOutput {
    schema_version: String,
    baseline: OutputProfile,
    candidate: OutputProfile,
    scope_weight: DiffScopeWeight,
    truncated: bool,
    truncation_reasons: Vec<TruncationReason>,
    warnings: Vec<String>,
    #[serde(default)]
    next_steps: Vec<String>,
    data: DiffData,
}

pub(crate) fn output_schema(
    required: &[&str],
    scope_weight: Value,
) -> Arc<serde_json::Map<String, Value>> {
    let profile = serde_json::json!({
        "type":"object",
        "properties":{"canonical_path":{"type":"string"},"fingerprint":{"type":"string"},"byte_len":{"type":"integer"},"modified_unix_ms":{"type":["integer","null"]},"alias":{"type":"string"},"weight_semantics":{"type":"object","properties":{"unit":{"type":"string","const":"opaque"},"basis":{"type":"string","const":"folded_input"},"sample_period_us":{"type":"integer","minimum":1}},"required":["unit","basis"]}},
        "required":["canonical_path","fingerprint","byte_len","modified_unix_ms","alias","weight_semantics"]
    });
    let properties = serde_json::json!({
        "schema_version":{"type":"string","const":"2"},
        "profile":profile.clone(),
        "baseline":profile.clone(),
        "candidate":profile,
        "scope_weight":scope_weight,
        "truncated":{"type":"boolean"},
        "truncation_reasons":{"type":"array","items":{"type":"object","properties":{"kind":{"type":"string"}},"required":["kind"]}},
        "warnings":{"type":"array","items":{"type":"string"}},
        "data":{"type":"object"}
    });
    let error = serde_json::json!({
        "type":"object",
        "properties":{"code":{"type":"string"},"message":{"type":"string"},"details":{},"retry_hint":{"type":"string"}},
        "required":["code","message","details","retry_hint"]
    });
    Arc::new(
        serde_json::json!({
            "type":"object",
            "properties":properties,
            "anyOf":[{"required":required},error]
        })
        .as_object()
        .expect("static output schema is object")
        .clone(),
    )
}
pub(crate) fn single_output_schema() -> Arc<serde_json::Map<String, Value>> {
    output_schema(
        &[
            "schema_version",
            "profile",
            "scope_weight",
            "truncated",
            "truncation_reasons",
            "warnings",
            "data",
        ],
        serde_json::json!({"type":"integer","minimum":0}),
    )
}
pub(crate) fn typed_output_schema<T: JsonSchema>() -> Arc<serde_json::Map<String, Value>> {
    let mut schema = serde_json::to_value(schemars::schema_for!(T))
        .expect("static output schema serializes")
        .as_object()
        .expect("static output schema is object")
        .clone();
    let required = schema
        .remove("required")
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let properties = schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .expect("derived output schema has properties");
    properties.insert(
        "schema_version".into(),
        json!({"type":"string","const":"2"}),
    );
    properties.insert("code".into(), json!({"type":"string"}));
    properties.insert("message".into(), json!({"type":"string"}));
    properties.insert("details".into(), json!({}));
    properties.insert("retry_hint".into(), json!({"type":"string"}));
    schema.insert(
        "anyOf".into(),
        json!([
            {"required":required},
            {"required":["code","message","details","retry_hint"]}
        ]),
    );
    schema.into()
}

pub(crate) fn typed_output<T>(value: Value) -> Result<Value, ApiError>
where
    T: DeserializeOwned + Serialize,
{
    let typed: T = serde_json::from_value(value).map_err(|error| {
        ApiError::new(
            "internal_error",
            format!("Internal typed output contract mismatch: {error}"),
            json!({}),
            "Retry the query; if it persists, report the profile and arguments.",
        )
    })?;
    serde_json::to_value(typed)
        .map_err(|_| ApiError::internal("Could not serialize typed tool output"))
}

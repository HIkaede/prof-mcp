use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::profile::{FrameId, FrameStats, Profile};

pub const SCHEMA_VERSION: &str = "2";

pub fn percent(weight: u64, scope: u64) -> f64 {
    if scope == 0 {
        0.0
    } else {
        (weight as f64) * 100.0 / (scope as f64)
    }
}

pub fn profile_meta(profile: &Profile) -> Value {
    json!({
        "canonical_path": profile.source.canonical_path.display().to_string(),
        "fingerprint": profile.source.fingerprint,
        "byte_len": profile.source.byte_len,
        "modified_unix_ms": profile.source.modified_unix_ms,
    })
}

pub fn envelope(
    profile: &Profile,
    scope_weight: u64,
    truncation_reasons: Vec<Value>,
    warnings: Vec<String>,
    data: Value,
) -> Value {
    json!({
        "schema_version":SCHEMA_VERSION,
        "profile":profile_meta(profile),
        "scope_weight":scope_weight,
        "truncated":!truncation_reasons.is_empty(),
        "truncation_reasons":truncation_reasons,
        "warnings":warnings,
        "data":data
    })
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
pub struct FrameRow {
    pub frame_id: FrameId,
    pub name: String,
    pub self_weight: u64,
    pub inclusive_weight: u64,
    pub stack_count: u32,
    pub profile_percent: f64,
    pub scope_percent: f64,
}

pub fn frame_row(
    profile: &Profile,
    id: FrameId,
    stats: &FrameStats,
    scope_weight: u64,
) -> FrameRow {
    frame_row_with_percent_weight(profile, id, stats, scope_weight, stats.inclusive_weight)
}

pub fn frame_row_with_percent_weight(
    profile: &Profile,
    id: FrameId,
    stats: &FrameStats,
    scope_weight: u64,
    percent_weight: u64,
) -> FrameRow {
    FrameRow {
        frame_id: id,
        name: profile.frame_name(id).to_owned(),
        self_weight: stats.self_weight,
        inclusive_weight: stats.inclusive_weight,
        stack_count: stats.stack_count,
        profile_percent: percent(percent_weight, profile.total_weight),
        scope_percent: percent(percent_weight, scope_weight),
    }
}

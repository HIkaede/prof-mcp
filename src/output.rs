use schemars::JsonSchema;
use serde::Serialize;

use crate::profile::{FrameId, FrameStats, Profile};

mod budget;
mod types;
#[cfg(test)]
pub(crate) use budget::MAX_RESPONSE_BYTES;
pub(crate) use budget::{ResponseBudget, response_too_large};
pub use types::*;

pub const SCHEMA_VERSION: &str = "2";

pub fn percent(weight: u64, scope: u64) -> f64 {
    if scope == 0 {
        0.0
    } else {
        (weight as f64) * 100.0 / (scope as f64)
    }
}

pub(crate) fn typed_envelope<'a, T>(
    profile: &'a Profile,
    scope_weight: u64,
    truncation_reasons: Vec<TruncationReason>,
    warnings: Vec<&'static str>,
    data: T,
) -> OutputEnvelope<'a, T> {
    OutputEnvelope {
        schema_version: SCHEMA_VERSION,
        profile: output_profile(profile),
        scope_weight,
        truncated: !truncation_reasons.is_empty(),
        truncation_reasons,
        warnings,
        data,
    }
}

pub(crate) fn output_profile(profile: &Profile) -> OutputProfile<'_> {
    OutputProfile {
        fingerprint: &profile.source.fingerprint,
        alias: "",
        weight_semantics: WeightSemantics {
            unit: "opaque",
            basis: "folded_input",
        },
    }
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct FrameRow<'a> {
    pub frame_id: FrameId,
    pub name: &'a str,
    pub self_weight: u64,
    pub inclusive_weight: u64,
    pub stack_count: u32,
    pub profile_percent: f64,
    pub scope_percent: f64,
}

pub fn frame_row<'a>(
    profile: &'a Profile,
    id: FrameId,
    stats: &FrameStats,
    scope_weight: u64,
) -> FrameRow<'a> {
    frame_row_with_percent_weight(profile, id, stats, scope_weight, stats.inclusive_weight)
}

pub fn frame_row_with_percent_weight<'a>(
    profile: &'a Profile,
    id: FrameId,
    stats: &FrameStats,
    scope_weight: u64,
    percent_weight: u64,
) -> FrameRow<'a> {
    FrameRow {
        frame_id: id,
        name: profile.frame_name(id),
        self_weight: stats.self_weight,
        inclusive_weight: stats.inclusive_weight,
        stack_count: stats.stack_count,
        profile_percent: percent(percent_weight, profile.total_weight),
        scope_percent: percent(percent_weight, scope_weight),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_borrow_model_strings() {
        let profile = crate::profile::ProfileBuilder::new(crate::profile::BuildLimits::default())
            .from_reader(
                std::io::Cursor::new(b"root;leaf 1\n"),
                "/rows.folded".into(),
            )
            .unwrap();
        let id = profile.frame_id("leaf").unwrap();
        let row = frame_row(&profile, id, &profile.frame_stats[id as usize], 1);
        let metadata = output_profile(&profile);
        assert_eq!(row.name.as_ptr(), profile.frame_name(id).as_ptr());
        assert_eq!(
            metadata.fingerprint.as_ptr(),
            profile.source.fingerprint.as_ptr()
        );
        assert_eq!(serde_json::to_value(row).unwrap()["name"], "leaf");
    }
}

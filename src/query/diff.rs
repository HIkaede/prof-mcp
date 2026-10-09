use serde_json::{Value, json};

use crate::output::{SCHEMA_VERSION, percent, profile_meta};

use super::{
    ApiError, DiffSort, Profile, TopSort, check_limit, compile_regex, metric_weight,
    row_limit_reason,
};

#[derive(serde::Serialize)]
struct DiffRow<'a> {
    name: &'a str,
    baseline_weight: u64,
    candidate_weight: u64,
    baseline_percent: f64,
    candidate_percent: f64,
    delta_pp: f64,
}

pub fn diff(
    baseline: &Profile,
    candidate: &Profile,
    metric: TopSort,
    sort: DiffSort,
    limit: usize,
    name_regex: Option<&str>,
) -> Result<Value, ApiError> {
    check_limit(limit, 1, 200, "limit")?;
    let regex = name_regex.map(compile_regex).transpose()?;
    let mut names: Vec<&str> = baseline
        .frames
        .iter()
        .map(|f| f.name.as_ref())
        .chain(candidate.frames.iter().map(|f| f.name.as_ref()))
        .collect();
    names.sort_unstable();
    names.dedup();
    let mut rows: Vec<_> = names
        .into_iter()
        .filter_map(|name| {
            if regex.as_ref().is_some_and(|re| !re.is_match(name)) {
                return None;
            }
            let weight = |profile: &Profile| {
                profile
                    .frame_id(name)
                    .map(|id| metric_weight(&profile.frame_stats[id as usize], metric))
                    .unwrap_or(0)
            };
            let baseline_weight = weight(baseline);
            let candidate_weight = weight(candidate);
            let baseline_percent = percent(baseline_weight, baseline.total_weight);
            let candidate_percent = percent(candidate_weight, candidate.total_weight);
            Some(DiffRow {
                name,
                baseline_weight,
                candidate_weight,
                baseline_percent,
                candidate_percent,
                delta_pp: candidate_percent - baseline_percent,
            })
        })
        .collect();
    rows.sort_unstable_by(|a, b| {
        let primary = match sort {
            DiffSort::Regression => b.delta_pp.total_cmp(&a.delta_pp),
            DiffSort::Improvement => a.delta_pp.total_cmp(&b.delta_pp),
            DiffSort::Absolute => b.delta_pp.abs().total_cmp(&a.delta_pp.abs()),
        };
        primary.then_with(|| a.name.cmp(b.name))
    });
    let available = rows.len();
    let truncation_reasons = row_limit_reason(limit, available);
    rows.truncate(limit);
    let warnings =
        vec!["Percentage-point changes do not prove causality or statistical significance."];
    Ok(
        json!({"schema_version":SCHEMA_VERSION,"baseline":profile_meta(baseline),"candidate":profile_meta(candidate),"scope_weight":{"baseline":baseline.total_weight,"candidate":candidate.total_weight},"truncated":!truncation_reasons.is_empty(),"truncation_reasons":truncation_reasons,"warnings":warnings,"data":{"metric":match metric {TopSort::SelfWeight=>"self",TopSort::Inclusive=>"inclusive"},"sort":match sort {DiffSort::Regression=>"regression",DiffSort::Improvement=>"improvement",DiffSort::Absolute=>"absolute"},"rows":rows}}),
    )
}

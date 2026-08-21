use serde_json::{Value, json};

use crate::output::{SCHEMA_VERSION, percent, profile_meta};

use super::{
    ApiError, DiffSort, Profile, TopSort, check_limit, compile_regex, metric_weight,
    row_limit_reason,
};

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
    let mut names: Vec<_> = baseline
        .frames
        .iter()
        .map(|f| f.name.to_string())
        .chain(candidate.frames.iter().map(|f| f.name.to_string()))
        .collect();
    names.sort();
    names.dedup();
    let mut rows: Vec<_> = names.into_iter().filter_map(|name| {
        if regex.as_ref().is_some_and(|re| !re.is_match(&name)) { return None; }
        let b = baseline.frame_id(&name).map(|id| &baseline.frame_stats[id as usize]); let c = candidate.frame_id(&name).map(|id| &candidate.frame_stats[id as usize]);
        let bw = b.map(|s| metric_weight(s, metric)).unwrap_or(0); let cw = c.map(|s| metric_weight(s, metric)).unwrap_or(0);
        let bp = percent(bw, baseline.total_weight); let cp = percent(cw, candidate.total_weight); let delta = cp - bp;
        Some(json!({"name":name,"baseline_weight":bw,"candidate_weight":cw,"baseline_percent":bp,"candidate_percent":cp,"delta_pp":delta}))
    }).collect::<Vec<Value>>();
    rows.sort_by(|a, b| {
        let ad = a["delta_pp"].as_f64().unwrap_or(0.0);
        let bd = b["delta_pp"].as_f64().unwrap_or(0.0);
        let primary = match sort {
            DiffSort::Regression => bd.total_cmp(&ad),
            DiffSort::Improvement => ad.total_cmp(&bd),
            DiffSort::Absolute => bd.abs().total_cmp(&ad.abs()),
        };
        primary.then_with(|| a["name"].as_str().cmp(&b["name"].as_str()))
    });
    let available = rows.len();
    let truncation_reasons = row_limit_reason(limit, available);
    rows.truncate(limit);
    let mut warnings =
        vec!["Percentage-point changes do not prove causality or statistical significance.".into()];
    let total_weight_ratio = total_weight_ratio(baseline.total_weight, candidate.total_weight);
    if !(0.8..=1.25).contains(&total_weight_ratio) {
        warnings.push(format!(
            "Total weight differs by {:.2}x (baseline={}, candidate={}); delta_pp conflates relative share change with overall workload change.",
            total_weight_ratio, baseline.total_weight, candidate.total_weight
        ));
    }
    Ok(
        json!({"schema_version":SCHEMA_VERSION,"baseline":profile_meta(baseline),"candidate":profile_meta(candidate),"scope_weight":{"baseline":baseline.total_weight,"candidate":candidate.total_weight},"truncated":!truncation_reasons.is_empty(),"truncation_reasons":truncation_reasons,"warnings":warnings,"data":{"metric":match metric {TopSort::SelfWeight=>"self",TopSort::Inclusive=>"inclusive"},"sort":match sort {DiffSort::Regression=>"regression",DiffSort::Improvement=>"improvement",DiffSort::Absolute=>"absolute"},"total_weight_ratio":total_weight_ratio,"rows":rows}}),
    )
}

fn total_weight_ratio(baseline_total: u64, candidate_total: u64) -> f64 {
    if baseline_total == 0 || candidate_total == 0 {
        return 1.0;
    }
    candidate_total as f64 / baseline_total as f64
}

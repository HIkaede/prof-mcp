use serde_json::{Value, json};

use super::{
    ApiError, MatchMode, Profile, TopSort, check_limit, compile_regex, envelope, frame_order,
    frame_row, row_limit_reason,
};

pub fn find_symbols(
    profile: &Profile,
    query: &str,
    mode: MatchMode,
    limit: usize,
) -> Result<Value, ApiError> {
    check_limit(limit, 1, 100, "limit")?;
    let regex = match mode {
        MatchMode::Contains => None,
        MatchMode::Regex => Some(compile_regex(query)?),
    };
    let mut ids: Vec<_> = (0..profile.frames.len() as u32)
        .filter(|id| match &regex {
            Some(regex) => regex.is_match(profile.frame_name(*id)),
            None => profile.frame_name(*id).contains(query),
        })
        .collect();
    ids.sort_by(|a, b| frame_order(profile, *a, *b, TopSort::Inclusive));
    let available = ids.len();
    let truncation_reasons = row_limit_reason(limit, available);
    ids.truncate(limit);
    let warnings = if ids.is_empty() {
        vec!["No exact frame identities matched.".into()]
    } else {
        Vec::new()
    };
    let rows: Vec<_> = ids
        .iter()
        .map(|id| {
            frame_row(
                profile,
                *id,
                &profile.frame_stats[*id as usize],
                profile.total_weight,
            )
        })
        .collect();
    Ok(envelope(
        profile,
        profile.total_weight,
        truncation_reasons,
        warnings,
        json!({"query":query,"mode":match mode {MatchMode::Contains=>"contains",MatchMode::Regex=>"regex"},"matches":rows}),
    ))
}

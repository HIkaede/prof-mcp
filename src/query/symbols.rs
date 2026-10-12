use crate::output::{FindData, FindOutput, ResponseBudget};

use super::{
    ApiError, MatchMode, Profile, check_limit, compile_regex, envelope, frame_row, row_limit_reason,
};

pub fn find_symbols<'p>(
    profile: &'p Profile,
    query: &'p str,
    mode: MatchMode,
    limit: usize,
) -> Result<FindOutput<'p>, ApiError> {
    check_limit(limit, 1, 100, "limit")?;
    let regex = match mode {
        MatchMode::Contains => None,
        MatchMode::Regex => Some(compile_regex(query)?),
    };
    let mut ids = Vec::with_capacity(limit);
    let mut available = 0;
    for &id in profile.top_inclusive.iter() {
        let matches = match &regex {
            Some(regex) => regex.is_match(profile.frame_name(id)),
            None => profile.frame_name(id).contains(query),
        };
        if matches {
            available += 1;
            if ids.len() < limit {
                ids.push(id);
            }
        }
    }
    let truncation_reasons = row_limit_reason(limit, available);
    let warnings = if ids.is_empty() {
        vec!["No exact frame identities matched."]
    } else {
        Vec::new()
    };
    let mut bytes = ResponseBudget::new();
    bytes.json(&query)?;
    let rows: Vec<_> = ids
        .iter()
        .map(|id| {
            bytes.row_name(profile.frame_name(*id))?;
            Ok(frame_row(
                profile,
                *id,
                &profile.frame_stats[*id as usize],
                profile.total_weight,
            ))
        })
        .collect::<Result<_, ApiError>>()?;
    Ok(envelope(
        profile,
        profile.total_weight,
        truncation_reasons,
        warnings,
        FindData {
            query,
            mode,
            matches: rows,
        },
    ))
}

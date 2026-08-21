use hashbrown::{HashMap, HashSet};
use serde_json::{Value, json};

use super::{
    ApiError, FrameId, MatchMode, Profile, TopSort, check_limit, compile_regex, envelope,
    frame_order, frame_row, row_limit_reason,
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
        vec!["No exact frame identities matched; try a broader contains query or profile_find_symbols regex.".into()]
    } else {
        Vec::new()
    };
    let rows: Vec<_> = ids
        .iter()
        .map(|id| {
            let row = frame_row(
                profile,
                *id,
                &profile.frame_stats[*id as usize],
                profile.total_weight,
            );
            let mut value = serde_json::to_value(row).expect("frame row serializes");
            value
                .as_object_mut()
                .expect("frame row is an object")
                .insert("context_hint".into(), symbol_context(profile, *id));
            value
        })
        .collect();
    Ok(envelope(
        profile,
        profile.total_weight,
        truncation_reasons,
        warnings,
        json!({"query":query,"mode":match mode {MatchMode::Contains=>"contains",MatchMode::Regex=>"regex"},"symbol_metadata":"folded_frames_and_observed_context_only","matches":rows}),
    ))
}

fn symbol_context(profile: &Profile, frame: FrameId) -> Value {
    let mut callers: HashMap<FrameId, u64> = HashMap::new();
    let mut callees: HashMap<FrameId, u64> = HashMap::new();
    for stack_id in &profile.frame_to_stacks[frame as usize] {
        let stack = &profile.stacks[*stack_id as usize];
        let mut stack_callers = HashSet::new();
        let mut stack_callees = HashSet::new();
        for (position, candidate) in stack.frames.iter().enumerate() {
            if *candidate != frame {
                continue;
            }
            if position > 0 {
                stack_callers.insert(stack.frames[position - 1]);
            }
            if position + 1 < stack.frames.len() {
                stack_callees.insert(stack.frames[position + 1]);
            }
        }
        for caller in stack_callers {
            *callers.entry(caller).or_default() += stack.weight;
        }
        for callee in stack_callees {
            *callees.entry(callee).or_default() += stack.weight;
        }
    }
    json!({
        "top_callers":context_rows(profile,callers),
        "top_callees":context_rows(profile,callees)
    })
}

fn context_rows(profile: &Profile, weights: HashMap<FrameId, u64>) -> Vec<Value> {
    let mut rows: Vec<_> = weights.into_iter().collect();
    rows.sort_by(|(left_id, left_weight), (right_id, right_weight)| {
        right_weight
            .cmp(left_weight)
            .then_with(|| {
                profile
                    .frame_name(*left_id)
                    .cmp(profile.frame_name(*right_id))
            })
            .then(left_id.cmp(right_id))
    });
    rows.into_iter()
        .take(3)
        .map(|(frame_id, weight)| {
            json!({"frame_id":frame_id,"name":profile.frame_name(frame_id),"weight":weight})
        })
        .collect()
}

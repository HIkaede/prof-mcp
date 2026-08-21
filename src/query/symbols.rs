use std::collections::BTreeMap;

use hashbrown::{HashMap, HashSet};
use serde_json::{Value, json};

use super::{
    ApiError, FrameId, MatchMode, Profile, TopSort, check_limit, compile_regex, envelope,
    frame_order, frame_row, normalize_frame_name, row_limit_reason,
};

pub fn find_symbols(
    profile: &Profile,
    query: &str,
    mode: MatchMode,
    limit: usize,
    normalize: bool,
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
    if normalize {
        return find_symbols_grouped(profile, query, mode, limit, ids);
    }
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

fn find_symbols_grouped(
    profile: &Profile,
    query: &str,
    mode: MatchMode,
    limit: usize,
    ids: Vec<FrameId>,
) -> Result<Value, ApiError> {
    struct Group {
        members: Vec<FrameId>,
        self_weight: u64,
        inclusive_weight: u64,
    }
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for id in ids {
        let key = normalize_frame_name(profile.frame_name(id));
        let stats = &profile.frame_stats[id as usize];
        let group = groups.entry(key).or_insert(Group {
            members: Vec::new(),
            self_weight: 0,
            inclusive_weight: 0,
        });
        group.members.push(id);
        group.self_weight += stats.self_weight;
        group.inclusive_weight += stats.inclusive_weight;
    }
    let mut rows: Vec<(String, Group)> = groups.into_iter().collect();
    rows.sort_by(|(left_name, left), (right_name, right)| {
        right
            .inclusive_weight
            .cmp(&left.inclusive_weight)
            .then_with(|| left_name.cmp(right_name))
    });
    let available = rows.len();
    let truncation_reasons = row_limit_reason(limit, available);
    rows.truncate(limit);
    let warnings = if rows.is_empty() {
        vec!["No exact frame identities matched; try a broader contains query or profile_find_symbols regex.".into()]
    } else {
        Vec::new()
    };
    let matches: Vec<_> = rows
        .into_iter()
        .map(|(normalized_name, group)| {
            let mut members: Vec<String> = group
                .members
                .iter()
                .map(|id| profile.frame_name(*id).to_string())
                .collect();
            members.sort();
            members.truncate(5);
            json!({
                "normalized_name":normalized_name,
                "member_count":group.members.len(),
                "members":members,
                "self_weight":group.self_weight,
                "inclusive_weight":group.inclusive_weight,
                "profile_percent":super::percent(group.inclusive_weight,profile.total_weight)
            })
        })
        .collect();
    Ok(envelope(
        profile,
        profile.total_weight,
        truncation_reasons,
        warnings,
        json!({"query":query,"mode":match mode {MatchMode::Contains=>"contains",MatchMode::Regex=>"regex"},"symbol_metadata":"folded_frames_and_observed_context_only","matches":matches}),
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

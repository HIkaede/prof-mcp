use hashbrown::HashSet;
use serde_json::{Value, json};

use super::{
    ApiError, FrameSelector, FrameStats, Profile, TopSort, check_limit, compile_regex, envelope,
    frame_order_stats, frame_row_with_percent_weight, metric_weight, resolve_selector,
    row_limit_reason,
};

pub fn top(
    profile: &Profile,
    sort: TopSort,
    limit: usize,
    focus: Option<&FrameSelector>,
    name_regex: Option<&str>,
) -> Result<Value, ApiError> {
    check_limit(limit, 1, 200, "limit")?;
    let focused_id = focus
        .map(|selector| resolve_selector(profile, selector))
        .transpose()?;
    let stack_ids: Vec<_> = match focused_id {
        Some(id) => profile.frame_to_stacks[id as usize].clone(),
        None => (0..profile.stacks.len() as u32).collect(),
    };
    let scope_weight = stack_ids
        .iter()
        .map(|id| profile.stacks[*id as usize].weight)
        .sum();
    let stats = subset_stats(profile, &stack_ids);
    let regex = name_regex.map(compile_regex).transpose()?;
    let mut ids: Vec<_> = (0..profile.frames.len() as u32)
        .filter(|id| {
            stats[*id as usize].inclusive_weight > 0
                && regex
                    .as_ref()
                    .is_none_or(|re| re.is_match(profile.frame_name(*id)))
        })
        .collect();
    ids.sort_by(|a, b| frame_order_stats(profile, &stats, *a, *b, sort));
    let available = ids.len();
    let truncation_reasons = row_limit_reason(limit, available);
    ids.truncate(limit);
    let rows: Vec<_> = ids
        .iter()
        .map(|id| {
            frame_row_with_percent_weight(
                profile,
                *id,
                &stats[*id as usize],
                scope_weight,
                metric_weight(&stats[*id as usize], sort),
            )
        })
        .collect();
    Ok(envelope(
        profile,
        scope_weight,
        truncation_reasons,
        Vec::new(),
        json!({"sort":match sort {TopSort::SelfWeight=>"self",TopSort::Inclusive=>"inclusive"},"focus":focused_id,"rows":rows}),
    ))
}

fn subset_stats(profile: &Profile, stack_ids: &[u32]) -> Vec<FrameStats> {
    let mut result = vec![FrameStats::default(); profile.frames.len()];
    for stack_id in stack_ids {
        let stack = &profile.stacks[*stack_id as usize];
        let mut seen = HashSet::new();
        for frame in &stack.frames {
            if seen.insert(*frame) {
                result[*frame as usize].inclusive_weight += stack.weight;
                result[*frame as usize].stack_count += 1;
            }
        }
        let leaf = *stack.frames.last().expect("non-empty stack");
        result[leaf as usize].self_weight += stack.weight;
    }
    result
}

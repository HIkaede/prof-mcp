//! Self/inclusive ranking of exact folded-frame identities.

use hashbrown::HashMap;
use serde_json::{Value, json};

use super::{
    ApiError, FrameSelector, FrameStats, Profile, TopSort, check_limit, compile_regex, envelope,
    frame_order_stats, metric_weight, resolve_selector, row_limit_reason,
};

#[derive(serde::Serialize)]
struct TopRow<'a> {
    frame_id: u32,
    name: &'a str,
    self_weight: u64,
    inclusive_weight: u64,
    stack_count: u32,
    profile_percent: f64,
    scope_percent: f64,
}

pub fn top(
    profile: &Profile,
    metric: TopSort,
    limit: usize,
    frame: Option<&FrameSelector>,
    name_regex: Option<&str>,
) -> Result<Value, ApiError> {
    check_limit(limit, 1, 200, "limit")?;
    let frame_id = frame
        .map(|selector| resolve_selector(profile, selector))
        .transpose()?;
    let scope_weight = frame_id.map_or(profile.total_weight, |id| {
        profile.frame_stats[id as usize].inclusive_weight
    });
    let scoped_stacks = frame_id.and_then(|id| {
        let stacks = &profile.frame_to_stacks[id as usize];
        // A posting contains each matching stack once, including recursive frames.
        (stacks.len() != profile.stacks.len()).then_some(stacks)
    });
    let sparse_stats = scoped_stacks
        .filter(|stacks| {
            stacks.len() < profile.frames.len() / 8
                && stacks
                    .iter()
                    .map(|id| profile.stacks.stack(*id).frames.len())
                    .sum::<usize>()
                    < profile.frames.len() / 8
        })
        .map(|stacks| sparse_subset_stats(profile, stacks));
    let scoped_stats = scoped_stacks
        .filter(|_| sparse_stats.is_none())
        .map(|stacks| subset_stats(profile, stacks));
    let stats = scoped_stats.as_deref().unwrap_or(&profile.frame_stats);
    let stats_for = |id: u32| {
        sparse_stats
            .as_ref()
            .map_or(&stats[id as usize], |rows| &rows[&id].0)
    };
    let is_subset = scoped_stacks.is_some();
    let regex = name_regex.map(compile_regex).transpose()?;
    let order = match metric {
        TopSort::SelfWeight => &profile.top_self,
        TopSort::Inclusive => &profile.top_inclusive,
    };
    let sparse_ids = sparse_stats
        .as_ref()
        .map(|rows| rows.keys().copied().collect::<Vec<_>>());
    let candidates = sparse_ids.as_deref().unwrap_or(order);
    let (ids, available) = if !is_subset && regex.is_none() {
        (order[..limit.min(order.len())].to_vec(), order.len())
    } else {
        let mut ids = Vec::with_capacity(limit);
        let mut available = 0;
        for &id in candidates {
            if stats_for(id).inclusive_weight > 0
                && regex
                    .as_ref()
                    .is_none_or(|re| re.is_match(profile.frame_name(id)))
            {
                available += 1;
                if is_subset || ids.len() < limit {
                    ids.push(id);
                }
            }
        }
        if is_subset {
            let order = |a: &u32, b: &u32| {
                if sparse_stats.is_none() {
                    return frame_order_stats(profile, stats, *a, *b, metric);
                }
                let an = stats_for(*a);
                let bn = stats_for(*b);
                metric_weight(bn, metric)
                    .cmp(&metric_weight(an, metric))
                    .then_with(|| bn.self_weight.cmp(&an.self_weight))
                    .then_with(|| profile.frame_name(*a).cmp(profile.frame_name(*b)))
                    .then(a.cmp(b))
            };
            if ids.len() > limit {
                ids.select_nth_unstable_by(limit, order);
            }
            ids.truncate(limit);
            ids.sort_unstable_by(order);
        }
        (ids, available)
    };
    let truncation_reasons = row_limit_reason(limit, available);
    let rows: Vec<_> = ids
        .iter()
        .map(|id| {
            let stats = stats_for(*id);
            let weight = metric_weight(stats, metric);
            TopRow {
                frame_id: *id,
                name: profile.frame_name(*id),
                self_weight: stats.self_weight,
                inclusive_weight: stats.inclusive_weight,
                stack_count: stats.stack_count,
                profile_percent: crate::output::percent(weight, profile.total_weight),
                scope_percent: crate::output::percent(weight, scope_weight),
            }
        })
        .collect();
    Ok(envelope(
        profile,
        scope_weight,
        truncation_reasons,
        Vec::new(),
        json!({"metric":match metric {TopSort::SelfWeight=>"self",TopSort::Inclusive=>"inclusive"},"frame":frame_id,"rows":rows}),
    ))
}

fn sparse_subset_stats(profile: &Profile, stack_ids: &[u32]) -> HashMap<u32, (FrameStats, u32)> {
    let mut result = HashMap::new();
    for &stack_id in stack_ids {
        let stack = profile.stacks.stack(stack_id);
        for &frame in stack.frames {
            let (stats, seen) = result
                .entry(frame)
                .or_insert((FrameStats::default(), u32::MAX));
            if *seen != stack_id {
                *seen = stack_id;
                stats.inclusive_weight += stack.weight;
                stats.stack_count += 1;
            }
        }
        result
            .get_mut(stack.frames.last().expect("non-empty stack"))
            .unwrap()
            .0
            .self_weight += stack.weight;
    }
    result
}

fn subset_stats(profile: &Profile, stack_ids: &[u32]) -> Vec<FrameStats> {
    let mut result = vec![FrameStats::default(); profile.frames.len()];
    let mut seen = vec![u32::MAX; profile.frames.len()];
    for stack_id in stack_ids {
        let stack = profile.stacks.stack(*stack_id);
        for frame in stack.frames {
            if seen[*frame as usize] != *stack_id {
                seen[*frame as usize] = *stack_id;
                result[*frame as usize].inclusive_weight += stack.weight;
                result[*frame as usize].stack_count += 1;
            }
        }
        let leaf = *stack.frames.last().expect("non-empty stack");
        result[leaf as usize].self_weight += stack.weight;
    }
    result
}

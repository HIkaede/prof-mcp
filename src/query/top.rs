//! Self/inclusive frame ranking, optionally grouped by normalized symbol name.

use std::collections::BTreeMap;

use hashbrown::HashSet;
use serde_json::{Value, json};

use super::{
    ApiError, FrameId, FrameSelector, FrameStats, Profile, TopSort, check_limit, compile_regex,
    envelope, frame_order_stats, frame_row_with_percent_weight, metric_weight,
    normalize_frame_name, resolve_selector, row_limit_reason,
};

pub fn top(
    profile: &Profile,
    sort: TopSort,
    limit: usize,
    focus: Option<&FrameSelector>,
    name_regex: Option<&str>,
    normalize: bool,
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
    if normalize {
        return top_grouped(
            profile,
            sort,
            limit,
            focused_id,
            scope_weight,
            &stats,
            &mut ids,
        );
    }
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
        json!({"sort":match sort {TopSort::SelfWeight=>"self",TopSort::Inclusive=>"inclusive"},"focus":focused_id,"rows":rows,"grouped_rows":[]}),
    ))
}

#[allow(clippy::too_many_arguments)]
fn top_grouped(
    profile: &Profile,
    sort: TopSort,
    limit: usize,
    focused_id: Option<FrameId>,
    scope_weight: u64,
    stats: &[FrameStats],
    ids: &mut [FrameId],
) -> Result<Value, ApiError> {
    struct Group {
        members: Vec<FrameId>,
        self_weight: u64,
        inclusive_weight: u64,
        stack_count_sum: u32,
    }
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for &id in ids.iter() {
        let key = normalize_frame_name(profile.frame_name(id));
        let group = groups.entry(key).or_insert(Group {
            members: Vec::new(),
            self_weight: 0,
            inclusive_weight: 0,
            stack_count_sum: 0,
        });
        group.members.push(id);
        group.self_weight += stats[id as usize].self_weight;
        group.inclusive_weight += stats[id as usize].inclusive_weight;
        group.stack_count_sum += stats[id as usize].stack_count;
    }
    let mut rows: Vec<(String, Group)> = groups.into_iter().collect();
    rows.sort_by(|(left_name, left), (right_name, right)| {
        let weight = |group: &Group| match sort {
            TopSort::SelfWeight => group.self_weight,
            TopSort::Inclusive => group.inclusive_weight,
        };
        weight(right)
            .cmp(&weight(left))
            .then_with(|| left_name.cmp(right_name))
    });
    let available = rows.len();
    let truncation_reasons = row_limit_reason(limit, available);
    rows.truncate(limit);
    let grouped_rows: Vec<_> = rows
        .into_iter()
        .map(|(normalized_name, group)| {
            let metric = match sort {
                TopSort::SelfWeight => group.self_weight,
                TopSort::Inclusive => group.inclusive_weight,
            };
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
                "stack_count_sum":group.stack_count_sum,
                "profile_percent":percent_of(metric,profile.total_weight),
                "scope_percent":percent_of(metric,scope_weight)
            })
        })
        .collect();
    Ok(envelope(
        profile,
        scope_weight,
        truncation_reasons,
        Vec::new(),
        json!({
            "sort":match sort {TopSort::SelfWeight=>"self",TopSort::Inclusive=>"inclusive"},
            "focus":focused_id,
            "rows":[],
            "grouped_rows":grouped_rows
        }),
    ))
}

fn percent_of(weight: u64, scope: u64) -> f64 {
    super::percent(weight, scope)
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

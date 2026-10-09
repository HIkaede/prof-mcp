//! Workspace-level orientation: totals, shape, and hot frames.

use hashbrown::HashMap;
use serde_json::{Value, json};

use super::{
    FrameId, Profile, TopSort, envelope, frame_order, frame_row, frame_row_with_percent_weight,
    percent,
};

pub fn summary(profile: &Profile) -> Value {
    let mut ids: Vec<_> = (0..profile.frames.len() as u32).collect();
    ids.sort_by(|a, b| frame_order(profile, *a, *b, TopSort::SelfWeight));
    let top_self: Vec<_> = ids
        .iter()
        .take(5)
        .map(|id| {
            frame_row_with_percent_weight(
                profile,
                *id,
                &profile.frame_stats[*id as usize],
                profile.total_weight,
                profile.frame_stats[*id as usize].self_weight,
            )
        })
        .collect();
    ids.sort_by(|a, b| frame_order(profile, *a, *b, TopSort::Inclusive));
    let top_inclusive: Vec<_> = ids
        .iter()
        .take(5)
        .map(|id| {
            frame_row(
                profile,
                *id,
                &profile.frame_stats[*id as usize],
                profile.total_weight,
            )
        })
        .collect();
    let unknown_frame_weight = profile
        .frame_id("[unknown]")
        .map(|id| profile.frame_stats[id as usize].inclusive_weight)
        .unwrap_or(0);
    let stack_concentration = stack_concentration(profile);
    let recursion = recursion_report(profile);
    let mut warnings =
        vec!["Weight unit is opaque; it is not assumed to be time or cycles.".into()];
    if !recursion.is_empty() {
        warnings.push(
            "Recursive frames detected; exact-frame inclusive weights count each stack once. Context occurrences can overlap.".into(),
        );
    }
    envelope(
        profile,
        profile.total_weight,
        Vec::new(),
        warnings,
        json!({
            "total_weight":profile.total_weight,
            "frame_count":profile.frames.len(),
            "unique_stack_count":profile.stacks.len(),
            "max_depth":profile.max_depth,
            "unknown_frame_weight":unknown_frame_weight,
            "stack_concentration":stack_concentration,
            "recursion_detected":recursion,
            "top_self":top_self,
            "top_inclusive":top_inclusive
        }),
    )
}

/// Weight share covered by the heaviest unique stacks.
fn stack_concentration(profile: &Profile) -> Value {
    let mut weights: Vec<u64> = profile.stacks.iter().map(|stack| stack.weight).collect();
    weights.sort_unstable_by(|a, b| b.cmp(a));
    let share = |count: usize| -> f64 {
        let sum: u64 = weights.iter().take(count).sum();
        percent(sum, profile.total_weight)
    };
    json!({
        "top_10_stacks_percent":share(10),
        "top_50_stacks_percent":share(50),
        "unique_stack_count":weights.len()
    })
}

/// Frames that occur more than once within a single contributing stack.
fn recursion_report(profile: &Profile) -> Vec<Value> {
    #[allow(clippy::type_complexity)]
    let mut recursive: HashMap<FrameId, (usize, u64)> = HashMap::new();
    for stack in &profile.stacks {
        let mut counts: HashMap<FrameId, usize> = HashMap::new();
        for frame in &stack.frames {
            *counts.entry(*frame).or_default() += 1;
        }
        for (frame, count) in counts {
            if count > 1
                && let Some(entry) = recursive.get_mut(&frame)
            {
                entry.0 = entry.0.max(count);
                entry.1 += stack.weight;
            } else if count > 1 {
                recursive.insert(frame, (count, stack.weight));
            }
        }
    }
    let mut rows: Vec<(FrameId, usize, u64)> = recursive
        .into_iter()
        .map(|(frame, (count, weight))| (frame, count, weight))
        .collect();
    rows.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| profile.frame_name(a.0).cmp(profile.frame_name(b.0)))
            .then(a.0.cmp(&b.0))
    });
    rows.into_iter()
        .take(5)
        .map(|(frame_id, max_occurrences_per_stack, affected_weight)| {
            json!({
                "frame_id":frame_id,
                "name":profile.frame_name(frame_id),
                "max_occurrences_per_stack":max_occurrences_per_stack,
                "affected_weight":affected_weight,
                "affected_weight_percent":percent(affected_weight,profile.total_weight)
            })
        })
        .collect()
}

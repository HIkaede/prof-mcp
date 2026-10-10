//! Workspace-level orientation: totals, shape, and hot frames.

use serde_json::{Value, json};

use super::{Profile, envelope, frame_row, frame_row_with_percent_weight, percent};

pub fn summary(profile: &Profile) -> Value {
    let top_self: Vec<_> = profile
        .top_self
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
    let top_inclusive: Vec<_> = profile
        .top_inclusive
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
            "stack_concentration":stack_concentration,
            "recursion_detected":recursion,
            "top_self":top_self,
            "top_inclusive":top_inclusive
        }),
    )
}

/// Weight share covered by the heaviest unique stacks.
fn stack_concentration(profile: &Profile) -> Value {
    json!({
        "top_10_stacks_percent":percent(profile.heaviest_stack_weights[0], profile.total_weight),
        "top_50_stacks_percent":percent(profile.heaviest_stack_weights[1], profile.total_weight),
        "unique_stack_count":profile.stacks.len()
    })
}

/// Frames that occur more than once within a single contributing stack.
fn recursion_report(profile: &Profile) -> Vec<Value> {
    profile
        .recursive_frames
        .iter()
        .map(|row| {
            let frame_id = row.frame;
            let max_occurrences_per_stack = row.max_occurrences;
            let affected_weight = row.affected_weight;
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

use serde_json::{Value, json};

use super::{Profile, TopSort, envelope, frame_order, frame_row, frame_row_with_percent_weight};

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
    envelope(
        profile,
        profile.total_weight,
        Vec::new(),
        vec!["Weight unit is opaque; it is not assumed to be time or cycles.".into()],
        json!({"total_weight":profile.total_weight,"frame_count":profile.frames.len(),"unique_stack_count":profile.stacks.len(),"max_depth":profile.max_depth,"unknown_frame_weight":unknown_frame_weight,"top_self":top_self,"top_inclusive":top_inclusive}),
    )
}

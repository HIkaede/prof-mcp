use serde_json::{Value, json};

use super::{
    ApiError, DEFAULT_MAX_TOTAL_FRAMES, FrameId, FrameSelector, FrameWindow, MAX_TOTAL_FRAMES,
    Profile, StackRecord, check_limit, envelope, percent, resolve_selector, row_limit_reason,
};

pub fn paths(profile: &Profile, selector: &FrameSelector, limit: usize) -> Result<Value, ApiError> {
    paths_with_window(profile, selector, limit, None)
}

pub fn paths_with_window(
    profile: &Profile,
    selector: &FrameSelector,
    limit: usize,
    window: Option<FrameWindow>,
) -> Result<Value, ApiError> {
    paths_with_window_budget(profile, selector, limit, window, DEFAULT_MAX_TOTAL_FRAMES)
}

pub fn paths_with_window_budget(
    profile: &Profile,
    selector: &FrameSelector,
    limit: usize,
    window: Option<FrameWindow>,
    max_total_frames: usize,
) -> Result<Value, ApiError> {
    check_limit(limit, 1, 50, "limit")?;
    check_frame_window(window)?;
    check_limit(max_total_frames, 1, MAX_TOTAL_FRAMES, "max_total_frames")?;
    let frame = resolve_selector(profile, selector)?;
    let stack_ids = &profile.frame_to_stacks[frame as usize];
    let scope: u64 = stack_ids
        .iter()
        .map(|id| profile.stacks[*id as usize].weight)
        .sum();
    let mut stacks: Vec<&StackRecord> = stack_ids
        .iter()
        .map(|id| &profile.stacks[*id as usize])
        .collect();
    stacks.sort_by(|a, b| {
        b.weight.cmp(&a.weight).then_with(|| {
            frame_sequence(profile, &a.frames).cmp(&frame_sequence(profile, &b.frames))
        })
    });
    let available = stacks.len();
    let mut truncation_reasons = row_limit_reason(limit, available);
    stacks.truncate(limit);
    let selected_paths = stacks.len();
    let mut window_cropped_paths = 0usize;
    let mut window_omitted_before = 0usize;
    let mut window_omitted_after = 0usize;
    let mut budget_remaining = max_total_frames;
    let mut budget_available = 0usize;
    let mut budget_returned = 0usize;
    let mut budget_cropped_paths = 0usize;
    let mut rows = Vec::new();
    for stack in stacks {
        let positions: Vec<_> = stack
            .frames
            .iter()
            .enumerate()
            .filter_map(|(index, id)| (*id == frame).then_some(index))
            .collect();
        let total_depth = stack.frames.len();
        let (requested_frame_start, requested_frame_end) =
            frame_window_range(total_depth, &positions, window);
        let requested_frames = requested_frame_end - requested_frame_start;
        budget_available += requested_frames;
        if requested_frame_start != 0 || requested_frame_end != total_depth {
            window_cropped_paths += 1;
            window_omitted_before += requested_frame_start;
            window_omitted_after += total_depth - requested_frame_end;
        }
        if budget_remaining == 0 {
            continue;
        }
        let (frame_start, frame_end) = if requested_frames <= budget_remaining {
            (requested_frame_start, requested_frame_end)
        } else {
            budget_cropped_paths += 1;
            budget_range(
                requested_frame_start,
                requested_frame_end,
                &positions,
                window,
                budget_remaining,
            )
        };
        let returned_frames = frame_end - frame_start;
        budget_remaining -= returned_frames;
        budget_returned += returned_frames;
        let display_target_positions: Vec<_> = positions
            .iter()
            .filter(|position| **position >= frame_start && **position < frame_end)
            .map(|position| *position - frame_start)
            .collect();
        let mut row = serde_json::Map::new();
        row.insert(
            "frames".into(),
            json!(frame_sequence(
                profile,
                &stack.frames[frame_start..frame_end]
            )),
        );
        row.insert("weight".into(), json!(stack.weight));
        row.insert(
            "profile_percent".into(),
            json!(percent(stack.weight, profile.total_weight)),
        );
        row.insert("scope_percent".into(), json!(percent(stack.weight, scope)));
        row.insert("target_positions".into(), json!(positions));
        row.insert(
            "display_target_positions".into(),
            json!(display_target_positions),
        );
        row.insert("total_depth".into(), json!(total_depth));
        let cropped = requested_frame_start != frame_start || requested_frame_end != frame_end;
        if window.is_some() || cropped {
            row.insert("requested_frame_start".into(), json!(requested_frame_start));
            row.insert("requested_frame_end".into(), json!(requested_frame_end));
        }
        row.insert("frame_start".into(), json!(frame_start));
        row.insert("frame_end".into(), json!(frame_end));
        let omitted_before = frame_start;
        let omitted_after = total_depth - frame_end;
        if omitted_before > 0 || omitted_after > 0 {
            row.insert("omitted_before".into(), json!(omitted_before));
            row.insert("omitted_after".into(), json!(omitted_after));
        }
        let budget_omitted_before = frame_start - requested_frame_start;
        let budget_omitted_after = requested_frame_end - frame_end;
        if budget_omitted_before > 0 || budget_omitted_after > 0 {
            row.insert("budget_omitted_before".into(), json!(budget_omitted_before));
            row.insert("budget_omitted_after".into(), json!(budget_omitted_after));
        }
        rows.push(Value::Object(row));
    }
    if window_cropped_paths > 0 {
        truncation_reasons.push(json!({
            "kind":"frame_window",
            "mode":match window {
                Some(FrameWindow::Head { .. })=>"head",
                Some(FrameWindow::Tail { .. })=>"tail",
                Some(FrameWindow::AroundTarget { .. })=>"around_target",
                None=>"none"
            },
            "cropped_paths":window_cropped_paths,
            "total_omitted_before":window_omitted_before,
            "total_omitted_after":window_omitted_after
        }));
    }
    let budget_omitted = budget_available - budget_returned;
    let total_frame_budget = json!({
        "limit":max_total_frames,
        "available":budget_available,
        "returned":budget_returned,
        "omitted":budget_omitted,
        "selected_paths":selected_paths,
        "returned_paths":rows.len(),
        "omitted_paths":selected_paths-rows.len(),
        "cropped_paths":budget_cropped_paths,
    });
    if budget_omitted > 0 {
        let mut reason = total_frame_budget
            .as_object()
            .expect("static object")
            .clone();
        reason.insert("kind".into(), Value::String("total_frame_budget".into()));
        truncation_reasons.push(Value::Object(reason));
    }
    Ok(envelope(
        profile,
        scope,
        truncation_reasons,
        Vec::new(),
        json!({"through":frame,"paths":rows,"total_frame_budget":total_frame_budget}),
    ))
}

fn check_frame_window(window: Option<FrameWindow>) -> Result<(), ApiError> {
    match window {
        None => Ok(()),
        Some(FrameWindow::Head { lines }) | Some(FrameWindow::Tail { lines }) => {
            check_limit(lines, 1, 4096, "frame_window.lines")
        }
        Some(FrameWindow::AroundTarget { before, after }) => {
            if before > 4096 || after > 4096 || (before == 0 && after == 0) {
                return Err(ApiError::new(
                    "invalid_budget",
                    "frame_window around_target requires before/after <= 4096 and at least one non-zero value",
                    json!({"before":before,"after":after}),
                    "Use a bounded non-empty display window.",
                ));
            }
            Ok(())
        }
    }
}

fn frame_window_range(
    total_depth: usize,
    positions: &[usize],
    window: Option<FrameWindow>,
) -> (usize, usize) {
    match window {
        None => (0, total_depth),
        Some(FrameWindow::Head { lines }) => (0, lines.min(total_depth)),
        Some(FrameWindow::Tail { lines }) => (total_depth.saturating_sub(lines), total_depth),
        Some(FrameWindow::AroundTarget { before, after }) => {
            let first = *positions.first().expect("selected frame occurs in stack");
            let last = *positions.last().expect("selected frame occurs in stack");
            (
                first.saturating_sub(before),
                (last.saturating_add(after).saturating_add(1)).min(total_depth),
            )
        }
    }
}

fn budget_range(
    requested_start: usize,
    requested_end: usize,
    positions: &[usize],
    window: Option<FrameWindow>,
    remaining: usize,
) -> (usize, usize) {
    debug_assert!(remaining > 0);
    debug_assert!(requested_end - requested_start > remaining);
    match window {
        Some(FrameWindow::Head { .. }) => (requested_start, requested_start + remaining),
        Some(FrameWindow::Tail { .. }) => (requested_end - remaining, requested_end),
        None | Some(FrameWindow::AroundTarget { .. }) => {
            let anchor = *positions.first().expect("selected frame occurs in stack");
            let max_start = requested_end - remaining;
            let centered_start = anchor.saturating_sub(remaining / 2);
            let start = centered_start.clamp(requested_start, max_start);
            (start, start + remaining)
        }
    }
}

fn frame_sequence(profile: &Profile, frames: &[FrameId]) -> Vec<String> {
    frames
        .iter()
        .map(|id| profile.frame_name(*id).to_owned())
        .collect()
}

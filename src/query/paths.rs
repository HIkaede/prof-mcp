use crate::output::{
    FrameWindowMode, PathRow, PathsData, PathsOutput, ResponseBudget, TotalFrameBudget,
};
use serde_json::json;

use super::{
    ApiError, FrameSelector, FrameWindow, MAX_TOTAL_FRAMES, Profile, TruncationReason, check_limit,
    display_percent, envelope, resolve_selector,
};

pub fn paths_with_window_budget<'p>(
    profile: &'p Profile,
    selector: &FrameSelector,
    limit: usize,
    window: Option<FrameWindow>,
    max_total_frames: usize,
) -> Result<PathsOutput<'p>, ApiError> {
    check_limit(limit, 1, 50, "limit")?;
    check_frame_window(window)?;
    check_limit(max_total_frames, 1, MAX_TOTAL_FRAMES, "max_total_frames")?;
    let frame = resolve_selector(profile, selector)?;
    let stack_ids = profile.frame_to_stacks.ranked(frame as usize);
    let scope = profile.frame_stats[frame as usize].inclusive_weight;
    let available = stack_ids.len();
    let stacks = &stack_ids[..limit.min(stack_ids.len())];
    let mut truncation_reasons = Vec::new();
    let selected_paths = stacks.len();
    let mut window_cropped_paths = 0usize;
    let mut window_omitted_before = 0usize;
    let mut window_omitted_after = 0usize;
    let mut budget_remaining = max_total_frames;
    let mut budget_available = 0usize;
    let mut budget_returned = 0usize;
    let mut budget_cropped_paths = 0usize;
    let mut rows = Vec::new();
    let mut bytes = ResponseBudget::new();
    for posting in stacks {
        let stack = profile.stacks.stack(*posting);
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
            budget_cropped_paths += 1;
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
        bytes.json(&positions)?;
        bytes.json(&display_target_positions)?;
        bytes.charge(64)?;
        let frames = stack.frames[frame_start..frame_end]
            .iter()
            .map(|id| {
                let name = profile.frame_name(*id);
                bytes.json(&name)?;
                Ok(name)
            })
            .collect::<Result<_, ApiError>>()?;
        let cropped = requested_frame_start != frame_start || requested_frame_end != frame_end;
        rows.push(PathRow {
            frames,
            weight: stack.weight,
            profile_percent: display_percent(stack.weight, profile.total_weight),
            target_positions: positions,
            display_target_positions,
            total_depth,
            requested_frame_start: if window.is_some() || cropped {
                requested_frame_start
            } else {
                0
            },
            requested_frame_end: if window.is_some() || cropped {
                requested_frame_end
            } else {
                0
            },
            frame_start,
            frame_end,
            omitted_before: frame_start,
            omitted_after: total_depth - frame_end,
            budget_omitted_before: frame_start - requested_frame_start,
            budget_omitted_after: requested_frame_end - frame_end,
        });
    }
    // `returned` counts rows that survive the total-frame budget, so the reason
    // never claims more rows than `data.paths` holds. The reason itself is only
    // about selection: budget omissions are reported by `total_frame_budget`.
    if available > limit {
        truncation_reasons.push(TruncationReason::RowLimit {
            limit,
            returned: rows.len(),
            available,
            omitted: available - rows.len(),
        });
    }
    if let Some(window) = window.filter(|_| window_cropped_paths > 0) {
        truncation_reasons.push(TruncationReason::FrameWindow {
            mode: match window {
                FrameWindow::Head { .. } => FrameWindowMode::Head,
                FrameWindow::Tail { .. } => FrameWindowMode::Tail,
                FrameWindow::AroundTarget { .. } => FrameWindowMode::AroundTarget,
            },
            cropped_paths: window_cropped_paths,
            total_omitted_before: window_omitted_before,
            total_omitted_after: window_omitted_after,
        });
    }
    let budget_omitted = budget_available - budget_returned;
    let total_frame_budget = TotalFrameBudget {
        limit: max_total_frames,
        available: budget_available,
        returned: budget_returned,
        omitted: budget_omitted,
        selected_paths,
        returned_paths: rows.len(),
        omitted_paths: selected_paths - rows.len(),
        cropped_paths: budget_cropped_paths,
    };
    if budget_omitted > 0 {
        truncation_reasons.push(TruncationReason::TotalFrameBudget(
            total_frame_budget.clone(),
        ));
    }
    Ok(envelope(
        profile,
        scope,
        truncation_reasons,
        Vec::new(),
        PathsData {
            frame,
            paths: rows,
            total_frame_budget,
        },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_recursive_windows() {
        for (window, expected) in [
            (None, (0, 6)),
            (Some(FrameWindow::Head { lines: 2 }), (0, 2)),
            (Some(FrameWindow::Tail { lines: 2 }), (4, 6)),
            (Some(FrameWindow::Head { lines: 4096 }), (0, 6)),
            (Some(FrameWindow::Tail { lines: 4096 }), (0, 6)),
            (
                Some(FrameWindow::AroundTarget {
                    before: 1,
                    after: 1,
                }),
                (0, 6),
            ),
            (
                Some(FrameWindow::AroundTarget {
                    before: 0,
                    after: 1,
                }),
                (1, 6),
            ),
        ] {
            assert_eq!(frame_window_range(6, &[1, 4], window), expected);
        }
    }

    #[test]
    fn crop_to_frame_budget() {
        for (window, expected) in [
            (Some(FrameWindow::Head { lines: 8 }), (1, 4)),
            (Some(FrameWindow::Tail { lines: 8 }), (6, 9)),
            (None, (4, 7)),
            (
                Some(FrameWindow::AroundTarget {
                    before: 4,
                    after: 3,
                }),
                (4, 7),
            ),
        ] {
            assert_eq!(budget_range(1, 9, &[5], window, 3), expected);
        }
        assert_eq!(budget_range(1, 9, &[1], None, 3), (1, 4));
        assert_eq!(budget_range(1, 9, &[8], None, 3), (6, 9));
    }
}

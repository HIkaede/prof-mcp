use crate::{
    error::ApiError,
    output::{ResponseBudget, response_too_large},
    profile::{FrameId, StackId},
    query::Cancellation,
};

use super::{
    Completeness, Projection, Row, Scope, Table, Totals, Truncation, View,
    facts::Facts,
    plan::PathWindow,
    project::{Calculation, ContextRow, ResolvedAnchor, Rows},
    result::{Share, share},
};

pub(super) fn render(
    facts: &Facts<'_, '_>,
    view: &View,
    calculation: Calculation,
    offset: usize,
    bytes: &mut ResponseBudget,
    allow_empty_page: bool,
    cancellation: Cancellation<'_>,
) -> Result<Projection, ApiError> {
    let Calculation {
        rows,
        additive,
        matched,
        filtered,
        coverage,
        anchor,
        aggregate,
    } = calculation;
    let count = rows.len(facts.profile);
    let mut completeness = Completeness {
        query_exact: true,
        rows_complete: true,
        paths_complete: true,
    };
    let mut truncation = Vec::new();
    let mut table = None;
    let mut returned_weight = 0;
    if let Some(limit) = view.limit() {
        let mut output = Vec::new();
        let mut omitted_frames = 0;
        let mut cropped_paths = 0;
        let mut bytes_exceeded = false;
        let window = DisplayWindow::new(view, facts)?;
        for index in offset..count.min(offset.saturating_add(limit)) {
            cancellation.check()?;
            let restore = bytes.request_remaining();
            match render_row(
                &rows,
                facts,
                index,
                anchor.as_ref().map(|anchor| anchor.weight),
                &window,
                bytes,
            ) {
                Ok((row, omitted)) => {
                    omitted_frames += omitted;
                    cropped_paths += usize::from(omitted > 0);
                    output.push(row);
                    if additive {
                        returned_weight += rows.weight(facts.profile, index);
                    }
                }
                Err(error)
                    if error.code == "query_too_large"
                        && (allow_empty_page || !output.is_empty()) =>
                {
                    bytes.restore(restore);
                    bytes_exceeded = true;
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        let returned = output.len();
        completeness.rows_complete = returned == count;
        completeness.paths_complete = cropped_paths == 0;
        if offset > 0 || count > limit {
            truncation.push(Truncation::RowLimit {
                limit,
                available: count,
                returned,
                offset,
            });
        }
        if bytes_exceeded {
            truncation.push(Truncation::ResponseBytes {
                available: count,
                returned,
                offset,
            });
        }
        if cropped_paths > 0 {
            truncation.push(Truncation::FrameWindow {
                cropped_paths,
                omitted_frames,
            });
        }
        table = Some(Table {
            columns: columns(&rows),
            rows: output,
        });
    }
    let totals = if matches!(
        view,
        View::Frames { totals: false, .. } | View::Aggregate { .. }
    ) {
        None
    } else {
        Some(Totals {
            rows_additive: additive,
            matched_weight: matched,
            returned_weight: additive.then_some(returned_weight),
            omitted_weight: matched.map(|matched| matched - returned_weight),
            filtered_weight: filtered,
            stack_union_weight: coverage,
        })
    };
    let stat = if let View::Frames { stat, .. } = view {
        Some(*stat)
    } else {
        None
    };
    let path_range = match view {
        View::Frames { path_range, .. } | View::Aggregate { path_range, .. } => Some(*path_range),
        _ => None,
    };
    cancellation.check()?;
    Ok(Projection {
        kind: view.kind(),
        stat,
        path_range,
        anchor,
        completeness,
        totals,
        table,
        aggregate,
        truncation,
        cursor: None,
        resume_view: None,
    })
}

fn columns(rows: &Rows) -> &'static [&'static str] {
    match rows {
        Rows::Frames(_) | Rows::RankedSelf(_) => &[
            "frame_id",
            "name",
            "weight",
            "scope_percent",
            "profile_percent",
        ],
        Rows::Relations(_) => &[
            "frame_id",
            "name",
            "weight",
            "scope_percent",
            "profile_percent",
            "anchor_percent",
        ],
        Rows::Contexts(_) => &[
            "context",
            "frame_id",
            "name",
            "depth",
            "self_weight",
            "total_weight",
            "scope_percent",
            "profile_percent",
        ],
        Rows::Children(_) => &[
            "context",
            "frame_id",
            "name",
            "depth",
            "self_weight",
            "total_weight",
            "scope_percent",
            "profile_percent",
            "anchor_percent",
        ],
        Rows::Paths(_) | Rows::RankedPaths => &[
            "stack_id",
            "frames",
            "weight",
            "scope_percent",
            "profile_percent",
            "frame_start",
            "frame_end",
            "path_depth",
            "target_position",
        ],
        Rows::None => &[],
    }
}

fn render_row(
    rows: &Rows,
    facts: &Facts<'_, '_>,
    index: usize,
    anchor_weight: Option<u64>,
    window: &DisplayWindow,
    bytes: &mut ResponseBudget,
) -> Result<(Row, usize), ApiError> {
    let weight = rows.weight(facts.profile, index);
    let scope = facts.scope;
    let (selected, total) = shares(weight, scope);
    let relation = matches!(rows, Rows::Relations(_));
    let result = match rows {
        Rows::Frames(rows) | Rows::Relations(rows) => {
            let (frame, _) = rows[index];
            let borrowed = facts.profile.frame_name(frame);
            bytes.charge(72)?;
            bytes.embedded_json(&borrowed)?;
            if let Some(anchor) = anchor_weight.filter(|_| relation) {
                Row::Relation(
                    frame,
                    borrowed.to_owned(),
                    weight,
                    selected,
                    total,
                    share(weight, anchor),
                )
            } else {
                Row::Frame(frame, borrowed.to_owned(), weight, selected, total)
            }
        }
        Rows::RankedSelf(_) => {
            let frame = facts.profile.top_self()[index];
            let name = facts.profile.frame_name(frame);
            bytes.charge(72)?;
            bytes.embedded_json(&name)?;
            Row::Frame(frame, name.to_owned(), weight, selected, total)
        }
        Rows::Contexts(rows) | Rows::Children(rows) => {
            let row = &rows[index];
            render_context(facts, row, anchor_weight, selected, total, bytes)?
        }
        Rows::Paths(_) | Rows::RankedPaths => {
            return render_path(
                facts,
                rows.path_id(facts.profile, index),
                weight,
                window,
                bytes,
            );
        }
        Rows::None => return Err(response_too_large()),
    };
    Ok((result, 0))
}

fn render_context(
    facts: &Facts<'_, '_>,
    row: &ContextRow,
    anchor_weight: Option<u64>,
    selected: Share,
    total: Share,
    bytes: &mut ResponseBudget,
) -> Result<Row, ApiError> {
    let name = facts.profile.frame_name(row.frame);
    bytes.charge(128)?;
    bytes.embedded_json(&name)?;
    Ok(if let Some(anchor) = anchor_weight {
        Row::Child(
            row.context,
            row.frame,
            name.to_owned(),
            row.depth,
            row.own,
            row.weight,
            selected,
            total,
            share(row.weight, anchor),
        )
    } else {
        Row::Context(
            row.context,
            row.frame,
            name.to_owned(),
            row.depth,
            row.own,
            row.weight,
            selected,
            total,
        )
    })
}

fn shares(weight: u64, scope: Scope) -> (Share, Share) {
    (
        share(weight, scope.selected_weight),
        share(weight, scope.profile_weight),
    )
}

enum DisplayWindow {
    Head(usize),
    Tail(usize),
    Around {
        target: ResolvedAnchor,
        before: usize,
        after: usize,
    },
    Full,
}

impl DisplayWindow {
    fn new(view: &View, facts: &Facts<'_, '_>) -> Result<Self, ApiError> {
        Ok(match view {
            View::Paths {
                window: PathWindow::Head { frames },
                ..
            } => Self::Head(*frames),
            View::Paths {
                window: PathWindow::Tail { frames },
                ..
            } => Self::Tail(*frames),
            View::Paths {
                window:
                    PathWindow::AroundTarget {
                        target,
                        before,
                        after,
                    },
                ..
            } => Self::Around {
                target: ResolvedAnchor::from_input(facts.profile, target)?,
                before: *before,
                after: *after,
            },
            _ => Self::Full,
        })
    }

    fn range(
        &self,
        facts: &Facts<'_, '_>,
        id: StackId,
        frames: &[FrameId],
    ) -> (usize, usize, Option<usize>) {
        let depth = frames.len();
        match *self {
            Self::Head(count) => (0, count.min(depth), None),
            Self::Tail(count) => (depth.saturating_sub(count), depth, None),
            Self::Full => (0, depth, None),
            Self::Around {
                ref target,
                before,
                after,
            } => {
                let at = match *target {
                    ResolvedAnchor::Frame(frame) => frames.iter().rposition(|id| *id == frame),
                    ResolvedAnchor::Context(context) => facts
                        .profile
                        .context_range(context)
                        .filter(|range| range.contains(&id))
                        .and_then(|_| facts.profile.context_depth(context))
                        .and_then(|depth| depth.checked_sub(1)),
                };
                at.map_or(
                    (depth.saturating_sub(before + 1 + after), depth, None),
                    |at| {
                        (
                            at.saturating_sub(before),
                            depth.min(at + after + 1),
                            Some(at),
                        )
                    },
                )
            }
        }
    }
}

fn render_path(
    facts: &Facts<'_, '_>,
    id: StackId,
    weight: u64,
    window: &DisplayWindow,
    bytes: &mut ResponseBudget,
) -> Result<(Row, usize), ApiError> {
    let frames = facts.profile.stacks().stack(id).frames;
    let (start, end, target) = window.range(facts, id, frames);
    let (selected, total) = shares(weight, facts.scope);
    bytes.charge(106)?;
    let names = frames[start..end]
        .iter()
        .map(|&frame| {
            let name = facts.profile.frame_name(frame);
            bytes.charge(16)?;
            bytes.embedded_json(&name)?;
            Ok((frame, name.to_owned()))
        })
        .collect::<Result<_, ApiError>>()?;
    let cropped = frames.len() - (end - start);
    Ok((
        Row::Path(
            id,
            names,
            weight,
            selected,
            total,
            start,
            end,
            frames.len(),
            target,
        ),
        cropped,
    ))
}

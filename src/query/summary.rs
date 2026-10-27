//! Workspace-level orientation: totals, shape, and hot frames.

use crate::error::ApiError;
use crate::output::{
    RecursionRow, RegistryOutput, ResponseBudget, StackConcentration, SummaryData, SummaryOutput,
    SymbolCoverage,
};

use super::{
    Profile, TruncationReason, display_percent, envelope, frame_row, frame_row_with_percent_weight,
};

/// Rows returned per summary ranking; extra rows are reported as truncated.
const SUMMARY_ROWS: usize = 5;

pub fn summary<'p>(profile: &'p Profile) -> Result<SummaryOutput<'p>, ApiError> {
    let mut bytes = ResponseBudget::new();
    let top_self: Vec<_> = profile
        .top_self
        .iter()
        .take(SUMMARY_ROWS)
        .map(|id| {
            bytes.row_name(profile.frame_name(*id))?;
            Ok(frame_row_with_percent_weight(
                profile,
                *id,
                &profile.frame_stats[*id as usize],
                profile.total_weight,
                profile.frame_stats[*id as usize].self_weight,
            ))
        })
        .collect::<Result<_, ApiError>>()?;
    let top_inclusive: Vec<_> = profile
        .top_inclusive
        .iter()
        .take(SUMMARY_ROWS)
        .map(|id| {
            bytes.row_name(profile.frame_name(*id))?;
            Ok(frame_row(
                profile,
                *id,
                &profile.frame_stats[*id as usize],
                profile.total_weight,
            ))
        })
        .collect::<Result<_, ApiError>>()?;
    let stack_concentration = stack_concentration(profile);
    let recursion = recursion_report(profile, &mut bytes)?;
    let mut truncation_reasons = Vec::new();
    for (available, returned) in [
        (profile.top_self.len(), top_self.len()),
        (profile.top_inclusive.len(), top_inclusive.len()),
        (profile.recursive_frames.len(), recursion.len()),
    ] {
        if available > returned {
            truncation_reasons.push(TruncationReason::RowLimit {
                limit: SUMMARY_ROWS,
                returned,
                available,
                omitted: available - returned,
            });
        }
    }
    let symbol_coverage = symbol_coverage(profile);
    let mut warnings = vec!["Weight unit is opaque; it is not assumed to be time or cycles."];
    if !recursion.is_empty() {
        warnings.push(
            "Recursive frames detected; exact-frame inclusive weights count each stack once. Context occurrences can overlap.",
        );
    }
    Ok(envelope(
        profile,
        profile.total_weight,
        truncation_reasons,
        warnings,
        SummaryData {
            total_weight: profile.total_weight,
            frame_count: profile.frames.len(),
            unique_stack_count: profile.stacks.len(),
            max_depth: profile.max_depth,
            stack_concentration,
            symbol_coverage,
            recursion_detected: recursion,
            top_self,
            top_inclusive,
            registry: RegistryOutput {
                registry_root: String::new(),
                active: String::new(),
                profile_count: 0,
                profiles: Vec::new(),
            },
        },
    ))
}

/// Frames perf could not resolve to a symbol, weighted by self time. Inclusive
/// weights would double count, and overlapping contexts already exceed 100%.
fn symbol_coverage(profile: &Profile) -> SymbolCoverage {
    let mut unresolved_frames = 0;
    let mut unresolved_self = 0_u64;
    for index in 0..profile.frames.len() {
        if !profile.frame_name(index as u32).starts_with("[unknown@") {
            continue;
        }
        unresolved_frames += 1;
        unresolved_self = unresolved_self.saturating_add(profile.frame_stats[index].self_weight);
    }
    SymbolCoverage {
        unresolved_frames,
        frame_count: profile.frames.len(),
        unresolved_self_percent: display_percent(unresolved_self, profile.total_weight),
    }
}

/// Weight share covered by the heaviest unique stacks.
fn stack_concentration(profile: &Profile) -> StackConcentration {
    StackConcentration {
        top_10_stacks_percent: display_percent(
            profile.heaviest_stack_weights[0],
            profile.total_weight,
        ),
        top_50_stacks_percent: display_percent(
            profile.heaviest_stack_weights[1],
            profile.total_weight,
        ),
        unique_stack_count: profile.stacks.len(),
    }
}

/// Frames that occur more than once within a single contributing stack.
fn recursion_report<'p>(
    profile: &'p Profile,
    bytes: &mut ResponseBudget,
) -> Result<Vec<RecursionRow<'p>>, ApiError> {
    profile
        .recursive_frames
        .iter()
        .take(SUMMARY_ROWS)
        .map(|row| {
            bytes.row_name(profile.frame_name(row.frame))?;
            Ok(RecursionRow {
                frame_id: row.frame,
                name: profile.frame_name(row.frame),
                max_occurrences_per_stack: row.max_occurrences as usize,
                affected_weight: row.affected_weight,
                affected_weight_percent: display_percent(row.affected_weight, profile.total_weight),
            })
        })
        .collect()
}

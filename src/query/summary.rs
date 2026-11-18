//! Workspace-level orientation: totals, shape, and hot frames.

use crate::error::ApiError;
use crate::output::{
    FoldedCapabilities, FoldedQuality, RecursionRow, RegistryOutput, ResponseBudget,
    StackConcentration, SummaryData, SummaryOutput, SymbolCoverage,
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
            snapshot_id: profile.snapshot_id(),
            capabilities: FoldedCapabilities {
                storage: "folded_aggregate",
                metric: "opaque_weight",
                retained_dimensions: &["ordered_frame_labels", "stack_weight"],
                queryable_dimensions: &["frame", "context", "stack"],
                unavailable: &[
                    "timestamp",
                    "thread",
                    "cpu",
                    "sample_count",
                    "sample_period",
                    "call_count",
                    "off_cpu_duration",
                    "request_latency",
                ],
            },
            quality: FoldedQuality {
                lost_samples: None,
                lost_records: None,
                known_truncated_weight: None,
                unwind_failed_weight: None,
                source_dimensions: None,
                discarded_dimensions: None,
            },
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

/// Recognize the generic marker and the capture adapter's unresolved PC syntax.
fn unknown_label(label: &str) -> bool {
    if !label.starts_with("[unknown") {
        return false;
    }
    let name = match label.split_once(" [") {
        Some((name, module)) if module.ends_with(']') && module.len() > 1 => name,
        _ => label,
    };
    name == "[unknown]"
        || name
            .strip_prefix("[unknown@0x")
            .and_then(|pc| pc.strip_suffix(']'))
            .is_some_and(|pc| !pc.is_empty() && pc.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn symbol_coverage(profile: &Profile) -> SymbolCoverage {
    let &(unresolved_frames, unresolved_self, covered) =
        profile.unknown_coverage.get_or_init(|| {
            let mut unresolved_frames = 0;
            let mut unresolved_self = 0;
            let mut covered = 0;
            for (i, name) in profile.frames.iter().enumerate() {
                if unknown_label(name) {
                    unresolved_frames += 1;
                    unresolved_self += profile.frame_stats[i].self_weight;
                    covered = profile.frame_stats[i].inclusive_weight;
                }
            }
            if unresolved_frames > 1 {
                let unresolved: Vec<_> = profile.frames.iter().map(unknown_label).collect();
                covered = profile
                    .stacks
                    .iter()
                    .filter(|stack| stack.frames.iter().any(|&id| unresolved[id as usize]))
                    .map(|stack| stack.weight)
                    .sum();
            }
            (unresolved_frames, unresolved_self, covered)
        });
    SymbolCoverage {
        unresolved_frames,
        frame_count: profile.frames.len(),
        unresolved_self_weight: unresolved_self,
        unresolved_self_percent: display_percent(unresolved_self, profile.total_weight),
        unresolved_stack_weight: covered,
        unresolved_stack_percent: (profile.total_weight > 0)
            .then(|| display_percent(covered, profile.total_weight)),
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

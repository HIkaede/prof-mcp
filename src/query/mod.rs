//! Shared selector types, budgets, and rendering state for profile queries.
//!
//! Each tool family lives in its own submodule: [`summary`], [`symbols`],
//! [`top`], [`tree`], [`direction`] (callers/callees), [`paths`], and [`diff`].

use std::{cmp::Ordering, collections::BTreeMap};

use regex::{Regex, RegexBuilder};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::ApiError;
pub(crate) use crate::output::{
    TreeTruncation, TruncationReason, frame_row, frame_row_with_percent_weight, percent,
    typed_envelope as envelope,
};
pub(crate) use crate::profile::{FrameId, FrameStats, NodeId, Profile};

mod diff;
mod direction;
mod paths;
mod summary;
mod symbols;
mod top;
mod tree;

mod legacy;
pub use legacy::*;

pub(crate) mod typed {
    pub(crate) use super::diff::diff;
    pub(crate) use super::direction::{callees, callers};
    pub(crate) use super::paths::paths_with_window_budget;
    pub(crate) use super::summary::summary;
    pub(crate) use super::symbols::find_symbols;
    pub(crate) use super::top::top;
    pub(crate) use super::tree::tree;
}

#[derive(Clone, Debug, Default)]
pub struct FrameSelector {
    pub frame_id: Option<u32>,
    pub frame_name: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub enum TopSort {
    #[serde(rename = "self")]
    SelfWeight,
    #[serde(rename = "inclusive")]
    Inclusive,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DiffSort {
    Regression,
    Improvement,
    Absolute,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MatchMode {
    Contains,
    Regex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameWindow {
    Head { lines: usize },
    Tail { lines: usize },
    AroundTarget { before: usize, after: usize },
}

pub const DEFAULT_MAX_TOTAL_FRAMES: usize = 500;
pub const MAX_TOTAL_FRAMES: usize = 5_000;

pub(crate) fn resolve_selector(
    profile: &Profile,
    selector: &FrameSelector,
) -> Result<FrameId, ApiError> {
    match (&selector.frame_id, &selector.frame_name) {
        (Some(_), Some(_)) | (None, None) => Err(ApiError::new(
            "invalid_frame_selector",
            "Frame selector must set exactly one of frame_id or frame_name",
            json!({}),
            "Use an exact frame_id from profile_find_symbols or one exact frame_name.",
        )),
        (Some(id), None) => {
            if (*id as usize) < profile.frames.len() {
                Ok(*id)
            } else {
                Err(ApiError::new(
                    "frame_not_found",
                    format!("Frame id not found: {id}"),
                    json!({"frame_id":id}),
                    "Call profile_find_symbols to discover valid frame ids.",
                ))
            }
        }
        (None, Some(name)) => profile.frame_id(name).ok_or_else(|| {
            ApiError::new(
                "frame_not_found",
                format!("Frame not found: {name}"),
                json!({"frame_name":name}),
                "Call profile_find_symbols to discover exact frame names.",
            )
        }),
    }
}
pub(crate) fn compile_regex(pattern: &str) -> Result<Regex, ApiError> {
    if pattern.len() > 4096 {
        return Err(ApiError::new(
            "invalid_regex",
            "Regex exceeds 4 KiB limit",
            json!({"length":pattern.len()}),
            "Use a shorter regex.",
        ));
    }
    RegexBuilder::new(pattern)
        .size_limit(10 * 1024 * 1024)
        .dfa_size_limit(2 * 1024 * 1024)
        .build()
        .map_err(|error| {
            ApiError::new(
                "invalid_regex",
                format!("Invalid regex: {error}"),
                json!({"pattern":pattern}),
                "Simplify the regex or fix its syntax and retry.",
            )
        })
}
pub(crate) fn check_limit(
    value: usize,
    min: usize,
    max: usize,
    field: &str,
) -> Result<(), ApiError> {
    if value < min || value > max {
        Err(ApiError::new(
            "invalid_budget",
            format!("{field} must be between {min} and {max}"),
            json!({field:value,"min":min,"max":max}),
            "Use a documented output budget.",
        ))
    } else {
        Ok(())
    }
}
pub(crate) fn check_budget(depth: usize, nodes: usize, percent: f64) -> Result<(), ApiError> {
    check_limit(depth, 0, 16, "max_depth")?;
    check_limit(nodes, 1, 512, "max_nodes")?;
    if !(0.0..=100.0).contains(&percent) || !percent.is_finite() {
        Err(ApiError::new(
            "invalid_budget",
            "min_scope_percent must be finite and between 0 and 100",
            json!({"min_scope_percent":percent}),
            "Use a documented tree budget.",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn row_limit_reason(limit: usize, available: usize) -> Vec<TruncationReason> {
    if available > limit {
        vec![TruncationReason::RowLimit {
            limit,
            returned: limit,
            available,
            omitted: available - limit,
        }]
    } else {
        Vec::new()
    }
}

pub(crate) fn tree_reason_values(
    stats: &BTreeMap<TreeTruncation, TruncationStats>,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
) -> Vec<TruncationReason> {
    stats
        .iter()
        .map(|(kind, stats)| match kind {
            TreeTruncation::DepthLimit => TruncationReason::DepthLimit {
                max_depth,
                omitted_children: stats.count,
                omitted_weight: stats.weight,
            },
            TreeTruncation::MinScopePercent => TruncationReason::MinScopePercent {
                threshold: min_scope_percent,
                omitted_children: stats.count,
                omitted_weight: stats.weight,
            },
            TreeTruncation::NodeBudget => TruncationReason::NodeBudget {
                max_nodes,
                omitted_children: stats.count,
                omitted_weight: stats.weight,
            },
        })
        .collect()
}

pub(crate) fn frame_order_stats(
    profile: &Profile,
    stats: &[FrameStats],
    a: FrameId,
    b: FrameId,
    sort: TopSort,
) -> Ordering {
    let aw = metric_weight(&stats[a as usize], sort);
    let bw = metric_weight(&stats[b as usize], sort);
    bw.cmp(&aw)
        .then_with(|| {
            stats[b as usize]
                .self_weight
                .cmp(&stats[a as usize].self_weight)
        })
        .then_with(|| profile.frame_name(a).cmp(profile.frame_name(b)))
        .then(a.cmp(&b))
}
pub(crate) fn metric_weight(stats: &FrameStats, sort: TopSort) -> u64 {
    match sort {
        TopSort::SelfWeight => stats.self_weight,
        TopSort::Inclusive => stats.inclusive_weight,
    }
}

pub(crate) struct RenderState<'a, C> {
    pub(crate) scope: u64,
    pub(crate) max_depth: usize,
    pub(crate) min_percent: f64,
    pub(crate) budget: &'a mut usize,
    pub(crate) reason_stats: &'a mut BTreeMap<TreeTruncation, TruncationStats>,
    pub(crate) continuations: &'a mut Vec<C>,
    pub(crate) continuation_count: &'a mut usize,
    pub(crate) continuation_limit: usize,
    /// Frame-id path from the rendered root; maintained by [`render_temp`]
    /// so omitted direction subtrees can be addressed by later requests.
    pub(crate) frame_path: Vec<FrameId>,
    pub(crate) bytes: crate::output::ResponseBudget,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct TruncationStats {
    pub(crate) count: usize,
    pub(crate) weight: u64,
}

impl<C> RenderState<'_, C> {
    pub(crate) fn note_truncation(&mut self, reason: TreeTruncation, weight: u64) {
        let stats = self.reason_stats.entry(reason).or_default();
        stats.count = stats.count.saturating_add(1);
        stats.weight = stats.weight.saturating_add(weight);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_tree_budgets() {
        for (depth, nodes, percent) in [(0, 1, 0.0), (16, 512, 100.0)] {
            check_budget(depth, nodes, percent).unwrap();
        }
        for (depth, nodes, percent) in [
            (17, 1, 0.0),
            (0, 0, 0.0),
            (0, 513, 0.0),
            (0, 1, -0.1),
            (0, 1, 100.1),
            (0, 1, f64::NAN),
            (0, 1, f64::INFINITY),
            (0, 1, f64::NEG_INFINITY),
        ] {
            assert_eq!(
                check_budget(depth, nodes, percent).unwrap_err().code,
                "invalid_budget"
            );
        }
    }

    #[test]
    fn rank_tied_frames() {
        use crate::profile::{BuildLimits, ProfileBuilder};
        let input = b"z 5\na 5\nb;leaf 5\n";
        let profile = ProfileBuilder::new(BuildLimits::default())
            .from_reader(
                std::io::Cursor::new(input),
                "/ranking.folded".into(),
                input.len() as u64,
                None,
            )
            .unwrap();
        let id = |name| profile.frame_id(name).unwrap();
        assert_eq!(
            frame_order_stats(
                &profile,
                &profile.frame_stats,
                id("a"),
                id("z"),
                TopSort::SelfWeight
            ),
            Ordering::Less
        );
        assert_eq!(
            frame_order_stats(
                &profile,
                &profile.frame_stats,
                id("z"),
                id("b"),
                TopSort::Inclusive
            ),
            Ordering::Less
        );
        assert_eq!(
            frame_order_stats(
                &profile,
                &profile.frame_stats,
                id("b"),
                id("z"),
                TopSort::Inclusive
            ),
            Ordering::Greater
        );
    }
}

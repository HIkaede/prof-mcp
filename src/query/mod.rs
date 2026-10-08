//! Shared selector types, budgets, and rendering state for profile queries.
//!
//! Each tool family lives in its own submodule: [`summary`], [`symbols`],
//! [`top`], [`tree`], [`direction`] (callers/callees), [`paths`], and [`diff`].

use std::{cmp::Ordering, collections::BTreeMap};

use regex::Regex;
use serde_json::{Value, json};

use crate::error::ApiError;
pub(crate) use crate::output::{envelope, frame_row, frame_row_with_percent_weight, percent};
pub(crate) use crate::profile::{ContextNode, FrameId, FrameStats, NodeId, Profile, StackRecord};

mod diff;
mod direction;
mod paths;
mod summary;
mod symbols;
mod top;
mod tree;

pub use diff::diff;
pub use direction::{callees, callers};
pub use paths::{paths, paths_with_window, paths_with_window_budget};
pub use summary::summary;
pub use symbols::find_symbols;
pub use top::top;
pub use tree::tree;

#[derive(Clone, Debug, Default)]
pub struct FrameSelector {
    pub frame_id: Option<u32>,
    pub frame_name: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopSort {
    SelfWeight,
    Inclusive,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffSort {
    Regression,
    Improvement,
    Absolute,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    Regex::new(pattern).map_err(|error| {
        ApiError::new(
            "invalid_regex",
            format!("Invalid regex: {error}"),
            json!({"pattern":pattern}),
            "Fix the regex syntax and retry.",
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

pub(crate) fn row_limit_reason(limit: usize, available: usize) -> Vec<Value> {
    if available > limit {
        vec![json!({
            "kind":"row_limit",
            "limit":limit,
            "returned":limit,
            "available":available,
            "omitted":available-limit
        })]
    } else {
        Vec::new()
    }
}

pub(crate) fn tree_reason_values(
    stats: &BTreeMap<&'static str, TruncationStats>,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
) -> Vec<Value> {
    stats
        .iter()
        .map(|(kind, stats)| match *kind {
            "depth_limit" => json!({"kind":"depth_limit","max_depth":max_depth,"omitted_children":stats.count,"omitted_weight":stats.weight}),
            "node_budget" => json!({"kind":"node_budget","max_nodes":max_nodes,"omitted_children":stats.count,"omitted_weight":stats.weight}),
            "min_scope_percent" => {
                json!({"kind":"min_scope_percent","threshold":min_scope_percent,"omitted_children":stats.count,"omitted_weight":stats.weight})
            }
            _ => json!({"kind":kind,"omitted_children":stats.count,"omitted_weight":stats.weight}),
        })
        .collect()
}

pub(crate) fn frame_order(profile: &Profile, a: FrameId, b: FrameId, sort: TopSort) -> Ordering {
    frame_order_stats(profile, &profile.frame_stats, a, b, sort)
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

/// Strip balanced `<...>` template arguments from a folded frame name.
///
/// Falls back to the original name when stripping would erase it entirely or
/// when brackets never rebalance (for example `operator<<`), so normalization
/// never invents empty symbols or merges distinct operators.
pub(crate) fn normalize_frame_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut depth = 0usize;
    for character in name.chars() {
        match character {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(character),
            _ => {}
        }
    }
    let trimmed = out.trim_end();
    if trimmed.is_empty() || depth != 0 {
        name.to_string()
    } else {
        trimmed.to_string()
    }
}

pub(crate) struct RenderState<'a> {
    pub(crate) scope: u64,
    pub(crate) max_depth: usize,
    pub(crate) min_percent: f64,
    pub(crate) budget: &'a mut usize,
    pub(crate) reason_stats: &'a mut BTreeMap<&'static str, TruncationStats>,
    pub(crate) continuations: &'a mut Vec<Value>,
    pub(crate) continuation_count: &'a mut usize,
    pub(crate) continuation_limit: usize,
    /// Frame-id path from the rendered root; maintained by [`render_temp`]
    /// so omitted direction subtrees can be addressed by later requests.
    pub(crate) frame_path: Vec<FrameId>,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct TruncationStats {
    pub(crate) count: usize,
    pub(crate) weight: u64,
}

impl RenderState<'_> {
    pub(crate) fn note_truncation(&mut self, reason: &'static str, weight: u64) {
        let stats = self.reason_stats.entry(reason).or_default();
        stats.count = stats.count.saturating_add(1);
        stats.weight = stats.weight.saturating_add(weight);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_keeps_operators_and_strips_nested_templates() {
        for (input, expected) in [
            ("make<A<B>, C>", "make"),
            ("make<T>::call<U>", "make::call"),
            ("operator<<", "operator<<"),
            ("<T>", "<T>"),
            ("unclosed<T", "unclosed<T"),
            ("plain", "plain"),
        ] {
            assert_eq!(normalize_frame_name(input), expected, "{input}");
        }
    }

    #[test]
    fn tree_budget_accepts_endpoints_and_rejects_nonfinite_percentages() {
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
    fn ranking_breaks_weight_ties_by_self_weight_then_name() {
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
            frame_order(&profile, id("a"), id("z"), TopSort::SelfWeight),
            Ordering::Less
        );
        assert_eq!(
            frame_order(&profile, id("z"), id("b"), TopSort::Inclusive),
            Ordering::Less
        );
        assert_eq!(
            frame_order(&profile, id("b"), id("z"), TopSort::Inclusive),
            Ordering::Greater
        );
    }
}

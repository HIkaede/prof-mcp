//! Compare measured weights inside two independently pinned folded selections.
use std::{cmp::Ordering, collections::BinaryHeap};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::output::{ResponseBudget, display_percent, percent, round_percent};
use crate::profile::{FrameId, FrameStats, Profile};

use super::engine::selected_frame_stats;
use super::selection::{Metric, SelectedFacts, Selection, Source};
use super::{Cancellation, DiffSort, TopSort, check_limit, compile_regex, metric_weight};

#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityPolicy {
    #[default]
    ExactFrameLabel,
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompareRequest {
    pub baseline: Source,
    pub candidate: Source,
    #[serde(default)]
    pub metric: Metric,
    #[serde(default = "default_stat")]
    pub stat: TopSort,
    #[serde(default = "default_order")]
    pub sort: DiffSort,
    #[serde(default)]
    pub identity_policy: IdentityPolicy,
    pub name_regex: Option<String>,
    #[serde(default = "default_limit")]
    #[schemars(range(min = 1, max = 200))]
    pub limit: usize,
}

fn default_stat() -> TopSort {
    TopSort::SelfWeight
}
fn default_order() -> DiffSort {
    DiffSort::Absolute
}
fn default_limit() -> usize {
    10
}

#[derive(Debug, JsonSchema, Serialize)]
pub struct Side {
    pub selection: Selection,
    pub scope_weight: u64,
}

#[derive(Debug, JsonSchema, Serialize)]
pub struct ComparisonRow {
    pub name: String,
    pub baseline_present: bool,
    pub candidate_present: bool,
    pub baseline_weight: u64,
    pub candidate_weight: u64,
    pub baseline_percent: Option<f64>,
    pub candidate_percent: Option<f64>,
    pub delta_pp: Option<f64>,
    pub relative_weight_change_percent: Option<f64>,
}

#[derive(Debug, JsonSchema, Serialize)]
pub struct Comparison {
    pub schema_version: &'static str,
    pub metric: Metric,
    pub stat: TopSort,
    pub sort: DiffSort,
    pub identity_policy: IdentityPolicy,
    pub baseline: Side,
    pub candidate: Side,
    pub matched_rows: usize,
    pub omitted_rows: usize,
    pub rows: Vec<ComparisonRow>,
}

// Keep the precomputed global table for full selections. Only scoped queries
// need the same per-selection aggregation used by the frames view.
enum Stats<'p> {
    Global(&'p [FrameStats]),
    Scoped(hashbrown::HashMap<FrameId, (FrameStats, u32)>),
}

impl<'p> Stats<'p> {
    fn new(
        profile: &'p Profile,
        facts: &SelectedFacts,
        cancel: Cancellation<'_>,
    ) -> Result<Self, ApiError> {
        if facts.len() == profile.stacks().len() {
            Ok(Self::Global(profile.frame_stats()))
        } else {
            Ok(Self::Scoped(selected_frame_stats(profile, facts, cancel)?))
        }
    }
    fn weight(&self, id: FrameId, stat: TopSort) -> Option<u64> {
        let entry = match self {
            Self::Global(rows) => rows.get(id as usize),
            Self::Scoped(rows) => rows.get(&id).map(|(stats, _)| stats),
        };
        entry.map(|stats| metric_weight(stats, stat))
    }
}

/// Merge the two lexically sorted dictionaries. Numeric IDs are local to each side.
fn aligned_frames<'a>(
    baseline: &'a Profile,
    candidate: &'a Profile,
) -> impl Iterator<Item = (&'a str, Option<u32>, Option<u32>)> {
    let mut left = baseline.frames.iter().enumerate().peekable();
    let mut right = candidate.frames.iter().enumerate().peekable();
    std::iter::from_fn(move || {
        let order = match (left.peek(), right.peek()) {
            (Some((_, a)), Some((_, b))) => a.cmp(b),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => return None,
        };
        let a = (order != Ordering::Greater).then(|| left.next().unwrap());
        let b = (order != Ordering::Less).then(|| right.next().unwrap());
        let (_, name) = a.or(b).unwrap();
        Some((name, a.map(|(id, _)| id as u32), b.map(|(id, _)| id as u32)))
    })
}

pub fn compare(
    baseline: &Profile,
    candidate: &Profile,
    request: &CompareRequest,
) -> Result<Comparison, ApiError> {
    compare_cancellable(baseline, candidate, request, Cancellation::default())
}

pub fn compare_cancellable(
    baseline: &Profile,
    candidate: &Profile,
    request: &CompareRequest,
    cancel: Cancellation<'_>,
) -> Result<Comparison, ApiError> {
    cancel.check()?;
    check_limit(request.limit, 1, 200, "limit")?;
    request.metric.validate(baseline)?;
    request.metric.validate(candidate)?;
    let regex = request
        .name_regex
        .as_deref()
        .map(compile_regex)
        .transpose()?;
    let a = request.baseline.selection(baseline)?;
    let b = request.candidate.selection(candidate)?;
    let af = a.evaluate_cancellable(baseline, cancel)?;
    let bf = b.evaluate_cancellable(candidate, cancel)?;
    let (at, bt) = (af.scope_weight(), bf.scope_weight());
    let ast = Stats::new(baseline, &af, cancel)?;
    let bst = Stats::new(candidate, &bf, cancel)?;
    let mut best = BinaryHeap::with_capacity(request.limit);
    let mut matched_rows = 0;
    for (index, (name, ai, bi)) in aligned_frames(baseline, candidate).enumerate() {
        cancel.checkpoint(index)?;
        let aw = ai.and_then(|id| ast.weight(id, request.stat));
        let bw = bi.and_then(|id| bst.weight(id, request.stat));
        if (aw.is_none() && bw.is_none()) || regex.as_ref().is_some_and(|re| !re.is_match(name)) {
            continue;
        }
        // Both denominators are constant across rows. Rank the exact
        // difference before rounding, even near the JSON-safe integer limit.
        let delta = i128::from(bw.unwrap_or(0)) * i128::from(at)
            - i128::from(aw.unwrap_or(0)) * i128::from(bt);
        let key = match request.sort {
            DiffSort::Regression => -delta,
            DiffSort::Improvement => delta,
            DiffSort::Absolute => -delta.abs(),
        };
        let row = (key, name, aw, bw);
        if best.len() < request.limit {
            best.push(row);
        } else if let Some(mut worst) = best.peek_mut()
            && row < *worst
        {
            *worst = row;
        }
        matched_rows += 1;
    }
    let mut budget = ResponseBudget::new();
    let mut rows = Vec::with_capacity(best.len());
    for (_, name, aw, bw) in best.into_sorted_vec() {
        budget.row_name(name)?;
        let (x, y) = (aw.unwrap_or(0), bw.unwrap_or(0));
        rows.push(ComparisonRow {
            name: name.to_owned(),
            baseline_present: aw.is_some(),
            candidate_present: bw.is_some(),
            baseline_weight: x,
            candidate_weight: y,
            baseline_percent: (at > 0).then(|| display_percent(x, at)),
            candidate_percent: (bt > 0).then(|| display_percent(y, bt)),
            delta_pp: (at > 0 && bt > 0).then(|| round_percent(percent(y, bt) - percent(x, at))),
            relative_weight_change_percent: (x > 0)
                .then(|| round_percent(100.0 * (y as f64 - x as f64) / x as f64)),
        });
    }
    let result = Comparison {
        schema_version: super::engine::SCHEMA_VERSION,
        metric: request.metric,
        stat: request.stat,
        sort: request.sort,
        identity_policy: request.identity_policy,
        baseline: Side {
            selection: a,
            scope_weight: at,
        },
        candidate: Side {
            selection: b,
            scope_weight: bt,
        },
        matched_rows,
        omitted_rows: matched_rows - rows.len(),
        rows,
    };
    ResponseBudget::new().json(&result)?;
    cancel.check()?;
    Ok(result)
}

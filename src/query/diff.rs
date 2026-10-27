use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::output::{
    DiffData, DiffOutput, DiffRow, DiffScopeWeight, ResponseBudget, SCHEMA_VERSION, output_profile,
    percent,
};

use super::{
    ApiError, DiffSort, Profile, TopSort, check_limit, compile_regex, metric_weight,
    row_limit_reason,
};

struct Candidate<'a> {
    name: &'a str,
    baseline_weight: u64,
    candidate_weight: u64,
    delta_pp: f64,
    name_rank: usize,
}

pub fn diff<'p>(
    baseline: &'p Profile,
    candidate: &'p Profile,
    metric: TopSort,
    sort: DiffSort,
    limit: usize,
    name_regex: Option<&str>,
) -> Result<DiffOutput<'p>, ApiError> {
    check_limit(limit, 1, 200, "limit")?;
    let regex = name_regex.map(compile_regex).transpose()?;
    let mut baseline_ids = (0..baseline.frames.len() as u32).peekable();
    let mut candidate_ids = (0..candidate.frames.len() as u32).peekable();
    let mut rows = BinaryHeap::with_capacity(
        limit.min(baseline.frames.len().saturating_add(candidate.frames.len())),
    );
    let mut available = 0;
    while baseline_ids.peek().is_some() || candidate_ids.peek().is_some() {
        let baseline_id = baseline_ids.peek().copied();
        let candidate_id = candidate_ids.peek().copied();
        let baseline_name = baseline_id.map(|id| baseline.frame_name(id));
        let candidate_name = candidate_id.map(|id| candidate.frame_name(id));
        let ordering = match (baseline_name, candidate_name) {
            (Some(a), Some(b)) => a.cmp(b),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => unreachable!("at least one dictionary has a frame"),
        };
        let mut baseline_weight = 0;
        let mut candidate_weight = 0;
        let mut name = "";
        if ordering != std::cmp::Ordering::Greater {
            let id = baseline_ids.next().unwrap();
            name = baseline_name.unwrap();
            baseline_weight = metric_weight(&baseline.frame_stats[id as usize], metric);
        }
        if ordering != std::cmp::Ordering::Less {
            let id = candidate_ids.next().unwrap();
            name = candidate_name.unwrap();
            candidate_weight = metric_weight(&candidate.frame_stats[id as usize], metric);
        }
        if regex.as_ref().is_some_and(|re| !re.is_match(name)) {
            continue;
        }
        // A frame with no weight on either side carries no measurement: listing it
        // as an unchanged row would only spend the limit on zeros.
        if baseline_weight == 0 && candidate_weight == 0 {
            continue;
        }
        let baseline_percent = percent(baseline_weight, baseline.total_weight);
        let candidate_percent = percent(candidate_weight, candidate.total_weight);
        let row = Candidate {
            name,
            baseline_weight,
            candidate_weight,
            delta_pp: candidate_percent - baseline_percent,
            name_rank: available,
        };
        available += 1;
        if rows.len() < limit {
            rows.push(HeapCandidate { sort, row });
        } else if candidate_order(&row, &rows.peek().expect("non-empty heap").row, sort)
            == Ordering::Less
        {
            rows.pop();
            rows.push(HeapCandidate { sort, row });
        }
    }
    let mut rows: Vec<_> = rows.into_iter().map(|row| row.row).collect();
    rows.sort_unstable_by(|a, b| candidate_order(a, b, sort));
    let mut bytes = ResponseBudget::new();
    let rows: Vec<_> = rows
        .into_iter()
        .map(|row| {
            bytes.row_name(row.name)?;
            Ok(DiffRow {
                name: row.name,
                baseline_weight: row.baseline_weight,
                candidate_weight: row.candidate_weight,
                baseline_percent: percent(row.baseline_weight, baseline.total_weight),
                candidate_percent: percent(row.candidate_weight, candidate.total_weight),
                delta_pp: row.delta_pp,
            })
        })
        .collect::<Result<_, ApiError>>()?;
    let truncation_reasons = row_limit_reason(limit, available);
    let output = super::envelope(
        candidate,
        candidate.total_weight,
        truncation_reasons,
        vec!["Percentage-point changes do not prove causality or statistical significance."],
        DiffData { metric, sort, rows },
    );
    Ok(DiffOutput {
        schema_version: SCHEMA_VERSION,
        baseline: output_profile(baseline),
        candidate: output.profile,
        scope_weight: DiffScopeWeight {
            baseline: baseline.total_weight,
            candidate: candidate.total_weight,
        },
        truncated: output.truncated,
        truncation_reasons: output.truncation_reasons,
        warnings: output.warnings,
        data: output.data,
    })
}

struct HeapCandidate<'a> {
    sort: DiffSort,
    row: Candidate<'a>,
}

impl PartialEq for HeapCandidate<'_> {
    fn eq(&self, other: &Self) -> bool {
        candidate_order(&self.row, &other.row, self.sort) == Ordering::Equal
    }
}

impl Eq for HeapCandidate<'_> {}

impl PartialOrd for HeapCandidate<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapCandidate<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        candidate_order(&self.row, &other.row, self.sort)
    }
}

fn candidate_order(a: &Candidate<'_>, b: &Candidate<'_>, sort: DiffSort) -> Ordering {
    let primary = match sort {
        DiffSort::Regression => b.delta_pp.total_cmp(&a.delta_pp),
        DiffSort::Improvement => a.delta_pp.total_cmp(&b.delta_pp),
        DiffSort::Absolute => b.delta_pp.abs().total_cmp(&a.delta_pp.abs()),
    };
    primary.then(a.name_rank.cmp(&b.name_rank))
}

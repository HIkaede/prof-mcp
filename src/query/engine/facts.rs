use std::{cell::OnceCell, collections::BTreeMap, ops::Range};

use hashbrown::HashMap;
use serde_json::json;

use crate::{
    error::ApiError,
    profile::{ContextRef, FrameId, FrameStats, Profile, StackId},
    query::{Cancellation, selection::SelectedFacts},
};

use super::Scope;

pub(super) const MAX_ROWS: usize = 250_000;
const MAX_FACTS: usize = 1_000_000;
const MAX_WORK: usize = 32_000_000;
const MAX_JSON_INTEGER: u64 = (1 << 53) - 1;

#[derive(Clone, Debug)]
pub struct MetricScope {
    pub profile_weight: u64,
    pub event_stream_id: Option<String>,
    pub scope_key: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct FactWeight {
    pub weight: u64,
    pub fact_count: u64,
}

#[derive(Clone, Copy)]
pub(super) struct Fact {
    pub(super) id: StackId,
    pub(super) measure: FactWeight,
}

pub(super) struct Work<'a> {
    remaining: usize,
    cancellation: Cancellation<'a>,
}

impl<'a> Work<'a> {
    pub(super) fn new(cancellation: Cancellation<'a>) -> Self {
        Self {
            remaining: MAX_WORK,
            cancellation,
        }
    }

    pub(super) fn charge(&mut self, amount: usize) -> Result<(), ApiError> {
        let remaining = self
            .remaining
            .checked_sub(amount)
            .ok_or_else(|| budget("projection_work", MAX_WORK))?;
        if remaining / 1024 != self.remaining / 1024 {
            self.check()?;
        }
        self.remaining = remaining;
        Ok(())
    }

    pub(super) fn check(&self) -> Result<(), ApiError> {
        self.cancellation.check()
    }
}

pub(super) fn budget(resource: &'static str, limit: usize) -> ApiError {
    ApiError::new(
        "query_budget_exceeded",
        "Exact projection exceeds the computation budget",
        json!({"resource":resource,"limit":limit}),
        "Narrow the observation selection or choose a smaller aggregation.",
    )
}

pub(super) fn check_rows(count: usize) -> Result<(), ApiError> {
    if count >= MAX_ROWS {
        return Err(budget("aggregation_rows", MAX_ROWS));
    }
    Ok(())
}

pub(super) struct ContextMeasure {
    pub(super) range: Range<StackId>,
    pub(super) depth: usize,
    pub(super) weight: u64,
    pub(super) own: u64,
    pub(super) has_facts: bool,
}

pub(super) struct Facts<'p, 'a> {
    pub(super) profile: &'p Profile,
    pub(super) scope: Scope,
    selected: SelectedFacts,
    weights: Option<Vec<Fact>>,
    prefix: OnceCell<Vec<(StackId, u64)>>,
    cancellation: Cancellation<'a>,
}

impl<'p, 'a> Facts<'p, 'a> {
    pub(super) fn new(
        profile: &'p Profile,
        selected: SelectedFacts,
        weighted: Option<(&MetricScope, &BTreeMap<StackId, FactWeight>)>,
        cancellation: Cancellation<'a>,
    ) -> Result<Self, ApiError> {
        let folded = Scope {
            profile_weight: profile.total_weight(),
            selected_weight: selected.scope_weight(),
            stack_shapes: selected.len(),
        };
        let mut scope = folded;
        let weights = if let Some((metric, weights)) = weighted {
            if metric.profile_weight > MAX_JSON_INTEGER {
                return Err(invalid_metric_weight());
            }
            if weights.len() > MAX_FACTS {
                return Err(budget("weighted_stack_shapes", MAX_FACTS));
            }
            if weights
                .last_key_value()
                .is_some_and(|(&id, _)| id as usize >= profile.stacks().len())
            {
                return Err(ApiError::internal(
                    "Weighted observation references a missing stack",
                ));
            }
            let mut ids = selected.iter().enumerate().peekable();
            let mut facts = Vec::with_capacity(weights.len().min(selected.len()));
            let mut selection_weight = 0u64;
            let mut fact_count = 0u64;
            for (index, (&id, &measure)) in weights.iter().enumerate() {
                cancellation.checkpoint(index)?;
                if measure.fact_count == 0 {
                    return Err(ApiError::internal("Weighted stack has no observations"));
                }
                while let Some(&(index, current)) = ids.peek()
                    && current < id
                {
                    cancellation.checkpoint(index)?;
                    ids.next();
                }
                if ids.peek().is_some_and(|&(_, current)| current == id) {
                    selection_weight = selection_weight
                        .checked_add(measure.weight)
                        .ok_or_else(|| ApiError::internal("Selected weight overflows"))?;
                    fact_count = fact_count.checked_add(measure.fact_count).ok_or_else(|| {
                        ApiError::internal("Selected observation count overflows")
                    })?;
                    facts.push(Fact { id, measure });
                }
            }
            if selection_weight > MAX_JSON_INTEGER || fact_count > MAX_JSON_INTEGER {
                return Err(invalid_metric_weight());
            }
            if selection_weight > metric.profile_weight {
                return Err(ApiError::internal(
                    "Selected weight exceeds the source weight",
                ));
            }
            scope = Scope {
                profile_weight: metric.profile_weight,
                selected_weight: selection_weight,
                stack_shapes: facts.len(),
            };
            Some(facts)
        } else {
            None
        };
        Ok(Self {
            profile,
            scope,
            selected,
            weights,
            prefix: OnceCell::new(),
            cancellation,
        })
    }

    pub(super) fn is_full_folded(&self) -> bool {
        self.weights.is_none() && self.selected.len() == self.profile.stacks().len()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = Fact> + '_ {
        self.in_range(0..self.profile.stacks().len() as StackId)
    }

    pub(super) fn in_range(&self, range: Range<StackId>) -> impl Iterator<Item = Fact> + '_ {
        let weighted = self.weights.as_ref().map_or(&[][..], |facts| {
            let begin = facts.partition_point(|fact| fact.id < range.start);
            let end = facts.partition_point(|fact| fact.id < range.end);
            &facts[begin..end]
        });
        let folded = self.weights.is_none().then_some(&self.selected);
        folded
            .into_iter()
            .flat_map(move |selected| selected.iter_range(range.clone()))
            .map(|id| Fact {
                id,
                measure: FactWeight {
                    weight: self.profile.stacks().stack(id).weight,
                    fact_count: 1,
                },
            })
            .chain(weighted.iter().copied())
    }

    pub(super) fn context(&self, context: ContextRef) -> Result<ContextMeasure, ApiError> {
        let range = self
            .profile
            .context_range(context)
            .ok_or_else(invalid_context)?;
        let depth = self
            .profile
            .context_depth(context)
            .ok_or_else(invalid_context)?;
        let terminal =
            !range.is_empty() && self.profile.stacks().stack(range.start).frames.len() == depth;
        if self.is_full_folded() {
            let total = self
                .profile
                .context_weight(context)
                .ok_or_else(invalid_context)?;
            let own = if terminal {
                self.profile.stacks().stack(range.start).weight
            } else {
                0
            };
            Ok(ContextMeasure {
                has_facts: !range.is_empty(),
                range,
                depth,
                weight: total,
                own,
            })
        } else {
            let prefix = self.prefix()?;
            let first = prefix.partition_point(|&(id, _)| id < range.start);
            let end = prefix.partition_point(|&(id, _)| id < range.end);
            let total = cumulative(prefix, end) - cumulative(prefix, first);
            let own = if terminal && prefix.get(first).is_some_and(|&(id, _)| id == range.start) {
                cumulative(prefix, first + 1) - cumulative(prefix, first)
            } else {
                0
            };
            Ok(ContextMeasure {
                has_facts: first < end,
                range,
                depth,
                weight: total,
                own,
            })
        }
    }

    fn prefix(&self) -> Result<&[(StackId, u64)], ApiError> {
        if self.scope.stack_shapes > MAX_FACTS {
            return Err(budget("selection_prefix_sums", MAX_FACTS));
        }
        if self.prefix.get().is_none() {
            let mut rows = Vec::with_capacity(self.scope.stack_shapes);
            let mut sum = 0;
            for (index, fact) in self.iter().enumerate() {
                self.cancellation.checkpoint(index)?;
                sum += fact.measure.weight;
                rows.push((fact.id, sum));
            }
            let _ = self.prefix.set(rows);
        }
        Ok(self.prefix.get().expect("initialized prefix totals"))
    }
}

fn cumulative(prefix: &[(StackId, u64)], index: usize) -> u64 {
    index.checked_sub(1).map_or(0, |index| prefix[index].1)
}

fn invalid_metric_weight() -> ApiError {
    ApiError::new(
        "query_budget_exceeded",
        "Sample metric exceeds the exact JSON integer range",
        json!({"resource":"metric_weight", "limit":MAX_JSON_INTEGER}),
        "Select a narrower sample range or use sample_count.",
    )
}

pub(super) fn invalid_context() -> ApiError {
    ApiError::new(
        "invalid_context_ref",
        "Context reference is outside the snapshot",
        json!({}),
        "Copy a context reference from a result for the same selection.",
    )
}

pub(crate) type SparseFrameStats = HashMap<FrameId, (FrameStats, StackId)>;

pub(crate) fn accumulate_sparse_stats(
    stats: &mut SparseFrameStats,
    stack: StackId,
    frames: &[FrameId],
    weight: u64,
    max_rows: usize,
) -> Result<(), ApiError> {
    for &frame in frames {
        if stats.len() >= max_rows && !stats.contains_key(&frame) {
            return Err(budget("aggregation_rows", max_rows));
        }
        let (stat, seen) = stats
            .entry(frame)
            .or_insert((FrameStats::default(), u32::MAX));
        if *seen != stack {
            *seen = stack;
            stat.inclusive_weight += weight;
            stat.stack_count += 1;
        }
    }
    if let Some(leaf) = frames.last() {
        stats.get_mut(leaf).unwrap().0.self_weight += weight;
    }
    Ok(())
}

pub(crate) fn selected_frame_stats(
    profile: &Profile,
    selected: &SelectedFacts,
    cancellation: Cancellation<'_>,
) -> Result<SparseFrameStats, ApiError> {
    let mut stats = SparseFrameStats::new();
    let mut work = Work::new(cancellation);
    work.check()?;
    for id in selected.iter() {
        let stack = profile.stacks().stack(id);
        work.charge(stack.frames.len())?;
        accumulate_sparse_stats(&mut stats, id, stack.frames, stack.weight, MAX_ROWS)?;
    }
    work.check()?;
    Ok(stats)
}

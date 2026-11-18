use std::{cmp::Reverse, ops::Range};

use hashbrown::{HashMap, HashSet};
use regex::Regex;
use serde_json::json;

use crate::{
    error::ApiError,
    profile::{ContextRef, FrameId, Profile, StackId},
    query::{TopSort, compile_regex, metric_weight},
};

use super::{
    Anchor, AnchorStats, NameFilter, Occurrence, View,
    facts::{
        ContextMeasure, Fact, Facts, MAX_ROWS, SparseFrameStats, Work, accumulate_sparse_stats,
        check_rows, invalid_context,
    },
    result::{Aggregate, share},
};

pub(super) struct Calculation {
    pub(super) rows: Rows,
    pub(super) additive: bool,
    pub(super) matched: Option<u64>,
    pub(super) filtered: Option<u64>,
    pub(super) coverage: Option<u64>,
    pub(super) anchor: Option<AnchorStats>,
    pub(super) aggregate: Option<Aggregate>,
}

impl Calculation {
    fn additive(rows: Rows, matched: u64) -> Self {
        Self {
            rows,
            additive: true,
            matched: Some(matched),
            filtered: None,
            coverage: None,
            anchor: None,
            aggregate: None,
        }
    }

    fn overlapping(rows: Rows, coverage: u64) -> Self {
        Self {
            rows,
            additive: false,
            matched: None,
            filtered: None,
            coverage: Some(coverage),
            anchor: None,
            aggregate: None,
        }
    }
}

pub(super) enum Rows {
    Frames(Vec<FrameRow>),
    RankedSelf(usize),
    Contexts(Vec<ContextRow>),
    Children(Vec<ContextRow>),
    Relations(Vec<FrameRow>),
    Paths(Vec<PathRow>),
    RankedPaths,
    None,
}

pub(super) type FrameRow = (FrameId, u64);
pub(super) type PathRow = (StackId, u64);

pub(super) struct ContextRow {
    pub(super) context: ContextRef,
    pub(super) frame: FrameId,
    pub(super) depth: usize,
    pub(super) weight: u64,
    pub(super) own: u64,
    range: Range<StackId>,
}

impl Rows {
    pub(super) fn len(&self, profile: &Profile) -> usize {
        match self {
            Self::Frames(rows) | Self::Relations(rows) => rows.len(),
            Self::RankedSelf(count) => *count,
            Self::Contexts(rows) | Self::Children(rows) => rows.len(),
            Self::Paths(rows) => rows.len(),
            Self::RankedPaths => profile.stacks().len(),
            Self::None => 0,
        }
    }

    pub(super) fn weight(&self, profile: &Profile, index: usize) -> u64 {
        match self {
            Self::Frames(rows) | Self::Relations(rows) => rows[index].1,
            Self::RankedSelf(_) => {
                profile.frame_stats()[profile.top_self()[index] as usize].self_weight
            }
            Self::Contexts(rows) | Self::Children(rows) => rows[index].weight,
            Self::Paths(rows) => rows[index].1,
            Self::RankedPaths => {
                profile
                    .stacks()
                    .stack(profile.postings().ranked_stacks()[index])
                    .weight
            }
            Self::None => unreachable!("aggregate has no table rows"),
        }
    }

    pub(super) fn path_id(&self, profile: &Profile, index: usize) -> StackId {
        match self {
            Self::Paths(rows) => rows[index].0,
            Self::RankedPaths => profile.postings().ranked_stacks()[index],
            _ => unreachable!("path id requested outside path results"),
        }
    }
}

pub(super) fn calculate(
    facts: &Facts<'_, '_>,
    work: &mut Work<'_>,
    view: &View,
) -> Result<Calculation, ApiError> {
    match view {
        View::Frames {
            stat,
            name_filter,
            include_zero,
            anchor,
            ..
        } => frames(
            facts,
            work,
            *stat,
            name_filter.as_ref(),
            *include_zero,
            *anchor,
        ),
        View::Contexts { frame, .. } => contexts(facts, work, frame.resolve(facts.profile, true)?),
        View::Children { context, .. } => {
            children(facts, work, context.unwrap_or(ContextRef::ROOT))
        }
        View::Callers {
            anchor, occurrence, ..
        } => relations(facts, work, anchor, *occurrence, true),
        View::Callees {
            anchor, occurrence, ..
        } => relations(facts, work, anchor, *occurrence, false),
        View::Paths { .. } => paths(facts, work),
        View::Aggregate {
            name_filter,
            anchor,
            ..
        } => aggregate(facts, work, name_filter.as_ref(), *anchor),
    }
}

#[derive(Default)]
struct AllowedFrames {
    matching: Option<HashSet<FrameId>>,
}

impl AllowedFrames {
    fn compile(
        profile: &Profile,
        filter: Option<&NameFilter>,
        work: &mut Work<'_>,
    ) -> Result<Self, ApiError> {
        let Some(filter) = filter else {
            return Ok(Self::default());
        };
        if filter.name_exact.is_some() == filter.name_regex.is_some() {
            return Err(ApiError::new(
                "invalid_view",
                "A name filter must set exactly one member",
                json!({}),
                "Use name_exact or name_regex.",
            ));
        }
        let mut matching = HashSet::new();
        if let Some(name) = &filter.name_exact {
            work.charge(name.len())?;
            matching.extend(profile.frame_id(name));
        } else if let Some(regex) = filter.name_regex.as_deref() {
            let regex: Regex = compile_regex(regex)?;
            for (index, name) in profile.frames().iter().enumerate() {
                work.charge(name.len().max(1))?;
                if regex.is_match(name) {
                    check_rows(matching.len())?;
                    matching.insert(index as FrameId);
                }
            }
        }
        Ok(Self {
            matching: Some(matching),
        })
    }

    fn matches(&self, id: FrameId) -> bool {
        self.matching
            .as_ref()
            .is_none_or(|matching| matching.contains(&id))
    }
}

struct ProjectedPath {
    range: Range<StackId>,
    begin: usize,
    weight: u64,
    anchor: Option<AnchorStats>,
}

impl ProjectedPath {
    fn new(facts: &Facts<'_, '_>, anchor: Option<ContextRef>) -> Result<Self, ApiError> {
        if let Some(context) = anchor {
            let ContextMeasure {
                range,
                depth,
                weight,
                own,
                ..
            } = facts.context(context)?;
            return Ok(Self {
                range,
                begin: depth.saturating_sub(1),
                weight,
                anchor: Some(anchor_stats(
                    facts,
                    Some(context),
                    facts.profile.context_frame(context),
                    weight,
                    own,
                )),
            });
        }
        Ok(Self {
            range: 0..facts.profile.stacks().len() as StackId,
            begin: 0,
            weight: facts.scope.selected_weight,
            anchor: None,
        })
    }

    fn facts<'a, 'p>(
        &'a self,
        facts: &'a Facts<'p, '_>,
    ) -> impl Iterator<Item = (Fact, &'p [FrameId])> + 'a {
        facts.in_range(self.range.clone()).map(|fact| {
            (
                fact,
                &facts.profile.stacks().stack(fact.id).frames[self.begin..],
            )
        })
    }
}

fn frames(
    facts: &Facts<'_, '_>,
    work: &mut Work<'_>,
    stat: TopSort,
    filter: Option<&NameFilter>,
    include_zero: bool,
    anchor: Option<ContextRef>,
) -> Result<Calculation, ApiError> {
    let allowed = AllowedFrames::compile(facts.profile, filter, work)?;
    let path = ProjectedPath::new(facts, anchor)?;
    let inclusive = stat == TopSort::Inclusive;
    if facts.scope.stack_shapes == 0 {
        let rows = Rows::Frames(Vec::new());
        let mut result = if inclusive {
            Calculation::overlapping(rows, 0)
        } else {
            let mut result = Calculation::additive(rows, 0);
            result.filtered = Some(0);
            result
        };
        result.anchor = path.anchor;
        return Ok(result);
    }
    if facts.is_full_folded() && anchor.is_none() && filter.is_none() && !inclusive {
        let ranked = facts.profile.top_self();
        let count = if include_zero {
            ranked.len()
        } else {
            ranked.partition_point(|&id| facts.profile.frame_stats()[id as usize].self_weight > 0)
        };
        let mut result = Calculation::additive(Rows::RankedSelf(count), path.weight);
        result.filtered = Some(0);
        return Ok(result);
    }
    let mut rows = Vec::new();
    let mut matched_weight = 0u64;
    let mut add_row = |frame: FrameId, value: u64| -> Result<(), ApiError> {
        if (include_zero || value > 0) && allowed.matches(frame) {
            check_rows(rows.len())?;
            rows.push((frame, value));
            if !inclusive {
                matched_weight += value;
            }
        }
        Ok(())
    };
    let mut coverage = 0u64;
    if facts.is_full_folded() && anchor.is_none() {
        let stats = facts.profile.frame_stats();
        for (frame, stats) in stats.iter().enumerate() {
            work.charge(1)?;
            add_row(frame as FrameId, metric_weight(stats, stat))?;
        }
        if inclusive {
            coverage = coverage_for(facts, work, &allowed, &path)?;
        }
    } else {
        let mut stats = SparseFrameStats::new();
        for (fact, frames) in path.facts(facts) {
            work.charge(if inclusive || include_zero {
                frames.len()
            } else {
                1
            })?;
            let measure = fact.measure.weight;
            if inclusive || include_zero {
                accumulate_sparse_stats(&mut stats, fact.id, frames, measure, MAX_ROWS)?;
                if inclusive && frames.iter().any(|&frame| allowed.matches(frame)) {
                    coverage += measure;
                }
            } else {
                let frame = *frames.last().expect("nonempty paths");
                if stats.len() >= MAX_ROWS && !stats.contains_key(&frame) {
                    check_rows(stats.len())?;
                }
                stats.entry(frame).or_default().0.self_weight += measure;
            }
        }
        for (frame, (stats, _)) in stats {
            add_row(frame, metric_weight(&stats, stat))?;
        }
    }
    rows.sort_unstable_by_key(|&(id, weight)| (Reverse(weight), id));
    let mut result = if inclusive {
        Calculation::overlapping(Rows::Frames(rows), coverage)
    } else {
        let mut result = Calculation::additive(Rows::Frames(rows), matched_weight);
        result.filtered = Some(path.weight - matched_weight);
        result
    };
    result.anchor = path.anchor;
    Ok(result)
}

fn coverage_for(
    facts: &Facts<'_, '_>,
    work: &mut Work<'_>,
    allowed: &AllowedFrames,
    path: &ProjectedPath,
) -> Result<u64, ApiError> {
    if allowed.matching.is_none() {
        return Ok(path.weight);
    }
    if allowed.matching.as_ref().is_some_and(HashSet::is_empty) {
        return Ok(0);
    }
    let mut coverage = 0;
    for (fact, frames) in path.facts(facts) {
        work.charge(frames.len())?;
        if frames.iter().any(|&frame| allowed.matches(frame)) {
            coverage += fact.measure.weight;
        }
    }
    Ok(coverage)
}

fn aggregate(
    facts: &Facts<'_, '_>,
    work: &mut Work<'_>,
    filter: Option<&NameFilter>,
    anchor: Option<ContextRef>,
) -> Result<Calculation, ApiError> {
    let allowed = AllowedFrames::compile(facts.profile, filter, work)?;
    let path = ProjectedPath::new(facts, anchor)?;
    let mut aggregate = Aggregate {
        self_sum: 0,
        stack_union_weight: 0,
        fact_count: 0,
        stack_shapes: 0,
    };
    for (fact, frames) in path.facts(facts) {
        work.charge(if filter.is_some() { frames.len() } else { 1 })?;
        if allowed.matches(*frames.last().expect("nonempty paths")) {
            aggregate.self_sum += fact.measure.weight;
        }
        if filter.is_none() || frames.iter().any(|&frame| allowed.matches(frame)) {
            aggregate.stack_union_weight += fact.measure.weight;
            aggregate.fact_count += fact.measure.fact_count;
            aggregate.stack_shapes += 1;
        }
    }
    let mut calculation = Calculation::overlapping(Rows::None, aggregate.stack_union_weight);
    calculation.aggregate = Some(aggregate);
    calculation.anchor = path.anchor;
    Ok(calculation)
}

fn contexts(
    facts: &Facts<'_, '_>,
    work: &mut Work<'_>,
    frame: FrameId,
) -> Result<Calculation, ApiError> {
    let profile = facts.profile;
    let mut rows = Vec::new();
    for (node_id, node) in profile.cct().nodes().iter().enumerate().skip(1) {
        let start = node.label_offset() as usize;
        let label_len = node.label_len() as usize;
        work.charge(label_len)?;
        for (offset, &id) in profile.stacks().frames()[start..start + label_len]
            .iter()
            .enumerate()
        {
            if id == frame {
                let context = ContextRef {
                    node_id: node_id as u32,
                    offset: offset as u32,
                };
                let ContextMeasure {
                    range,
                    depth,
                    weight,
                    own,
                    has_facts,
                } = facts.context(context)?;
                if has_facts {
                    check_rows(rows.len())?;
                    rows.push(ContextRow {
                        context,
                        frame,
                        depth,
                        weight,
                        own,
                        range,
                    });
                }
            }
        }
    }
    // Context stack ranges are nested or disjoint. Visit outer ranges first to count their union.
    rows.sort_unstable_by_key(|row| (row.range.start, Reverse(row.range.end)));
    let mut last_end = 0;
    let mut coverage = 0;
    for row in &rows {
        if row.range.start >= last_end {
            last_end = row.range.end;
            coverage += row.weight;
        }
    }
    sort_contexts(&mut rows);
    Ok(Calculation::overlapping(Rows::Contexts(rows), coverage))
}

fn children(
    facts: &Facts<'_, '_>,
    work: &mut Work<'_>,
    context: ContextRef,
) -> Result<Calculation, ApiError> {
    let ContextMeasure { weight, own, .. } = facts.context(context)?;
    let children = facts
        .profile
        .context_children(context)
        .ok_or_else(invalid_context)?;
    let mut rows = Vec::new();
    for child in children {
        work.charge(1)?;
        let ContextMeasure {
            range,
            depth,
            weight,
            own,
            has_facts,
        } = facts.context(child)?;
        if has_facts {
            check_rows(rows.len())?;
            let frame = facts.profile.context_frame(child).expect("child has frame");
            rows.push(ContextRow {
                context: child,
                frame,
                depth,
                weight,
                own,
                range,
            });
        }
    }
    sort_contexts(&mut rows);
    let mut result = Calculation::additive(Rows::Children(rows), weight - own);
    result.anchor = Some(anchor_stats(
        facts,
        Some(context),
        facts.profile.context_frame(context),
        weight,
        own,
    ));
    Ok(result)
}

fn sort_contexts(rows: &mut [ContextRow]) {
    rows.sort_unstable_by_key(|row| (Reverse(row.weight), row.range.start, row.depth));
}

pub(super) enum ResolvedAnchor {
    Frame(FrameId),
    Context(ContextRef),
}

impl ResolvedAnchor {
    pub(super) fn from_input(profile: &Profile, anchor: &Anchor) -> Result<Self, ApiError> {
        match anchor {
            Anchor::Frame(frame) => Ok(Self::Frame(frame.resolve(profile, true)?)),
            Anchor::Context { context } => {
                profile
                    .context_depth(*context)
                    .ok_or_else(invalid_context)?;
                Ok(Self::Context(*context))
            }
        }
    }
}

fn anchor_stats(
    facts: &Facts<'_, '_>,
    context: Option<ContextRef>,
    frame_id: Option<FrameId>,
    weight: u64,
    self_weight: u64,
) -> AnchorStats {
    AnchorStats {
        frame_id,
        context,
        weight,
        self_weight,
        scope_percent: share(weight, facts.scope.selected_weight),
        occurrence: None,
        no_neighbor_weight: None,
    }
}

fn relations(
    facts: &Facts<'_, '_>,
    work: &mut Work<'_>,
    input: &Anchor,
    occurrence: Option<Occurrence>,
    callers: bool,
) -> Result<Calculation, ApiError> {
    let anchor = ResolvedAnchor::from_input(facts.profile, input)?;
    let occurrence = match anchor {
        ResolvedAnchor::Frame(_) if callers => Some(occurrence.unwrap_or(Occurrence::Innermost)),
        ResolvedAnchor::Frame(_) => Some(occurrence.unwrap_or(Occurrence::Outermost)),
        ResolvedAnchor::Context(_) => None,
    };
    let (range, context_depth) = match anchor {
        ResolvedAnchor::Frame(_) => (0..facts.profile.stacks().len() as StackId, 0),
        ResolvedAnchor::Context(context) => (
            facts
                .profile
                .context_range(context)
                .ok_or_else(invalid_context)?,
            facts
                .profile
                .context_depth(context)
                .ok_or_else(invalid_context)?,
        ),
    };
    let mut rows = HashMap::<FrameId, u64>::new();
    let mut anchor_weight = 0;
    let mut self_weight = 0;
    let mut matched = 0;
    for fact in facts.in_range(range) {
        work.charge(1)?;
        let frames = facts.profile.stacks().stack(fact.id).frames;
        let position = match anchor {
            // The virtual root sits just before the first real frame.
            ResolvedAnchor::Context(_) => Some(context_depth),
            ResolvedAnchor::Frame(frame) => {
                work.charge(frames.len())?;
                match occurrence {
                    Some(Occurrence::Outermost) => frames.iter().position(|id| *id == frame),
                    _ => frames.iter().rposition(|id| *id == frame),
                }
                .map(|position| position + 1)
            }
        };
        if let Some(depth) = position {
            let weight = fact.measure.weight;
            anchor_weight += weight;
            let own = match anchor {
                ResolvedAnchor::Frame(frame) => frames.last() == Some(&frame),
                ResolvedAnchor::Context(_) => frames.len() == depth,
            };
            if own {
                self_weight += weight;
            }
            let next = if callers {
                depth.checked_sub(2).and_then(|caller| frames.get(caller))
            } else {
                frames.get(depth)
            };
            if let Some(&next) = next {
                if rows.len() >= MAX_ROWS && !rows.contains_key(&next) {
                    check_rows(rows.len())?;
                }
                *rows.entry(next).or_default() += weight;
                matched += weight;
            }
        }
    }
    let mut rows: Vec<_> = rows.into_iter().collect();
    rows.sort_unstable_by_key(|&(frame, weight)| (Reverse(weight), frame));
    let (context, frame) = match anchor {
        ResolvedAnchor::Frame(frame) => (None, Some(frame)),
        ResolvedAnchor::Context(context) => (Some(context), facts.profile.context_frame(context)),
    };
    let mut anchor_stats = anchor_stats(facts, context, frame, anchor_weight, self_weight);
    anchor_stats.occurrence = occurrence;
    anchor_stats.no_neighbor_weight = Some(anchor_weight - matched);
    let mut result = Calculation::additive(Rows::Relations(rows), matched);
    result.anchor = Some(anchor_stats);
    Ok(result)
}

fn paths(facts: &Facts<'_, '_>, work: &mut Work<'_>) -> Result<Calculation, ApiError> {
    if facts.scope.stack_shapes == 0 {
        return Ok(Calculation::additive(Rows::Paths(Vec::new()), 0));
    }
    let ranked = if facts.is_full_folded() && !facts.profile.postings().ranked_stacks().is_empty() {
        Rows::RankedPaths
    } else {
        if facts.scope.stack_shapes > MAX_ROWS {
            check_rows(facts.scope.stack_shapes)?;
        }
        let mut rows = Vec::with_capacity(facts.scope.stack_shapes);
        for fact in facts.iter() {
            work.charge(1)?;
            rows.push((fact.id, fact.measure.weight));
        }
        rows.sort_unstable_by_key(|&(id, weight)| (Reverse(weight), id));
        Rows::Paths(rows)
    };
    Ok(Calculation::additive(ranked, facts.scope.selected_weight))
}

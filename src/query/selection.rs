use std::{io, ops::Range};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    error::ApiError,
    profile::{ContextRef, FrameId, Profile, StackId},
    query::Cancellation,
};

const MAX_PREDICATE_DEPTH: usize = 4;
const MAX_PREDICATE_TERMS: usize = 32;
const MAX_PREDICATE_NODES: usize = 64;
const MAX_PREDICATE_FRAMES: usize = 32;
const MAX_PREDICATE_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    #[default]
    OpaqueWeight,
    SampleCount,
    SamplePeriod,
}

#[derive(Clone, Copy, Debug, JsonSchema, Serialize)]
pub struct MetricDefinition<'a> {
    pub id: Metric,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_stream_id: Option<&'a str>,
    pub unit: &'static str,
    pub basis: &'static str,
    pub aggregation: &'static str,
    pub attribution: &'static str,
    pub provenance: &'static str,
}

impl Metric {
    pub fn definition(self) -> MetricDefinition<'static> {
        let (unit, basis, provenance) = match self {
            Self::OpaqueWeight => ("input_weight", "aggregate_stack_weight", "folded"),
            Self::SampleCount => ("samples", "observation_count", "perf_sample_records"),
            Self::SamplePeriod => ("event_units", "recorded_period", "perf_sample_records"),
        };
        MetricDefinition {
            id: self,
            event_stream_id: None,
            unit,
            basis,
            aggregation: "sum",
            attribution: "sampled_leaf",
            provenance,
        }
    }

    pub fn validate(self, _profile: &Profile) -> Result<(), ApiError> {
        match self {
            Self::OpaqueWeight => Ok(()),
            Self::SampleCount | Self::SamplePeriod => Err(unavailable(self.definition().basis)),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameRef {
    FrameId(FrameId),
    FrameName(String),
}

impl FrameRef {
    pub fn resolve(&self, profile: &Profile, allow_ids: bool) -> Result<FrameId, ApiError> {
        match self {
            Self::FrameId(_) if !allow_ids => Err(snapshot_required()),
            Self::FrameId(id) if (*id as usize) < profile.frames().len() => Ok(*id),
            Self::FrameName(name) => profile.frame_id(name).ok_or_else(frame_not_found),
            Self::FrameId(_) => Err(frame_not_found()),
        }
    }

    fn resolved_id(&self) -> FrameId {
        match self {
            Self::FrameId(id) => *id,
            Self::FrameName(_) => unreachable!("selection was resolved to frame IDs"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Predicate {
    All(Vec<Predicate>),
    Any(Vec<Predicate>),
    ContainsAll(Vec<FrameRef>),
    ContainsAny(Vec<FrameRef>),
    ExcludeAny(Vec<FrameRef>),
    OrderedPath(Vec<FrameRef>),
    /// Exact raw frame names in consecutive order from the stack root. An empty prefix matches all stacks. An absent prefix matches no stacks.
    PathPrefix(#[schemars(length(max = MAX_PREDICATE_FRAMES))] Vec<String>),
    UnderContext(ContextRef),
    TimeRange {
        begin_ns: u64,
        end_ns: u64,
    },
    Thread {
        pid: u32,
        tid: u32,
    },
}

impl Default for Predicate {
    fn default() -> Self {
        Self::All(Vec::new())
    }
}

impl Predicate {
    pub fn validate_alias(&self, profile: &Profile) -> Result<(), ApiError> {
        self.resolve_alias(profile).map(|_| ())
    }

    pub(super) fn resolve_alias(&self, profile: &Profile) -> Result<Self, ApiError> {
        self.validate_limits()?;
        self.resolve(profile, false)
    }

    fn validate_limits(&self) -> Result<(), ApiError> {
        let mut count = PredicateCounts::default();
        self.count(1, &mut count)?;
        let mut bytes = PredicateBytes(0);
        serde_json::to_writer(&mut bytes, self)
            .map_err(|_| budget_error("predicate_bytes", MAX_PREDICATE_BYTES))
    }

    fn count(&self, depth: usize, count: &mut PredicateCounts) -> Result<(), ApiError> {
        if depth > MAX_PREDICATE_DEPTH {
            return Err(budget_error("predicate_depth", MAX_PREDICATE_DEPTH));
        }
        count.nodes += 1;
        if count.nodes > MAX_PREDICATE_NODES {
            return Err(budget_error("predicate_nodes", MAX_PREDICATE_NODES));
        }
        match self {
            Self::All(children) | Self::Any(children) => {
                for child in children {
                    child.count(depth + 1, count)?;
                }
                return Ok(());
            }
            Self::ContainsAll(frames)
            | Self::ContainsAny(frames)
            | Self::ExcludeAny(frames)
            | Self::OrderedPath(frames) => count.frames(
                frames.len(),
                frames.iter().filter_map(|frame| match frame {
                    FrameRef::FrameName(name) => Some(name.as_str()),
                    FrameRef::FrameId(_) => None,
                }),
            )?,
            Self::PathPrefix(names) => {
                count.frames(names.len(), names.iter().map(String::as_str))?
            }
            Self::UnderContext(_) | Self::TimeRange { .. } | Self::Thread { .. } => {}
        }
        count.terms += 1;
        if count.terms > MAX_PREDICATE_TERMS {
            return Err(budget_error("predicate_terms", MAX_PREDICATE_TERMS));
        }
        Ok(())
    }

    fn resolve(&self, profile: &Profile, allow_ids: bool) -> Result<Self, ApiError> {
        let frames = |refs: &[FrameRef], ordered: bool| {
            let mut resolved = refs
                .iter()
                .map(|frame| frame.resolve(profile, allow_ids).map(FrameRef::FrameId))
                .collect::<Result<Vec<_>, _>>()?;
            if !ordered {
                resolved.sort_unstable();
                resolved.dedup();
            }
            Ok::<_, ApiError>(resolved)
        };
        let predicate = match self {
            Self::All(children) | Self::Any(children) => {
                let resolved = children
                    .iter()
                    .map(|child| child.resolve(profile, allow_ids))
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(normalize_group(matches!(self, Self::All(_)), resolved));
            }
            Self::ContainsAll(refs) => Self::ContainsAll(frames(refs, false)?),
            Self::ContainsAny(refs) => Self::ContainsAny(frames(refs, false)?),
            Self::ExcludeAny(refs) => Self::ExcludeAny(frames(refs, false)?),
            Self::OrderedPath(refs) => Self::OrderedPath(frames(refs, true)?),
            Self::PathPrefix(names) if names.is_empty() => Self::default(),
            Self::PathPrefix(names) => names
                .iter()
                .map(|name| profile.frame_id(name))
                .collect::<Option<Vec<_>>>()
                .and_then(|prefix| profile.context_for_prefix(&prefix))
                .map_or_else(|| Self::Any(Vec::new()), Self::UnderContext),
            Self::UnderContext(_) if !allow_ids => return Err(snapshot_required()),
            Self::UnderContext(context) if profile.context_range(*context).is_none() => {
                return Err(ApiError::new(
                    "invalid_context_ref",
                    "Context does not exist in the selected snapshot",
                    json!({"context": context}),
                    "Use a context returned for this selection's snapshot.",
                ));
            }
            Self::UnderContext(context) => Self::UnderContext(*context),
            Self::TimeRange { .. } => return Err(unavailable("timestamp_ns")),
            Self::Thread { .. } => return Err(unavailable("pid_tid")),
        };
        Ok(predicate)
    }

    fn narrow_candidates<'a>(&self, profile: &'a Profile, candidates: &mut Candidates<'a>) {
        match self {
            Self::All(children) => {
                for child in children {
                    child.narrow_candidates(profile, candidates);
                }
            }
            Self::UnderContext(context) => {
                let range = profile
                    .context_range(*context)
                    .expect("context was validated");
                candidates.range.start = candidates.range.start.max(range.start);
                candidates.range.end = candidates.range.end.min(range.end);
            }
            Self::ContainsAll(frames) | Self::OrderedPath(frames) => {
                for frame in frames {
                    candidates
                        .narrow_posting(profile.postings().ranked(frame.resolved_id() as usize));
                }
            }
            Self::ContainsAny(frames) if frames.len() == 1 => {
                candidates
                    .narrow_posting(profile.postings().ranked(frames[0].resolved_id() as usize));
            }
            _ => {}
        }
    }

    fn matches(&self, profile: &Profile, stack: StackId, path: &[FrameId]) -> bool {
        let contains = |frame: &FrameRef| path.contains(&frame.resolved_id());
        match self {
            Self::All(children) => children
                .iter()
                .all(|child| child.matches(profile, stack, path)),
            Self::Any(children) => children
                .iter()
                .any(|child| child.matches(profile, stack, path)),
            Self::ContainsAll(frames) => frames.iter().all(contains),
            Self::ContainsAny(frames) => frames.iter().any(contains),
            Self::ExcludeAny(frames) => !frames.iter().any(contains),
            Self::OrderedPath(frames) => {
                let mut path = path.iter();
                frames
                    .iter()
                    .all(|frame| path.any(|&id| id == frame.resolved_id()))
            }
            Self::UnderContext(context) => profile
                .context_range(*context)
                .expect("context was validated")
                .contains(&stack),
            Self::PathPrefix(_) => unreachable!("path prefix was resolved to a context"),
            Self::TimeRange { .. } | Self::Thread { .. } => {
                unreachable!("dimensions were validated")
            }
        }
    }
}

fn normalize_group(all: bool, predicates: Vec<Predicate>) -> Predicate {
    let mut flattened = Vec::new();
    for predicate in predicates {
        match predicate {
            Predicate::All(children) if all => flattened.extend(children),
            Predicate::Any(children) if !all => flattened.extend(children),
            Predicate::All(children) if children.is_empty() => return Predicate::default(),
            Predicate::Any(children) if children.is_empty() => {
                return Predicate::Any(Vec::new());
            }
            other => flattened.push(other),
        }
    }
    flattened.sort_unstable();
    flattened.dedup();
    if flattened.len() == 1 {
        return flattened.pop().expect("group contains one predicate");
    }
    if all {
        Predicate::All(flattened)
    } else {
        Predicate::Any(flattened)
    }
}

#[derive(Default)]
struct PredicateCounts {
    nodes: usize,
    terms: usize,
    frames: usize,
}

impl PredicateCounts {
    fn frames<'a>(
        &mut self,
        len: usize,
        mut names: impl Iterator<Item = &'a str>,
    ) -> Result<(), ApiError> {
        self.frames = self.frames.saturating_add(len);
        if self.frames > MAX_PREDICATE_FRAMES {
            return Err(budget_error("predicate_frames", MAX_PREDICATE_FRAMES));
        }
        if names.any(|name| name.len() > MAX_PREDICATE_BYTES) {
            return Err(budget_error("predicate_bytes", MAX_PREDICATE_BYTES));
        }
        Ok(())
    }
}

struct PredicateBytes(usize);

impl io::Write for PredicateBytes {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(buf.len());
        if self.0 > MAX_PREDICATE_BYTES {
            return Err(io::Error::other("predicate byte budget exceeded"));
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub snapshot_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
    #[serde(default)]
    pub predicate: Predicate,
}

impl Selection {
    pub fn new(profile: &Profile, predicate: Predicate) -> Result<Self, ApiError> {
        Ok(Self::bound(profile, predicate.resolve_alias(profile)?))
    }

    fn bound(profile: &Profile, predicate: Predicate) -> Self {
        Self {
            snapshot_id: profile.snapshot_id().to_owned(),
            source_digest: Some(profile.fingerprint().to_owned()),
            predicate,
        }
    }

    pub fn normalized(&self, profile: &Profile) -> Result<Self, ApiError> {
        self.check_snapshot(profile)?;
        self.predicate.validate_limits()?;
        Ok(Self::bound(profile, self.predicate.resolve(profile, true)?))
    }

    pub fn refine(&self, profile: &Profile, predicate: Predicate) -> Result<Self, ApiError> {
        self.check_snapshot(profile)?;
        self.predicate.validate_limits()?;
        predicate.validate_limits()?;
        let predicate = normalize_group(
            true,
            vec![
                self.predicate.resolve(profile, true)?,
                predicate.resolve(profile, true)?,
            ],
        );
        predicate.validate_limits()?;
        Ok(Self::bound(profile, predicate))
    }

    pub fn check_snapshot(&self, profile: &Profile) -> Result<(), ApiError> {
        if self.snapshot_id == profile.snapshot_id() {
            return Ok(());
        }
        Err(ApiError::new(
            "snapshot_unavailable",
            "The requested snapshot is unavailable",
            json!({}),
            "Open a new query for a registered profile or restore the original snapshot.",
        ))
    }

    pub fn evaluate(&self, profile: &Profile) -> Result<SelectedFacts, ApiError> {
        self.evaluate_with_budget(profile, SelectionBudget::default())
    }

    pub fn evaluate_cancellable(
        &self,
        profile: &Profile,
        cancellation: Cancellation<'_>,
    ) -> Result<SelectedFacts, ApiError> {
        self.evaluate_with_budget_and_cancellation(
            profile,
            SelectionBudget::default(),
            cancellation,
        )
    }

    pub fn evaluate_with_budget(
        &self,
        profile: &Profile,
        budget: SelectionBudget,
    ) -> Result<SelectedFacts, ApiError> {
        self.evaluate_with_budget_and_cancellation(profile, budget, Cancellation::default())
    }

    fn evaluate_with_budget_and_cancellation(
        &self,
        profile: &Profile,
        budget: SelectionBudget,
        cancellation: Cancellation<'_>,
    ) -> Result<SelectedFacts, ApiError> {
        cancellation.check()?;
        self.check_snapshot(profile)?;
        self.predicate.validate_limits()?;
        let predicate = self.predicate.resolve(profile, true)?;
        cancellation.check()?;
        match predicate {
            Predicate::All(ref children) if children.is_empty() => Ok(SelectedFacts {
                ids: FactIds::All(profile.stacks().len() as StackId),
                scope_weight: profile.total_weight(),
            }),
            Predicate::Any(ref children) if children.is_empty() => Ok(SelectedFacts {
                ids: FactIds::StackRange(0..0),
                scope_weight: 0,
            }),
            Predicate::UnderContext(context) => {
                let range = profile
                    .context_range(context)
                    .expect("context was validated");
                Ok(SelectedFacts {
                    ids: FactIds::StackRange(range),
                    scope_weight: profile
                        .context_weight(context)
                        .expect("context was validated"),
                })
            }
            _ => collect_facts(profile, &predicate, budget, cancellation),
        }
    }
}

/// Select a live alias or reuse a snapshot-bound selection from a result.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum Source {
    Alias { profile: String },
    Pinned { selection: Selection },
}

impl Source {
    pub fn selection(&self, profile: &Profile) -> Result<Selection, ApiError> {
        match self {
            Self::Alias { .. } => Selection::new(profile, Predicate::default()),
            Self::Pinned { selection } => selection.normalized(profile),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SelectionBudget {
    pub max_stack_ids: usize,
}

impl Default for SelectionBudget {
    fn default() -> Self {
        Self {
            max_stack_ids: 1_000_000,
        }
    }
}

struct Candidates<'a> {
    range: Range<StackId>,
    posting: Option<&'a [StackId]>,
}

impl<'a> Candidates<'a> {
    fn narrow_posting(&mut self, posting: &'a [StackId]) {
        if self
            .posting
            .is_none_or(|previous| posting.len() < previous.len())
        {
            self.posting = Some(posting);
        }
    }

    fn iter(&self) -> impl Iterator<Item = StackId> + '_ {
        let posting = self
            .posting
            .filter(|posting| posting.len() < self.range.len());
        let range = if posting.is_some() {
            0..0
        } else {
            self.range.clone()
        };
        range.chain(posting.into_iter().flatten().copied())
    }
}

fn collect_facts(
    profile: &Profile,
    predicate: &Predicate,
    budget: SelectionBudget,
    cancellation: Cancellation<'_>,
) -> Result<SelectedFacts, ApiError> {
    let mut candidates = Candidates {
        range: 0..profile.stacks().len() as StackId,
        posting: None,
    };
    predicate.narrow_candidates(profile, &mut candidates);
    let mut ids = Vec::new();
    let mut scope_weight = 0;
    for (position, id) in candidates.iter().enumerate() {
        cancellation.checkpoint(position)?;
        if !candidates.range.contains(&id) {
            continue;
        }
        let stack = profile.stacks().stack(id);
        if !predicate.matches(profile, id, stack.frames) {
            continue;
        }
        if ids.len() == budget.max_stack_ids {
            return Err(budget_error("stack_ids", budget.max_stack_ids));
        }
        if ids.len() == ids.capacity() {
            let capacity = ids
                .capacity()
                .saturating_mul(2)
                .max(1)
                .min(budget.max_stack_ids);
            ids.try_reserve_exact(capacity - ids.len())
                .map_err(|_| budget_error("stack_ids", budget.max_stack_ids))?;
        }
        ids.push(id);
        scope_weight += stack.weight;
    }
    ids.sort_unstable();
    cancellation.check()?;
    Ok(SelectedFacts {
        ids: FactIds::SortedStackIds(ids),
        scope_weight,
    })
}

#[derive(Debug)]
enum FactIds {
    All(StackId),
    StackRange(Range<StackId>),
    SortedStackIds(Vec<StackId>),
}

#[derive(Debug)]
pub struct SelectedFacts {
    ids: FactIds,
    scope_weight: u64,
}

impl SelectedFacts {
    pub fn len(&self) -> usize {
        match &self.ids {
            FactIds::All(end) => *end as usize,
            FactIds::StackRange(range) => range.len(),
            FactIds::SortedStackIds(ids) => ids.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn scope_weight(&self) -> u64 {
        self.scope_weight
    }

    /// Each selected stack ID occurs once, in ascending lexical stack order.
    pub fn iter(&self) -> impl Iterator<Item = StackId> + '_ {
        self.iter_range(0..StackId::MAX)
    }

    pub fn iter_range(&self, bounds: Range<StackId>) -> impl Iterator<Item = StackId> + '_ {
        let intersect =
            |range: Range<StackId>| range.start.max(bounds.start)..range.end.min(bounds.end);
        let (range, ids) = match &self.ids {
            FactIds::All(end) => (intersect(0..*end), &[][..]),
            FactIds::StackRange(range) => (intersect(range.clone()), &[][..]),
            FactIds::SortedStackIds(ids) => {
                let start = ids.partition_point(|&id| id < bounds.start);
                let end = ids.partition_point(|&id| id < bounds.end);
                (0..0, &ids[start..end.max(start)])
            }
        };
        range.chain(ids.iter().copied())
    }
}

fn snapshot_required() -> ApiError {
    ApiError::new(
        "invalid_selection",
        "Numeric frame and context references require a snapshot-bound selection",
        json!({}),
        "Provide the selection that accompanied the references.",
    )
}

fn frame_not_found() -> ApiError {
    ApiError::frame_not_found("Use a frame returned for this profile.")
}

fn unavailable(capability: &'static str) -> ApiError {
    ApiError::new(
        "capability_unavailable",
        "The folded profile does not retain the requested observation data",
        json!({"capability": capability, "source_kind": "folded"}),
        "Choose opaque_weight or query a source that retains individual sample records.",
    )
}

fn budget_error(resource: &str, limit: usize) -> ApiError {
    ApiError::new(
        "query_budget_exceeded",
        "Selection exceeded a query budget",
        json!({"resource": resource, "limit": limit}),
        "Use a narrower selection or fewer predicate terms.",
    )
}

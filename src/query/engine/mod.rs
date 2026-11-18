use std::collections::BTreeMap;

use serde_json::json;

use crate::{
    error::ApiError,
    output::ResponseBudget,
    profile::{Profile, StackId},
    query::{
        Cancellation, TopSort,
        selection::{FrameRef, Metric, Predicate, Selection},
    },
};

mod facts;
mod plan;
mod project;
mod render;
mod result;

pub use facts::{FactWeight, MetricScope};
pub(crate) use facts::{accumulate_sparse_stats, selected_frame_stats};
pub use plan::{
    Anchor, Cursor, ExplorePlan, NameFilter, Occurrence, PathRange, PathWindow, QueryPlan, View,
    ViewKind,
};
pub use result::{
    Aggregate, AnchorStats, Completeness, ExploreQuality, ExploreResult, Projection, QueryResult,
    Row, SCHEMA_VERSION, Scope, Table, Totals, Truncation,
};

use facts::{Facts, Work};
use project::{ResolvedAnchor, calculate};

pub fn query(
    profile: &Profile,
    selection: Selection,
    plan: QueryPlan,
) -> Result<QueryResult, ApiError> {
    query_cancellable(profile, selection, plan, Cancellation::default())
}

pub fn query_cancellable(
    profile: &Profile,
    selection: Selection,
    plan: QueryPlan,
    cancellation: Cancellation<'_>,
) -> Result<QueryResult, ApiError> {
    plan.metric.validate(profile)?;
    run_query(profile, selection, plan, None, cancellation)
}

pub fn query_weighted(
    profile: &Profile,
    selection: Selection,
    plan: QueryPlan,
    metric_scope: MetricScope,
    weights: &BTreeMap<StackId, FactWeight>,
) -> Result<QueryResult, ApiError> {
    if plan.metric == Metric::OpaqueWeight {
        return Err(ApiError::new(
            "metric_unavailable",
            "A sample query requires a sample metric",
            json!({}),
            "Choose sample_count or sample_period for the selected event.",
        ));
    }
    run_query(
        profile,
        selection,
        plan,
        Some((metric_scope, weights)),
        Cancellation::default(),
    )
}

fn run_query(
    profile: &Profile,
    selection: Selection,
    plan: QueryPlan,
    weighted: Option<(MetricScope, &BTreeMap<StackId, FactWeight>)>,
    cancellation: Cancellation<'_>,
) -> Result<QueryResult, ApiError> {
    cancellation.check()?;
    let QueryPlan {
        refine,
        metric,
        view,
        cursor,
    } = plan;
    view.validate()?;
    let selection = normalized(profile, selection, refine)?;
    let facts = Facts::new(
        profile,
        selection.evaluate_cancellable(profile, cancellation)?,
        weighted.as_ref().map(|(scope, weights)| (scope, *weights)),
        cancellation,
    )?;
    let event_stream_id = weighted
        .as_ref()
        .and_then(|(scope, _)| scope.event_stream_id.as_deref());
    let key = CursorKey::new(
        &selection,
        metric,
        &view,
        &facts,
        weighted.as_ref().map(|(scope, _)| scope),
        cancellation,
    )?;
    let offset = key.position(cursor.as_ref(), view.limit().is_some())?;
    let calculation = calculate(&facts, &mut Work::new(cancellation), &view)?;
    cancellation.check()?;
    let available = calculation.rows.len(profile);
    if offset > 0 && offset >= available {
        return Err(invalid_cursor());
    }
    // Choose the same page in every transport, including JSON nested in MCP text.
    let mut bytes = ResponseBudget::new();
    bytes.embedded_json(&selection)?;
    bytes.embedded_json(&event_stream_id)?;
    bytes.charge(2048)?;
    let mut projection = render::render(
        &facts,
        &view,
        calculation,
        offset,
        &mut bytes,
        false,
        cancellation,
    )?;
    let returned = projection
        .table
        .as_ref()
        .map_or(0, |table| table.rows.len());
    if offset + returned < available {
        projection.cursor = Some(key.cursor(offset + returned));
    }
    let result = QueryResult {
        schema_version: SCHEMA_VERSION,
        selection,
        metric: metric.definition(),
        scope: facts.scope,
        event_stream_id: event_stream_id.map(str::to_owned),
        projection,
    };
    let mut wire = ResponseBudget::new();
    wire.charge(128)?; // Leave room for the tool result envelope.
    wire.embedded_json(&result)?;
    Ok(result)
}

fn normalized(
    profile: &Profile,
    selection: Selection,
    refine: Option<Predicate>,
) -> Result<Selection, ApiError> {
    match refine {
        Some(refine) => selection.refine(profile, refine),
        None => selection.normalized(profile),
    }
}

pub fn explore(
    profile: &Profile,
    selection: Selection,
    plan: ExplorePlan,
) -> Result<ExploreResult, ApiError> {
    explore_cancellable(profile, selection, plan, Cancellation::default())
}

pub fn explore_cancellable(
    profile: &Profile,
    selection: Selection,
    plan: ExplorePlan,
    cancellation: Cancellation<'_>,
) -> Result<ExploreResult, ApiError> {
    cancellation.check()?;
    let ExplorePlan {
        refine,
        metric,
        anchor,
    } = plan;
    metric.validate(profile)?;
    let selection = normalized(profile, selection, refine)?;
    let resolved = anchor
        .as_ref()
        .map(|anchor| ResolvedAnchor::from_input(profile, anchor))
        .transpose()?;
    let selection = match resolved {
        Some(ResolvedAnchor::Frame(frame)) => selection.refine(
            profile,
            Predicate::ContainsAny(vec![FrameRef::FrameId(frame)]),
        )?,
        Some(ResolvedAnchor::Context(context)) => {
            selection.refine(profile, Predicate::UnderContext(context))?
        }
        None => selection,
    };
    let facts = Facts::new(
        profile,
        selection.evaluate_cancellable(profile, cancellation)?,
        None,
        cancellation,
    )?;
    let mut work = Work::new(cancellation);
    let views = explore_views(resolved);
    let mut bytes = ResponseBudget::new();
    bytes.embedded_json(&selection)?;
    // Reserve the envelope before spending a shared display budget on rows.
    bytes.charge(6144)?;
    bytes.0 = bytes.0.min(7 * 1024);
    let mut panels = Vec::with_capacity(views.len());
    for view in views {
        work.check()?;
        let calculation = calculate(&facts, &mut work, &view)?;
        let available = calculation.rows.len(profile);
        let mut projection = render::render(
            &facts,
            &view,
            calculation,
            0,
            &mut bytes,
            true,
            cancellation,
        )?;
        let returned = projection
            .table
            .as_ref()
            .map_or(0, |table| table.rows.len());
        if returned < available {
            let key = CursorKey::new(&selection, metric, &view, &facts, None, cancellation)?;
            if returned > 0 {
                projection.cursor = Some(key.cursor(returned));
            }
            projection.resume_view = Some(view);
        }
        panels.push(projection);
    }
    let anchor = panels
        .iter()
        .find_map(|panel| panel.anchor)
        .map(|anchor| AnchorStats {
            occurrence: None,
            no_neighbor_weight: None,
            ..anchor
        })
        .filter(|_| anchor.is_some());
    let result = ExploreResult {
        schema_version: SCHEMA_VERSION,
        selection,
        metric: metric.definition(),
        scope: facts.scope,
        quality: ExploreQuality {
            stack_completeness: "unknown",
            lost_records: None,
        },
        anchor,
        panels,
    };
    ResponseBudget::new().json(&result)?;
    Ok(result)
}

fn self_frames(limit: usize) -> View {
    View::Frames {
        stat: TopSort::SelfWeight,
        name_filter: None,
        include_zero: false,
        limit,
        totals: true,
        path_range: PathRange::Full,
        anchor: None,
    }
}

fn explore_views(anchor: Option<ResolvedAnchor>) -> Vec<View> {
    match anchor {
        None => vec![
            self_frames(8),
            View::Children {
                context: None,
                limit: 4,
            },
        ],
        Some(anchor) => {
            let (target, mut panels) = match anchor {
                ResolvedAnchor::Frame(frame) => {
                    let frame = FrameRef::FrameId(frame);
                    let target = Anchor::Frame(frame.clone());
                    let panels = vec![
                        View::Contexts { frame, limit: 3 },
                        View::Callers {
                            anchor: target.clone(),
                            occurrence: None,
                            limit: 5,
                        },
                    ];
                    (target, panels)
                }
                ResolvedAnchor::Context(context) => (
                    Anchor::Context { context },
                    vec![
                        View::Children {
                            context: Some(context),
                            limit: 5,
                        },
                        self_frames(5),
                    ],
                ),
            };
            panels.push(View::Paths {
                limit: 2,
                window: PathWindow::AroundTarget {
                    target,
                    before: 4,
                    after: 7,
                },
            });
            panels
        }
    }
}

struct CursorKey(blake3::Hasher);

impl CursorKey {
    fn new(
        selection: &Selection,
        metric: Metric,
        view: &View,
        facts: &Facts<'_, '_>,
        weighted_scope: Option<&MetricScope>,
        cancellation: Cancellation<'_>,
    ) -> Result<Self, ApiError> {
        let mut represent =
            serde_json::to_value(view).map_err(|error| ApiError::internal(error.to_string()))?;
        let object = represent.as_object_mut().expect("tagged view object");
        // Page size does not affect ranking or the meaning of the next offset.
        object.remove("limit");
        let weighted_key = weighted_scope.map(|scope| (&scope.event_stream_id, &scope.scope_key));
        let identity = serde_json::to_vec(&(
            SCHEMA_VERSION,
            &selection.snapshot_id,
            &selection.predicate,
            metric,
            represent,
            weighted_key,
        ))
        .map_err(|error| ApiError::internal(error.to_string()))?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(&identity);
        hasher.update(&facts.scope.profile_weight.to_le_bytes());
        if metric != Metric::OpaqueWeight {
            for (index, fact) in facts.iter().enumerate() {
                cancellation.checkpoint(index)?;
                hasher.update(&fact.id.to_le_bytes());
                hasher.update(&fact.measure.weight.to_le_bytes());
                hasher.update(&fact.measure.fact_count.to_le_bytes());
            }
        }
        Ok(Self(hasher))
    }

    fn cursor(&self, position: usize) -> Cursor {
        let mut hasher = self.0.clone();
        hasher.update(&(position as u64).to_le_bytes());
        Cursor {
            position,
            digest: hasher.finalize().to_hex().to_string(),
        }
    }

    fn position(&self, cursor: Option<&Cursor>, pageable: bool) -> Result<usize, ApiError> {
        match cursor {
            None => Ok(0),
            Some(cursor)
                if pageable
                    && cursor.position > 0
                    && cursor.digest.len() == 64
                    && self.cursor(cursor.position).digest == cursor.digest =>
            {
                Ok(cursor.position)
            }
            Some(_) => Err(invalid_cursor()),
        }
    }
}

fn invalid_cursor() -> ApiError {
    ApiError::new(
        "invalid_cursor",
        "Cursor does not identify a next page of this query",
        json!({}),
        "Copy the cursor and selection from the previous page and repeat its metric and view.",
    )
}

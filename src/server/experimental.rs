//! Transport contracts for the opt-in query tools.

use rmcp::{
    handler::server::tool::schema_for_input,
    model::{JsonObject, Tool, ToolAnnotations},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use super::{
    ProfileServer,
    inputs::{FindInput, ProfileInput},
    respond::{experimental_too_large, experimental_value, tag_alias, tag_registry},
};
use crate::{
    cache::LoadedProfile,
    error::ApiError,
    output::{FindOutput, ResponseBudget, SummaryOutput},
    profile::Profile,
    query::{
        self, Cancellation,
        comparison::{CompareRequest, compare_cancellable},
        engine::{self, Anchor, Cursor, ExplorePlan, QueryPlan, View},
        selection::{Metric, Predicate, Selection, Source},
    },
    registry,
};

pub(super) const TOOL_NAMES: [&str; 5] = [
    "profile_summary",
    "profile_find_symbols",
    "profile_query",
    "profile_explore",
    "profile_diff",
];
pub(super) const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct QueryInput {
    /// Omit for the active alias. Copy a returned selection for numeric references.
    source: Option<Source>,
    /// Add this predicate to the source selection.
    refine: Option<Predicate>,
    #[serde(default)]
    metric: Metric,
    view: View,
    /// Copy the cursor from the previous page with its selection and the same view.
    cursor: Option<Cursor>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExploreInput {
    /// Omit for the active alias. Copy a returned selection for numeric references.
    source: Option<Source>,
    refine: Option<Predicate>,
    #[serde(default)]
    metric: Metric,
    /// An exact frame or a snapshot-bound context.
    anchor: Option<Anchor>,
}

#[derive(JsonSchema, Serialize)]
struct SelectedOutput<T> {
    snapshot_id: String,
    selection: Selection,
    #[serde(flatten)]
    evidence: T,
}

pub(super) fn tool(name: &str) -> Option<Tool> {
    let tool = match name {
        "profile_summary" => describe::<ProfileInput>(
            "profile_summary",
            "Summarize a registered profile and folded measurement quality. Omit profile for the active alias. Copy selection into source.selection to pin later queries.",
        ),
        "profile_find_symbols" => describe::<FindInput>(
            "profile_find_symbols",
            "Find exact frame identities by contains or regex. Matches use columns and rows. Copy selection into source.selection to use a returned frame_id.",
        ),
        "profile_query" => describe::<QueryInput>(
            "profile_query",
            "Filter stacks and query one view: frames, contexts, children, callers, callees, paths, or aggregate. Use refine.path_prefix with exact raw names to select a stack prefix from the root. Rows use columns and rows. Each result returns the exact selection and scope. Reuse source.selection for numeric references. For pagination copy cursor and selection and keep the same view.",
        ),
        "profile_explore" => describe::<ExploreInput>(
            "profile_explore",
            "Return a bounded set of hotspots, branches and paths for a profile or an exact frame/context. Omit anchor for an overview. To examine a context copy its reference and source.selection.",
        ),
        "profile_diff" => describe::<CompareRequest>(
            "profile_diff",
            "Compare two explicit profile sources or selections. Align by an explicit identity rule and report original values, shares and percentage-point changes.",
        ),
        _ => return None,
    };
    Some(tool)
}

fn describe<T: JsonSchema + 'static>(name: &'static str, description: &'static str) -> Tool {
    Tool::new(
        name,
        description,
        schema_for_input::<T>().expect("experimental input schema is an object"),
    )
    .with_annotations(ToolAnnotations::new().read_only(true).open_world(false))
}

impl ProfileServer {
    pub(super) async fn experimental_call(
        &self,
        tool: &str,
        arguments: JsonObject,
        is_cancelled: impl Fn() -> bool + Send + 'static,
    ) -> Result<Value, ApiError> {
        Cancellation::new(&is_cancelled).check()?;
        ResponseBudget(MAX_REQUEST_BYTES)
            .json(&arguments)
            .map_err(|_| request_too_large())?;
        match tool {
            "profile_summary" => {
                self.experimental_summary(parse(arguments)?, is_cancelled)
                    .await
            }
            "profile_find_symbols" => {
                self.experimental_find(parse(arguments)?, is_cancelled)
                    .await
            }
            "profile_query" => {
                self.experimental_query(parse(arguments)?, is_cancelled)
                    .await
            }
            "profile_explore" => {
                self.experimental_explore(parse(arguments)?, is_cancelled)
                    .await
            }
            "profile_diff" => {
                self.experimental_diff(parse(arguments)?, is_cancelled)
                    .await
            }
            _ => Err(ApiError::new(
                "unknown_tool",
                "Unknown experimental tool",
                json!({"tool":tool}),
                "Use profile_summary, profile_find_symbols, profile_query, profile_explore or profile_diff.",
            )),
        }
    }

    async fn experimental_summary(
        &self,
        input: ProfileInput,
        is_cancelled: impl Fn() -> bool + Send + 'static,
    ) -> Result<Value, ApiError> {
        let workspace = self.cache.workspace();
        self.experimental_work(
            is_cancelled,
            async {
                let (resolved, status) =
                    registry::resolve_with_status(workspace, input.profile.as_deref())?;
                Ok((self.cache.load_resolved(resolved).await?, status))
            },
            |(loaded, status), _| {
                let mut evidence: SummaryOutput<'_> =
                    query::summary(&loaded.profile).map_err(map_output_error)?;
                evidence.schema_version = engine::SCHEMA_VERSION;
                tag_alias(&mut evidence, &loaded.alias);
                tag_registry(&mut evidence, status);
                experimental_value(selected_output(&loaded.profile, evidence)?)
            },
        )
        .await
    }

    async fn experimental_find(
        &self,
        input: FindInput,
        is_cancelled: impl Fn() -> bool + Send + 'static,
    ) -> Result<Value, ApiError> {
        self.experimental_work(
            is_cancelled,
            self.profile(input.profile.as_deref()),
            move |loaded, _| {
                let mut evidence: FindOutput<'_> =
                    query::find_symbols(&loaded.profile, &input.query, input.mode, input.limit)
                        .map_err(map_output_error)?;
                evidence.schema_version = engine::SCHEMA_VERSION;
                tag_alias(&mut evidence, &loaded.alias);
                experimental_value(selected_output(&loaded.profile, evidence)?)
            },
        )
        .await
    }

    async fn experimental_query(
        &self,
        input: QueryInput,
        is_cancelled: impl Fn() -> bool + Send + 'static,
    ) -> Result<Value, ApiError> {
        let QueryInput {
            source,
            refine,
            metric,
            view,
            cursor,
        } = input;
        let mut plan = QueryPlan {
            refine,
            metric,
            view,
            cursor,
        };
        self.experimental_work(
            is_cancelled,
            async {
                let loaded = self.load_source(source.as_ref()).await?;
                Ok((loaded, source))
            },
            move |(loaded, source), cancellation| {
                let profile = &loaded.profile;
                if !matches!(source, Some(Source::Pinned { .. })) {
                    plan.refine = plan.alias_refinement(profile)?;
                }
                let selection = selected(source.as_ref(), profile)?;
                experimental_value(
                    engine::query_cancellable(profile, selection, plan, cancellation)
                        .map_err(map_output_error)?,
                )
            },
        )
        .await
    }

    async fn experimental_explore(
        &self,
        input: ExploreInput,
        is_cancelled: impl Fn() -> bool + Send + 'static,
    ) -> Result<Value, ApiError> {
        let ExploreInput {
            source,
            refine,
            metric,
            anchor,
        } = input;
        let mut plan = ExplorePlan {
            refine,
            metric,
            anchor,
        };
        self.experimental_work(
            is_cancelled,
            async {
                let loaded = self.load_source(source.as_ref()).await?;
                Ok((loaded, source))
            },
            move |(loaded, source), cancellation| {
                let profile = &loaded.profile;
                if !matches!(source, Some(Source::Pinned { .. })) {
                    plan.refine = plan.alias_refinement(profile)?;
                }
                let selection = selected(source.as_ref(), profile)?;
                experimental_value(
                    engine::explore_cancellable(profile, selection, plan, cancellation)
                        .map_err(map_output_error)?,
                )
            },
        )
        .await
    }

    async fn experimental_diff(
        &self,
        input: CompareRequest,
        is_cancelled: impl Fn() -> bool + Send + 'static,
    ) -> Result<Value, ApiError> {
        self.experimental_work(
            is_cancelled,
            async {
                let baseline = self.load_source(Some(&input.baseline)).await?;
                let candidate = self.load_source(Some(&input.candidate)).await?;
                Ok((baseline, candidate, input))
            },
            |(baseline, candidate, input), cancellation| {
                experimental_value(
                    compare_cancellable(
                        &baseline.profile,
                        &candidate.profile,
                        &input,
                        cancellation,
                    )
                    .map_err(map_output_error)?,
                )
            },
        )
        .await
    }

    async fn experimental_work<T: Send + 'static>(
        &self,
        is_cancelled: impl Fn() -> bool + Send + 'static,
        load: impl std::future::Future<Output = Result<T, ApiError>> + Send,
        run: impl FnOnce(T, Cancellation<'_>) -> Result<Value, ApiError> + Send + 'static,
    ) -> Result<Value, ApiError> {
        self.work(
            async {
                Cancellation::new(&is_cancelled).check()?;
                Ok((load.await?, is_cancelled))
            },
            move |(loaded, is_cancelled)| {
                let cancellation = Cancellation::new(&is_cancelled);
                cancellation.check()?;
                let result = run(loaded, cancellation)?;
                cancellation.check()?;
                Ok(result)
            },
        )
        .await
    }

    async fn load_source(&self, source: Option<&Source>) -> Result<LoadedProfile, ApiError> {
        match source {
            None => self.profile(None).await,
            Some(Source::Alias { profile }) => self.profile(Some(profile)).await,
            Some(Source::Pinned { selection }) => {
                validate_snapshot_id(&selection.snapshot_id)?;
                if let Some(digest) = selection.source_digest.as_deref() {
                    let loaded = self.load_snapshot(digest, &selection.snapshot_id).await?;
                    check_snapshot(&loaded.profile, &selection.snapshot_id)?;
                    return Ok(loaded);
                }
                self.find_snapshot(&selection.snapshot_id).await
            }
        }
    }

    async fn load_snapshot(
        &self,
        source_digest: &str,
        snapshot_id: &str,
    ) -> Result<LoadedProfile, ApiError> {
        let resolved =
            registry::resolve_snapshot_blob(self.cache.workspace(), source_digest, snapshot_id)?;
        self.cache.load_resolved(resolved).await.map_err(|error| {
            if error.code == "profile_not_found" {
                registry::snapshot_unavailable(snapshot_id)
            } else {
                error
            }
        })
    }

    async fn find_snapshot(&self, snapshot_id: &str) -> Result<LoadedProfile, ApiError> {
        const MAX_PROBES: usize = 8;
        let status = registry::status(self.cache.workspace()).map_err(|error| {
            if error.code == "workspace_not_registered" {
                registry::snapshot_unavailable(snapshot_id)
            } else {
                error
            }
        })?;
        let mut probed = std::collections::BTreeSet::new();
        for registration in &status.profiles {
            if !probed.insert(&registration.fingerprint) {
                continue;
            }
            if probed.len() > MAX_PROBES {
                return Err(ApiError::new(
                    "query_budget_exceeded",
                    "Locating a snapshot without a source digest exceeded the probe limit",
                    json!({"limit":MAX_PROBES}),
                    "Copy the full selection, including source_digest, from a result.",
                ));
            }
            match self
                .load_snapshot(&registration.fingerprint, snapshot_id)
                .await
            {
                Ok(loaded) if loaded.profile.snapshot_id() == snapshot_id => return Ok(loaded),
                Ok(_) => {}
                Err(error) if error.code == "snapshot_unavailable" => {}
                Err(error) => return Err(error),
            }
        }
        Err(registry::snapshot_unavailable(snapshot_id))
    }
}

fn selected(source: Option<&Source>, profile: &Profile) -> Result<Selection, ApiError> {
    match source {
        Some(source) => source.selection(profile),
        None => Selection::new(profile, Predicate::default()),
    }
}

fn selected_output<T>(profile: &Profile, evidence: T) -> Result<SelectedOutput<T>, ApiError> {
    Ok(SelectedOutput {
        snapshot_id: profile.snapshot_id().to_owned(),
        selection: selected(None, profile)?,
        evidence,
    })
}

fn map_output_error(error: ApiError) -> ApiError {
    if error.code == "query_too_large" {
        experimental_too_large()
    } else {
        error
    }
}

fn validate_snapshot_id(snapshot_id: &str) -> Result<(), ApiError> {
    if snapshot_id.len() == 64
        && snapshot_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Ok(());
    }
    Err(ApiError::new(
        "invalid_selection",
        "Snapshot ID must be 64 lowercase hexadecimal digits",
        json!({}),
        "Copy the snapshot-bound selection returned by the tool.",
    ))
}

fn check_snapshot(profile: &Profile, snapshot_id: &str) -> Result<(), ApiError> {
    if profile.snapshot_id() == snapshot_id {
        return Ok(());
    }
    Err(ApiError::new(
        "snapshot_mismatch",
        "Selection snapshot ID does not match the referenced data",
        json!({"expected_snapshot_id":snapshot_id,"actual_snapshot_id":profile.snapshot_id()}),
        "Copy an unchanged selection from the desired snapshot.",
    ))
}

fn parse<T: DeserializeOwned>(arguments: JsonObject) -> Result<T, ApiError> {
    serde_json::from_value(Value::Object(arguments)).map_err(invalid_request)
}

pub(super) fn invalid_request(error: serde_json::Error) -> ApiError {
    ApiError::new(
        "invalid_request",
        "Query request does not match the input contract",
        json!({"error":error.to_string()}),
        "Use the documented request fields and provide one profile or selection.",
    )
}

pub(super) fn request_too_large() -> ApiError {
    ApiError::new(
        "query_budget_exceeded",
        "Tool request exceeds its encoded byte limit",
        json!({"limit_bytes":MAX_REQUEST_BYTES}),
        "Use a shorter query with a smaller predicate.",
    )
}

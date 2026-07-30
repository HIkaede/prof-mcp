use std::sync::Arc;

use anyhow::Result;
use rmcp::{
    RoleServer, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
    tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::{
    cache::{LoadedProfile, ProfileCache},
    config::Config,
    error::ApiError,
    query::{self, DiffSort, FrameSelector, FrameWindow, MatchMode, TopSort},
};

pub struct ProfileServer {
    cache: Arc<ProfileCache>,
    tool_router: ToolRouter<Self>,
}

impl ProfileServer {
    pub fn new(config: Config) -> Result<Self, ApiError> {
        let workspace = std::env::current_dir().map_err(|error| {
            ApiError::new(
                "internal_error",
                format!("Could not determine current directory: {error}"),
                json!({}),
                "Start prof-mcp from an accessible workspace directory.",
            )
        })?;
        Self::new_in_workspace(config, workspace)
    }
    pub fn new_in_workspace(
        config: Config,
        workspace: std::path::PathBuf,
    ) -> Result<Self, ApiError> {
        let max_file_size = config.max_file_size_bytes();
        let cache = Arc::new(ProfileCache::new(
            workspace,
            max_file_size,
            config.cache_capacity,
        )?);
        Ok(Self {
            cache,
            tool_router: Self::tool_router(),
        })
    }
    async fn profile(&self, reference: Option<&str>) -> Result<LoadedProfile, ApiError> {
        self.cache.load(reference).await
    }
}

pub async fn run_stdio(config: Config) -> Result<()> {
    let server = ProfileServer::new(config).map_err(anyhow::Error::msg)?;
    server
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum FrameSelectorInput {
    ById(FrameIdInput),
    ByName(FrameNameInput),
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameIdInput {
    pub frame_id: u32,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameNameInput {
    pub frame_name: String,
}
#[allow(dead_code)]
#[derive(JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
enum FindModeSchema {
    Contains,
    Regex,
}
#[allow(dead_code)]
#[derive(JsonSchema, Serialize)]
enum MetricSchema {
    #[serde(rename = "self")]
    SelfWeight,
    #[serde(rename = "inclusive")]
    Inclusive,
}
#[allow(dead_code)]
#[derive(JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
enum DiffSortSchema {
    Regression,
    Improvement,
    Absolute,
}
impl From<FrameSelectorInput> for FrameSelector {
    fn from(value: FrameSelectorInput) -> Self {
        match value {
            FrameSelectorInput::ById(value) => Self {
                frame_id: Some(value.frame_id),
                frame_name: None,
            },
            FrameSelectorInput::ByName(value) => Self {
                frame_id: None,
                frame_name: Some(value.frame_name),
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileInput {
    #[serde(default)]
    pub profile: Option<String>,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindInput {
    #[serde(default)]
    pub profile: Option<String>,
    pub query: String,
    #[serde(default = "default_contains")]
    #[schemars(with = "FindModeSchema")]
    pub mode: String,
    #[serde(default = "default_find_limit")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: usize,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TopInput {
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default = "default_self")]
    #[schemars(with = "MetricSchema")]
    pub sort: String,
    #[serde(default = "default_top_limit")]
    #[schemars(range(min = 1, max = 200))]
    pub limit: usize,
    pub focus: Option<FrameSelectorInput>,
    pub name_regex: Option<String>,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TreeInput {
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub root_node_id: u32,
    pub profile_fingerprint: Option<String>,
    #[serde(default = "default_tree_depth")]
    #[schemars(range(min = 0, max = 16))]
    pub max_depth: usize,
    #[serde(default = "default_tree_nodes")]
    #[schemars(range(min = 1, max = 512))]
    pub max_nodes: usize,
    #[serde(default = "default_tree_percent")]
    #[schemars(range(min = 0.0, max = 100.0))]
    pub min_scope_percent: f64,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DirectionInput {
    #[serde(default)]
    pub profile: Option<String>,
    pub frame: FrameSelectorInput,
    #[serde(default = "default_direction_depth")]
    #[schemars(range(min = 0, max = 16))]
    pub max_depth: usize,
    #[serde(default = "default_tree_nodes")]
    #[schemars(range(min = 1, max = 512))]
    pub max_nodes: usize,
    #[serde(default = "default_tree_percent")]
    #[schemars(range(min = 0.0, max = 100.0))]
    pub min_scope_percent: f64,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PathsInput {
    #[serde(default)]
    pub profile: Option<String>,
    pub through: FrameSelectorInput,
    #[serde(default = "default_paths_limit")]
    #[schemars(range(min = 1, max = 50))]
    pub limit: usize,
    pub frame_window: Option<FrameWindowInput>,
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum FrameWindowInput {
    Head {
        #[schemars(range(min = 1, max = 4096))]
        lines: usize,
    },
    Tail {
        #[schemars(range(min = 1, max = 4096))]
        lines: usize,
    },
    AroundTarget {
        #[schemars(range(min = 0, max = 4096))]
        before: usize,
        #[schemars(range(min = 0, max = 4096))]
        after: usize,
    },
}
impl From<FrameWindowInput> for FrameWindow {
    fn from(value: FrameWindowInput) -> Self {
        match value {
            FrameWindowInput::Head { lines } => Self::Head { lines },
            FrameWindowInput::Tail { lines } => Self::Tail { lines },
            FrameWindowInput::AroundTarget { before, after } => {
                Self::AroundTarget { before, after }
            }
        }
    }
}
#[derive(Clone, Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiffInput {
    pub baseline: String,
    pub candidate: String,
    #[serde(default = "default_self")]
    #[schemars(with = "MetricSchema")]
    pub metric: String,
    #[serde(default = "default_regression")]
    #[schemars(with = "DiffSortSchema")]
    pub sort: String,
    #[serde(default = "default_diff_limit")]
    #[schemars(range(min = 1, max = 200))]
    pub limit: usize,
    pub name_regex: Option<String>,
}
fn default_contains() -> String {
    "contains".into()
}
fn default_self() -> String {
    "self".into()
}
fn default_regression() -> String {
    "regression".into()
}
fn default_find_limit() -> usize {
    20
}
fn default_top_limit() -> usize {
    20
}
fn default_tree_depth() -> usize {
    4
}
fn default_direction_depth() -> usize {
    5
}
fn default_tree_nodes() -> usize {
    64
}
fn default_tree_percent() -> f64 {
    0.1
}
fn default_paths_limit() -> usize {
    10
}
fn default_diff_limit() -> usize {
    30
}

fn output_schema(required: &[&str], scope_weight: Value) -> Arc<serde_json::Map<String, Value>> {
    let profile = serde_json::json!({
        "type":"object",
        "properties":{"canonical_path":{"type":"string"},"fingerprint":{"type":"string"},"byte_len":{"type":"integer"},"modified_unix_ms":{"type":["integer","null"]},"alias":{"type":"string"}},
        "required":["canonical_path","fingerprint","byte_len","modified_unix_ms","alias"]
    });
    let properties = serde_json::json!({
        "schema_version":{"type":"string","const":"2"},
        "profile":profile.clone(),
        "baseline":profile.clone(),
        "candidate":profile,
        "scope_weight":scope_weight,
        "truncated":{"type":"boolean"},
        "warnings":{"type":"array","items":{"type":"string"}},
        "data":{"type":"object"}
    });
    let error = serde_json::json!({
        "type":"object",
        "properties":{"code":{"type":"string"},"message":{"type":"string"},"details":{},"retry_hint":{"type":"string"}},
        "required":["code","message","details","retry_hint"]
    });
    Arc::new(
        serde_json::json!({
            "type":"object",
            "properties":properties,
            "anyOf":[{"required":required},error]
        })
        .as_object()
        .expect("static output schema is object")
        .clone(),
    )
}
fn single_output_schema() -> Arc<serde_json::Map<String, Value>> {
    output_schema(
        &[
            "schema_version",
            "profile",
            "scope_weight",
            "truncated",
            "warnings",
            "data",
        ],
        serde_json::json!({"type":"integer","minimum":0}),
    )
}
fn diff_output_schema() -> Arc<serde_json::Map<String, Value>> {
    output_schema(
        &[
            "schema_version",
            "baseline",
            "candidate",
            "scope_weight",
            "truncated",
            "warnings",
            "data",
        ],
        serde_json::json!({"type":"object","properties":{"baseline":{"type":"integer","minimum":0},"candidate":{"type":"integer","minimum":0}},"required":["baseline","candidate"]}),
    )
}

#[tool_router]
impl ProfileServer {
    #[tool(description = "Summarize a folded stack profile.", output_schema = single_output_schema())]
    async fn profile_summary(&self, Parameters(input): Parameters<ProfileInput>) -> CallToolResult {
        match self.profile(input.profile.as_deref()).await {
            Ok(loaded) => success(
                tag_alias(query::summary(&loaded.profile), &loaded.alias),
                "Profile summary returned; next use profile_top or profile_find_symbols.",
            ),
            Err(error) => failure(error),
        }
    }
    #[tool(description = "Find exact folded-frame identities by contains or regex.", output_schema = single_output_schema())]
    async fn profile_find_symbols(
        &self,
        Parameters(input): Parameters<FindInput>,
    ) -> CallToolResult {
        match self
            .profile(input.profile.as_deref())
            .await
            .and_then(|loaded| {
                query::find_symbols(
                    &loaded.profile,
                    &input.query,
                    parse_match(&input.mode)?,
                    input.limit,
                )
                .map(|value| tag_alias(value, &loaded.alias))
            }) {
            Ok(value) => success(
                value,
                "Symbol matches returned; use a frame_id in a focused query.",
            ),
            Err(error) => failure(error),
        }
    }
    #[tool(description = "Rank self or inclusive folded-frame weights.", output_schema = single_output_schema())]
    async fn profile_top(&self, Parameters(input): Parameters<TopInput>) -> CallToolResult {
        let focus = input.focus.map(FrameSelector::from);
        match self
            .profile(input.profile.as_deref())
            .await
            .and_then(|loaded| {
                query::top(
                    &loaded.profile,
                    parse_metric(&input.sort)?,
                    input.limit,
                    focus.as_ref(),
                    input.name_regex.as_deref(),
                )
                .map(|value| tag_alias(value, &loaded.alias))
            }) {
            Ok(value) => success(
                value,
                "Ranked frames returned; use profile_tree, callers, or callees for context.",
            ),
            Err(error) => failure(error),
        }
    }
    #[tool(description = "Expand one deterministic top-down CCT node.", output_schema = single_output_schema())]
    async fn profile_tree(&self, Parameters(input): Parameters<TreeInput>) -> CallToolResult {
        match self
            .profile(input.profile.as_deref())
            .await
            .and_then(|loaded| {
                query::tree(
                    &loaded.profile,
                    input.root_node_id,
                    input.profile_fingerprint.as_deref(),
                    input.max_depth,
                    input.max_nodes,
                    input.min_scope_percent,
                )
                .map(|value| tag_alias(value, &loaded.alias))
            }) {
            Ok(value) => success(
                value,
                "Tree page returned; pass its fingerprint for any non-root continuation.",
            ),
            Err(error) => failure(error),
        }
    }
    #[tool(description = "Show callers of one exact folded-frame identity.", output_schema = single_output_schema())]
    async fn profile_callers(
        &self,
        Parameters(input): Parameters<DirectionInput>,
    ) -> CallToolResult {
        let frame = FrameSelector::from(input.frame);
        match self
            .profile(input.profile.as_deref())
            .await
            .and_then(|loaded| {
                query::callers(
                    &loaded.profile,
                    &frame,
                    input.max_depth,
                    input.max_nodes,
                    input.min_scope_percent,
                )
                .map(|value| tag_alias(value, &loaded.alias))
            }) {
            Ok(value) => success(
                value,
                "Caller tree returned; use profile_paths for complete contributing stacks.",
            ),
            Err(error) => failure(error),
        }
    }
    #[tool(description = "Show callees of one exact folded-frame identity.", output_schema = single_output_schema())]
    async fn profile_callees(
        &self,
        Parameters(input): Parameters<DirectionInput>,
    ) -> CallToolResult {
        let frame = FrameSelector::from(input.frame);
        match self
            .profile(input.profile.as_deref())
            .await
            .and_then(|loaded| {
                query::callees(
                    &loaded.profile,
                    &frame,
                    input.max_depth,
                    input.max_nodes,
                    input.min_scope_percent,
                )
                .map(|value| tag_alias(value, &loaded.alias))
            }) {
            Ok(value) => success(
                value,
                "Callee tree returned; use profile_paths for complete contributing stacks.",
            ),
            Err(error) => failure(error),
        }
    }
    #[tool(description = "Return heavy complete stacks through one exact frame.", output_schema = single_output_schema())]
    async fn profile_paths(&self, Parameters(input): Parameters<PathsInput>) -> CallToolResult {
        let through = FrameSelector::from(input.through);
        let frame_window = input.frame_window.map(FrameWindow::from);
        match self
            .profile(input.profile.as_deref())
            .await
            .and_then(|loaded| {
                query::paths_with_window(&loaded.profile, &through, input.limit, frame_window)
                    .map(|value| tag_alias(value, &loaded.alias))
            }) {
            Ok(value) => success(
                value,
                "Heavy paths returned; inspect target_positions for recursive occurrences.",
            ),
            Err(error) => failure(error),
        }
    }
    #[tool(description = "Compare exact frame names between two folded profiles.", output_schema = diff_output_schema())]
    async fn profile_diff(&self, Parameters(input): Parameters<DiffInput>) -> CallToolResult {
        match async {
            let baseline = self.profile(Some(&input.baseline)).await?;
            let candidate = self.profile(Some(&input.candidate)).await?;
            query::diff(
                &baseline.profile,
                &candidate.profile,
                parse_metric(&input.metric)?,
                parse_diff_sort(&input.sort)?,
                input.limit,
                input.name_regex.as_deref(),
            )
            .map(|value| tag_diff_aliases(value, &baseline.alias, &candidate.alias))
        }
        .await
        {
            Ok(value) => success(
                value,
                "Profile diff returned; percentage-point changes are not causal evidence.",
            ),
            Err(error) => failure(error),
        }
    }
}

impl ServerHandler for ProfileServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Read-only deterministic queries over folded stack profiles.")
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let arguments = request.arguments.unwrap_or_default();
        let result = match request.name.as_ref() {
            "profile_summary" => {
                self.profile_summary(Parameters(parse_input(&arguments)?))
                    .await
            }
            "profile_find_symbols" => {
                self.profile_find_symbols(Parameters(parse_input(&arguments)?))
                    .await
            }
            "profile_top" => self.profile_top(Parameters(parse_input(&arguments)?)).await,
            "profile_tree" => {
                self.profile_tree(Parameters(parse_input(&arguments)?))
                    .await
            }
            "profile_callers" => {
                self.profile_callers(Parameters(parse_input(&arguments)?))
                    .await
            }
            "profile_callees" => {
                self.profile_callees(Parameters(parse_input(&arguments)?))
                    .await
            }
            "profile_paths" => {
                self.profile_paths(Parameters(parse_input(&arguments)?))
                    .await
            }
            "profile_diff" => {
                self.profile_diff(Parameters(parse_input(&arguments)?))
                    .await
            }
            _ => {
                return Err(rmcp::ErrorData::method_not_found::<
                    rmcp::model::CallToolRequestMethod,
                >());
            }
        };
        Ok(result.into())
    }
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let names = [
            "profile_summary",
            "profile_find_symbols",
            "profile_top",
            "profile_tree",
            "profile_callers",
            "profile_callees",
            "profile_paths",
            "profile_diff",
        ];
        Ok(ListToolsResult {
            tools: names
                .iter()
                .filter_map(|name| self.tool_router.get(name).cloned())
                .collect(),
            ..Default::default()
        })
    }
    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }
}

fn parse_metric(value: &str) -> Result<TopSort, ApiError> {
    match value {
        "self" => Ok(TopSort::SelfWeight),
        "inclusive" => Ok(TopSort::Inclusive),
        _ => Err(ApiError::new(
            "invalid_budget",
            "sort/metric must be self or inclusive",
            json!({"value":value}),
            "Use one documented enum value.",
        )),
    }
}
fn parse_input<T: DeserializeOwned>(
    arguments: &serde_json::Map<String, Value>,
) -> Result<T, rmcp::ErrorData> {
    serde_json::from_value(Value::Object(arguments.clone())).map_err(|error| {
        rmcp::ErrorData::invalid_params(format!("invalid tool arguments: {error}"), None)
    })
}
fn parse_match(value: &str) -> Result<MatchMode, ApiError> {
    match value {
        "contains" => Ok(MatchMode::Contains),
        "regex" => Ok(MatchMode::Regex),
        _ => Err(ApiError::new(
            "invalid_budget",
            "mode must be contains or regex",
            json!({"mode":value}),
            "Use one documented enum value.",
        )),
    }
}
fn parse_diff_sort(value: &str) -> Result<DiffSort, ApiError> {
    match value {
        "regression" => Ok(DiffSort::Regression),
        "improvement" => Ok(DiffSort::Improvement),
        "absolute" => Ok(DiffSort::Absolute),
        _ => Err(ApiError::new(
            "invalid_budget",
            "diff sort must be regression, improvement, or absolute",
            json!({"sort":value}),
            "Use one documented enum value.",
        )),
    }
}
fn success(value: Value, text: &str) -> CallToolResult {
    let truncated = value
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut result = CallToolResult::structured(value);
    result.content = vec![ContentBlock::text(format!("truncated={truncated}; {text}"))];
    result
}
fn tag_alias(mut value: Value, alias: &str) -> Value {
    if let Some(profile) = value.get_mut("profile").and_then(Value::as_object_mut) {
        profile.insert("alias".into(), Value::String(alias.into()));
    }
    value
}
fn tag_diff_aliases(mut value: Value, baseline: &str, candidate: &str) -> Value {
    if let Some(profile) = value.get_mut("baseline").and_then(Value::as_object_mut) {
        profile.insert("alias".into(), Value::String(baseline.into()));
    }
    if let Some(profile) = value.get_mut("candidate").and_then(Value::as_object_mut) {
        profile.insert("alias".into(), Value::String(candidate.into()));
    }
    value
}
fn failure(error: ApiError) -> CallToolResult {
    let mut result=CallToolResult::structured_error(serde_json::to_value(&error).unwrap_or_else(|_| json!({"code":"internal_error","message":"Could not serialize error","details":null,"retry_hint":"Retry."})));
    result.content = vec![ContentBlock::text(format!(
        "{}: {}",
        error.code,
        safe_text(&error.message)
    ))];
    result
}
fn safe_text(input: &str) -> String {
    input
        .chars()
        .map(|character| {
            if character.is_control() {
                '�'
            } else {
                character
            }
        })
        .collect()
}

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
    registry,
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
#[schemars(schema_with = "frame_selector_schema")]
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

fn frame_selector_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "oneOf":[
            {
                "type":"object",
                "additionalProperties":false,
                "properties":{"frame_id":{"type":"integer","minimum":0}},
                "required":["frame_id"]
            },
            {
                "type":"object",
                "additionalProperties":false,
                "properties":{"frame_name":{"type":"string"}},
                "required":["frame_name"]
            }
        ]
    })
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
        "truncation_reasons":{"type":"array","items":{"type":"object","properties":{"kind":{"type":"string"}},"required":["kind"]}},
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
            "truncation_reasons",
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
            "truncation_reasons",
            "warnings",
            "data",
        ],
        serde_json::json!({"type":"object","properties":{"baseline":{"type":"integer","minimum":0},"candidate":{"type":"integer","minimum":0}},"required":["baseline","candidate"]}),
    )
}

#[tool_router]
impl ProfileServer {
    #[tool(description = "Summarize a folded stack profile and list registry aliases.", output_schema = single_output_schema())]
    async fn profile_summary(&self, Parameters(input): Parameters<ProfileInput>) -> CallToolResult {
        match self
            .profile(input.profile.as_deref())
            .await
            .and_then(|loaded| {
                let status = registry::status(self.cache.workspace())?;
                Ok(tag_registry(
                    tag_alias(query::summary(&loaded.profile), &loaded.alias),
                    status,
                ))
            }) {
            Ok(value) => success(
                value,
                "Next use profile_find_symbols, then focused callers/callees/paths.",
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
    let mut result = CallToolResult::structured(value);
    let fallback = text_fallback(
        result
            .structured_content
            .as_ref()
            .expect("structured result retains content"),
        text,
    );
    result.content = vec![ContentBlock::text(fallback)];
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
fn tag_registry(mut value: Value, status: registry::RegistryStatus) -> Value {
    const REGISTRY_PROFILE_LIMIT: usize = 100;
    let available = status.profiles.len();
    let returned = available.min(REGISTRY_PROFILE_LIMIT);
    let mut profiles = Vec::with_capacity(returned);
    if let Some(active) = status
        .profiles
        .iter()
        .find(|profile| profile.alias == status.active)
    {
        profiles.push(active.clone());
    }
    profiles.extend(
        status
            .profiles
            .iter()
            .filter(|profile| profile.alias != status.active)
            .take(REGISTRY_PROFILE_LIMIT.saturating_sub(profiles.len()))
            .cloned(),
    );
    let mut registry_value = serde_json::to_value(status).unwrap_or_else(|_| json!({}));
    if let Some(registry) = registry_value.as_object_mut() {
        registry.insert("profile_count".into(), json!(available));
        registry.insert(
            "profiles".into(),
            serde_json::to_value(profiles).unwrap_or_else(|_| json!([])),
        );
    }
    if let Some(data) = value.get_mut("data").and_then(Value::as_object_mut) {
        data.insert("registry".into(), registry_value);
    }
    if available > REGISTRY_PROFILE_LIMIT
        && let Some(root) = value.as_object_mut()
    {
        root.insert("truncated".into(), Value::Bool(true));
        root.entry("truncation_reasons")
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .expect("query envelopes have a truncation_reasons array")
            .push(json!({
                "kind":"registry_profile_limit",
                "limit":REGISTRY_PROFILE_LIMIT,
                "returned":returned,
                "available":available,
                "omitted":available-returned,
            }));
    }
    value
}

fn text_fallback(value: &Value, next: &str) -> String {
    let truncated = value["truncated"].as_bool().unwrap_or(false);
    let reasons = value["truncation_reasons"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["kind"].as_str())
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_default();
    let alias = value["profile"]["alias"]
        .as_str()
        .or_else(|| value["candidate"]["alias"].as_str())
        .unwrap_or("-");
    let data = &value["data"];
    let detail = if let Some(total) = data["total_weight"].as_u64() {
        let top = data["top_self"]
            .as_array()
            .into_iter()
            .flatten()
            .take(3)
            .filter_map(|row| {
                Some(format!(
                    "{}:{}",
                    row["name"].as_str()?,
                    row["self_weight"].as_u64()?
                ))
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "total_weight={total}, frames={}, stacks={}, top_self=[{top}]",
            data["frame_count"].as_u64().unwrap_or(0),
            data["unique_stack_count"].as_u64().unwrap_or(0)
        )
    } else if let Some(matches) = data["matches"].as_array() {
        let names = matches
            .iter()
            .take(5)
            .filter_map(|row| row["name"].as_str())
            .collect::<Vec<_>>()
            .join(", ");
        format!("matches={} [{names}]", matches.len())
    } else if let Some(paths) = data["paths"].as_array() {
        let window = paths.first().map_or_else(
            || "no paths".to_owned(),
            |path| {
                format!(
                    "first_window={}..{}/{} target_display={}",
                    path["frame_start"].as_u64().unwrap_or(0),
                    path["frame_end"].as_u64().unwrap_or(0),
                    path["total_depth"].as_u64().unwrap_or(0),
                    path["display_target_positions"]
                )
            },
        );
        format!("paths={} {window}", paths.len())
    } else if let Some(rows) = data["rows"].as_array() {
        if rows.first().is_some_and(|row| row["delta_pp"].is_number()) {
            let changes = rows
                .iter()
                .take(5)
                .filter_map(|row| {
                    Some(format!(
                        "{}:{:+.4}pp",
                        row["name"].as_str()?,
                        row["delta_pp"].as_f64()?
                    ))
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("rows={} changes=[{changes}]", rows.len())
        } else {
            let names = rows
                .iter()
                .take(5)
                .filter_map(|row| row["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!("rows={} [{names}]", rows.len())
        }
    } else if let Some(frame) = data["frame"]["name"].as_str() {
        format!("frame={frame}, scope_weight={}", value["scope_weight"])
    } else if let Some(continuations) = data["continuations"].as_array() {
        format!(
            "tree_root={}, continuations={}",
            data["root"]["name"],
            continuations.len()
        )
    } else {
        format!("scope_weight={}", value["scope_weight"])
    };
    bounded_text(&format!(
        "profile={alias}; truncated={truncated}; reasons=[{reasons}]; {detail}; {next}"
    ))
}
fn failure(error: ApiError) -> CallToolResult {
    let mut result=CallToolResult::structured_error(serde_json::to_value(&error).unwrap_or_else(|_| json!({"code":"internal_error","message":"Could not serialize error","details":null,"retry_hint":"Retry."})));
    result.content = vec![ContentBlock::text(bounded_text(&format!(
        "{}: {}",
        error.code, error.message
    )))];
    result
}
fn bounded_text(input: &str) -> String {
    const TEXT_LIMIT_BYTES: usize = 2048;
    let mut output = String::with_capacity(input.len().min(TEXT_LIMIT_BYTES));
    for character in input.chars() {
        let character = if character.is_control() {
            '�'
        } else {
            character
        };
        if output.len().saturating_add(character.len_utf8()) > TEXT_LIMIT_BYTES {
            break;
        }
        output.push(character);
    }
    output
}

#[cfg(test)]
mod text_tests {
    use super::*;

    #[test]
    fn text_fallback_is_utf8_boundary_safe_and_control_safe() {
        let long = format!("{}\u{0000}\u{0007}", "多字节🙂".repeat(1024));
        let text = bounded_text(&long);
        assert!(text.len() <= 2048);
        assert!(!text.chars().any(char::is_control));
        assert!(std::str::from_utf8(text.as_bytes()).is_ok());
        assert!(text.ends_with('🙂') || text.ends_with('字') || text.ends_with('节'));

        let failure = failure(ApiError::new("invalid_test", long, json!({}), "retry"));
        let fallback = &failure.content[0]
            .as_text()
            .expect("failure must include text content")
            .text;
        assert!(fallback.len() <= 2048);
        assert!(!fallback.chars().any(char::is_control));
    }

    #[test]
    fn summary_bounds_registry_aliases_with_an_explicit_reason() {
        let profiles = (0..101)
            .map(|index| registry::RegistryProfile {
                alias: format!("p{index}"),
                fingerprint: "a".repeat(64),
                source_name: "sample.folded".into(),
                byte_len: 1,
                registered_unix_ms: 0,
            })
            .collect();
        let value = tag_registry(
            json!({"truncated":false,"truncation_reasons":[],"data":{}}),
            registry::RegistryStatus {
                registry_root: std::path::PathBuf::from(".prof-mcp"),
                active: "p100".into(),
                profiles,
            },
        );
        assert_eq!(value["data"]["registry"]["active"], "p100");
        assert_eq!(value["data"]["registry"]["profile_count"], 101);
        assert_eq!(
            value["data"]["registry"]["profiles"]
                .as_array()
                .unwrap()
                .len(),
            100
        );
        assert_eq!(value["data"]["registry"]["profiles"][0]["alias"], "p100");
        assert_eq!(value["truncated"], true);
        assert_eq!(
            value["truncation_reasons"][0]["kind"],
            "registry_profile_limit"
        );
        assert_eq!(value["truncation_reasons"][0]["omitted"], 1);
    }
}

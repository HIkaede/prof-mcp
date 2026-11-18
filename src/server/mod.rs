//! stdio MCP server: tool routing, input/output contracts, and response shaping.
//!
//! Tool entry points live in [`tools`]; input contracts in [`inputs`]; typed
//! output envelopes in [`crate::output`]; response tagging and text
//! fallbacks in [`respond`].

use std::sync::Arc;

use anyhow::Result;
use rmcp::{
    RoleServer, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext},
    model::{
        CacheScope, CallToolRequestParams, CallToolResponse, Implementation, ListToolsResult,
        PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
};

mod experimental;
mod inputs;
mod jsonl;
mod respond;
#[cfg(test)]
mod tests;
mod tools;

use serde_json::json;

use crate::cache::{LoadedProfile, ProfileCache};
use crate::config::{Config, OutputFormat, Toolset};
use crate::error::ApiError;

pub use jsonl::run_jsonl;

/// Tools clients see, in the order they are listed. `tests::listed_tools_match_the_router`
/// keeps this in sync with the router the `#[tool_router]` macro builds.
const TOOL_NAMES: [&str; 8] = [
    "profile_summary",
    "profile_find_symbols",
    "profile_top",
    "profile_tree",
    "profile_callers",
    "profile_callees",
    "profile_paths",
    "profile_diff",
];

#[derive(Clone)]
pub struct ProfileServer {
    cache: Arc<ProfileCache>,
    query_gate: Arc<tokio::sync::Semaphore>,
    top_workspace: Arc<tokio::sync::Mutex<crate::query::TopWorkspace>>,
    tool_router: ToolRouter<Self>,
    toolset: Toolset,
    output_format: OutputFormat,
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
        let cache = Arc::new(
            ProfileCache::new(workspace, max_file_size, 8)?
                .with_byte_budget(config.max_cache_bytes()),
        );
        Ok(Self {
            cache,
            query_gate: Arc::new(tokio::sync::Semaphore::new(1)),
            top_workspace: Arc::new(tokio::sync::Mutex::new(
                crate::query::TopWorkspace::default(),
            )),
            tool_router: Self::build_tool_router(),
            toolset: Toolset::default(),
            output_format: OutputFormat::default(),
        })
    }

    pub fn with_options(mut self, toolset: Toolset, output_format: OutputFormat) -> Self {
        self.toolset = toolset;
        self.output_format = output_format;
        self
    }

    async fn profile(&self, reference: Option<&str>) -> Result<LoadedProfile, ApiError> {
        self.cache.load(reference).await
    }

    async fn query<T: Send + 'static>(
        &self,
        load: impl std::future::Future<Output = Result<T, ApiError>> + Send,
        run: impl FnOnce(T) -> Result<rmcp::model::CallToolResult, ApiError> + Send + 'static,
    ) -> rmcp::model::CallToolResult {
        self.work(load, run).await.unwrap_or_else(respond::failure)
    }

    async fn work<T: Send + 'static, U: Send + 'static>(
        &self,
        load: impl std::future::Future<Output = Result<T, ApiError>> + Send,
        run: impl FnOnce(T) -> Result<U, ApiError> + Send + 'static,
    ) -> Result<U, ApiError> {
        // Bound cold loading, computation, and response shaping as one heavy operation.
        let permit = self
            .query_gate
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| ApiError::internal("Query worker unavailable"))?;
        let loaded = load.await?;
        tokio::task::spawn_blocking(move || {
            // Cancellation retains the permit until computation and response shaping finish.
            let _permit = permit;
            run(loaded)
        })
        .await
        .map_err(|_| ApiError::internal("Query worker failed"))?
    }
}

pub async fn run_stdio(config: Config) -> Result<()> {
    run_stdio_with_options(config, Toolset::default(), OutputFormat::default()).await
}

pub async fn run_stdio_with_options(
    config: Config,
    toolset: Toolset,
    output_format: OutputFormat,
) -> Result<()> {
    let server = ProfileServer::new(config)
        .map_err(anyhow::Error::msg)?
        .with_options(toolset, output_format);
    server
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}

impl ServerHandler for ProfileServer {
    fn get_info(&self) -> ServerConfig {
        let instructions = match self.toolset {
            Toolset::Legacy => {
                "Read-only folded profile queries. Omit profile for the active alias. Resume with continuations[i].continuation unchanged."
            }
            Toolset::Experimental => {
                "Read-only folded profile queries. Start with profile_explore or query a view directly. Omit source for the active profile. For numeric references or pagination, copy the result selection into source.selection."
            }
        };
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("prof-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(instructions)
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        if self.toolset == Toolset::Experimental {
            if !experimental::TOOL_NAMES.contains(&request.name.as_ref()) {
                return Err(rmcp::ErrorData::invalid_params("tool not found", None));
            }
            let cancellation = context.ct;
            let result = self
                .experimental_call(
                    &request.name,
                    request.arguments.unwrap_or_default(),
                    move || cancellation.is_cancelled(),
                )
                .await;
            return Ok(respond::experimental(result, self.output_format).into());
        }
        self.tool_router
            .call(ToolCallContext::new(self, request, context))
            .await
            .map(|response| match response {
                CallToolResponse::Complete(result) => {
                    respond::render_legacy(result, self.output_format).into()
                }
                response => response,
            })
    }
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let tools = match self.toolset {
            Toolset::Legacy => TOOL_NAMES
                .iter()
                .filter_map(|name| self.tool_router.get(name).cloned())
                .collect(),
            Toolset::Experimental => experimental::TOOL_NAMES
                .iter()
                .map(|name| experimental::tool(name).expect("experimental tool definition"))
                .collect(),
        };
        let mut result = ListToolsResult::with_all_items(tools);
        if context
            .protocol_version()
            .is_some_and(|version| version >= ProtocolVersion::V_2026_07_28)
        {
            result = result.with_ttl_ms(0).with_cache_scope(CacheScope::Private);
        }
        Ok(result)
    }
    fn get_tool(&self, name: &str) -> Option<Tool> {
        match self.toolset {
            Toolset::Legacy => self.tool_router.get(name).cloned(),
            Toolset::Experimental => experimental::tool(name),
        }
    }
}

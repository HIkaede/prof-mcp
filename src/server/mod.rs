//! stdio MCP server: tool routing, input/output contracts, and response shaping.
//!
//! Tool entry points live in [`tools`]; input contracts in [`inputs`]; typed
//! output envelopes and schemas in [`outputs`]; response tagging and text
//! fallbacks in [`respond`].

use std::sync::Arc;

use anyhow::Result;
use rmcp::{
    RoleServer, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CacheScope, CallToolRequestParams, CallToolResponse, ListToolsResult,
        PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
};

mod inputs;
mod outputs;
mod respond;
#[cfg(test)]
mod tests;
mod tools;

use inputs::parse_input;
use serde_json::json;

use crate::cache::{LoadedProfile, ProfileCache};
use crate::config::Config;
use crate::error::ApiError;

#[derive(Clone)]
pub struct ProfileServer {
    cache: Arc<ProfileCache>,
    query_gate: Arc<tokio::sync::Semaphore>,
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
        let cache = Arc::new(ProfileCache::new(workspace, max_file_size, 8)?);
        Ok(Self {
            cache,
            query_gate: Arc::new(tokio::sync::Semaphore::new(1)),
            tool_router: Self::build_tool_router(),
        })
    }
    async fn profile(&self, reference: Option<&str>) -> Result<LoadedProfile, ApiError> {
        self.cache.load(reference).await
    }

    async fn query<T: Send + 'static>(
        &self,
        load: impl std::future::Future<Output = Result<T, ApiError>> + Send,
        run: impl FnOnce(T) -> Result<serde_json::Value, ApiError> + Send + 'static,
    ) -> rmcp::model::CallToolResult {
        let permit = match self.query_gate.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => return respond::failure(ApiError::internal("Query worker unavailable")),
        };
        let loaded = match load.await {
            Ok(loaded) => loaded,
            Err(error) => return respond::failure(error),
        };
        tokio::task::spawn_blocking(move || {
            // Cancellation retains the permit until computation and response shaping finish.
            let _permit = permit;
            match run(loaded) {
                Ok(value) => respond::success(value),
                Err(error) => respond::failure(error),
            }
        })
        .await
        .unwrap_or_else(|_| respond::failure(ApiError::internal("Query worker failed")))
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

impl ServerHandler for ProfileServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Read-only folded profile queries. Omit profile for the active alias. Resume with continuations[i].continuation unchanged.")
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
        context: RequestContext<RoleServer>,
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
        let mut result = ListToolsResult::with_all_items(
            names
                .iter()
                .filter_map(|name| self.tool_router.get(name).cloned())
                .collect(),
        );
        if context
            .protocol_version()
            .is_some_and(|version| version >= ProtocolVersion::V_2026_07_28)
        {
            result = result.with_ttl_ms(0).with_cache_scope(CacheScope::Private);
        }
        Ok(result)
    }
    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }
}

//! The eight MCP tool entry points.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};

use super::ProfileServer;
use super::inputs::{
    DiffInput, DirectionInput, FindInput, PathsInput, ProfileInput, TopInput, TreeInput,
    parse_diff_sort, parse_match, parse_metric,
};
use super::outputs::{
    DiffOutput, DirectionOutput, PathsOutput, TopOutput, TreeOutput, single_output_schema,
    typed_output, typed_output_schema,
};
use super::respond::{failure, success, tag_alias, tag_diff_aliases, tag_registry};
use crate::query::{self, FrameSelector, FrameWindow};
use crate::registry;

#[tool_router]
impl ProfileServer {
    pub(crate) fn build_tool_router() -> ToolRouter<Self> {
        Self::tool_router()
    }

    #[tool(description = "Summarize a folded stack profile and list registry aliases.", output_schema = single_output_schema())]
    pub(crate) async fn profile_summary(
        &self,
        Parameters(input): Parameters<ProfileInput>,
    ) -> CallToolResult {
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
    pub(crate) async fn profile_find_symbols(
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
    #[tool(description = "Rank self or inclusive folded-frame weights.", output_schema = typed_output_schema::<TopOutput>())]
    pub(crate) async fn profile_top(
        &self,
        Parameters(input): Parameters<TopInput>,
    ) -> CallToolResult {
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
                .and_then(|value| typed_output::<TopOutput>(tag_alias(value, &loaded.alias)))
            }) {
            Ok(value) => success(
                value,
                "Ranked frames returned; use profile_tree, callers, or callees for context.",
            ),
            Err(error) => failure(error),
        }
    }
    #[tool(description = "Expand one deterministic top-down CCT node.", output_schema = typed_output_schema::<TreeOutput>())]
    pub(crate) async fn profile_tree(
        &self,
        Parameters(input): Parameters<TreeInput>,
    ) -> CallToolResult {
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
                .and_then(|value| typed_output::<TreeOutput>(tag_alias(value, &loaded.alias)))
            }) {
            Ok(value) => success(
                value,
                "Tree page returned; pass its fingerprint for any non-root continuation.",
            ),
            Err(error) => failure(error),
        }
    }
    #[tool(description = "Show callers of one exact folded-frame identity.", output_schema = typed_output_schema::<DirectionOutput>())]
    pub(crate) async fn profile_callers(
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
                .and_then(|value| typed_output::<DirectionOutput>(tag_alias(value, &loaded.alias)))
            }) {
            Ok(value) => success(
                value,
                "Caller tree returned; use profile_paths for complete contributing stacks.",
            ),
            Err(error) => failure(error),
        }
    }
    #[tool(description = "Show callees of one exact folded-frame identity.", output_schema = typed_output_schema::<DirectionOutput>())]
    pub(crate) async fn profile_callees(
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
                .and_then(|value| typed_output::<DirectionOutput>(tag_alias(value, &loaded.alias)))
            }) {
            Ok(value) => success(
                value,
                "Callee tree returned; use profile_paths for complete contributing stacks.",
            ),
            Err(error) => failure(error),
        }
    }
    #[tool(description = "Return heavy complete stacks through one exact frame.", output_schema = typed_output_schema::<PathsOutput>())]
    pub(crate) async fn profile_paths(
        &self,
        Parameters(input): Parameters<PathsInput>,
    ) -> CallToolResult {
        let through = FrameSelector::from(input.through);
        let frame_window = input.frame_window.map(FrameWindow::from);
        match self
            .profile(input.profile.as_deref())
            .await
            .and_then(|loaded| {
                query::paths_with_window_budget(
                    &loaded.profile,
                    &through,
                    input.limit,
                    frame_window,
                    input.max_total_frames,
                )
                .and_then(|value| typed_output::<PathsOutput>(tag_alias(value, &loaded.alias)))
            }) {
            Ok(value) => success(
                value,
                "Heavy paths returned; inspect target_positions for recursive occurrences.",
            ),
            Err(error) => failure(error),
        }
    }
    #[tool(description = "Compare exact frame names between two folded profiles.", output_schema = typed_output_schema::<DiffOutput>())]
    pub(crate) async fn profile_diff(
        &self,
        Parameters(input): Parameters<DiffInput>,
    ) -> CallToolResult {
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
            .and_then(|value| {
                typed_output::<DiffOutput>(tag_diff_aliases(
                    value,
                    &baseline.alias,
                    &candidate.alias,
                ))
            })
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

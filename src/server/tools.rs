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
use super::respond::{
    failure, success, tag_alias, tag_diff_aliases, tag_registry, tag_weight_semantics,
};
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
                    tag_weight_semantics(
                        tag_alias(query::summary(&loaded.profile), &loaded.alias),
                        loaded.sample_period_us,
                    ),
                    status,
                ))
            }) {
            Ok(value) => success(
                value,
                "Next use profile_find_symbols, then focused callers/callees/paths.",
                &[
                    "Resolve an exact frame with profile_find_symbols.",
                    "Expand the dominant region with profile_tree {root_node_id:0}.",
                ],
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
                    input.normalize,
                )
                .map(|value| {
                    tag_weight_semantics(tag_alias(value, &loaded.alias), loaded.sample_period_us)
                })
            }) {
            Ok(value) => success(
                value,
                "Symbol matches returned; use a frame_id in a focused query.",
                &["Pass a returned frame_id or exact frame_name to callers/callees/paths/top."],
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
                    input.normalize,
                )
                .and_then(|value| typed_output::<TopOutput>(tag_alias(value, &loaded.alias)))
            }) {
            Ok(value) => success(
                value,
                "Ranked frames returned; use profile_tree, callers, or callees for context.",
                &[
                    "Trace one row with profile_callers or profile_callees.",
                    "Inspect whole stacks with profile_paths through that frame.",
                ],
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
                &["Continue an omitted child via continuations node_id plus profile_fingerprint."],
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
                    input
                        .continuation
                        .as_ref()
                        .map(|c| (c.node_path.as_slice(), c.profile_fingerprint.as_str())),
                )
                .and_then(|value| typed_output::<DirectionOutput>(tag_alias(value, &loaded.alias)))
            }) {
            Ok(value) => success(
                value,
                "Caller tree returned; use profile_paths for complete contributing stacks.",
                &[
                    "Raise max_depth/max_nodes to deepen; continue omitted nodes via continuations node_path.",
                ],
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
                    input
                        .continuation
                        .as_ref()
                        .map(|c| (c.node_path.as_slice(), c.profile_fingerprint.as_str())),
                )
                .and_then(|value| typed_output::<DirectionOutput>(tag_alias(value, &loaded.alias)))
            }) {
            Ok(value) => success(
                value,
                "Callee tree returned; use profile_paths for complete contributing stacks.",
                &[
                    "Raise max_depth/max_nodes to deepen; continue omitted nodes via continuations node_path.",
                ],
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
                &["Use frame_window around_target to focus context around the target frame."],
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
                &[
                    "Re-run with sort=absolute to surface symmetric changes missed by regression order.",
                ],
            ),
            Err(error) => failure(error),
        }
    }
}

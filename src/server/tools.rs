//! The eight MCP tool entry points.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};

use super::ProfileServer;
use super::inputs::{
    Continuation, ContinuationTool, DiffInput, DirectionInput, FindInput, PathsInput, ProfileInput,
    TopInput, TreeInput, parse_diff_sort, parse_match, parse_metric,
};
use super::outputs::{
    DiffOutput, DirectionOutput, FindOutput, PathsOutput, SummaryOutput, TopOutput, TreeOutput,
    typed_output, typed_output_schema,
};
use super::respond::{failure, tag_alias, tag_continuations, tag_diff_aliases, tag_registry};
use crate::error::ApiError;
use crate::query::{self, FrameSelector, FrameWindow};
use crate::registry;

#[tool_router]
impl ProfileServer {
    pub(crate) fn build_tool_router() -> ToolRouter<Self> {
        Self::tool_router()
    }

    #[tool(description = "Summarize a folded stack profile and list registry aliases.", output_schema = typed_output_schema::<SummaryOutput>())]
    pub(crate) async fn profile_summary(
        &self,
        Parameters(input): Parameters<ProfileInput>,
    ) -> CallToolResult {
        let workspace = self.cache.workspace().to_owned();
        self.query(self.profile(input.profile.as_deref()), move |loaded| {
            let status = registry::status(&workspace)?;
            typed_output::<SummaryOutput>(tag_registry(
                tag_alias(query::summary(&loaded.profile), &loaded.alias),
                status,
            ))
        })
        .await
    }
    #[tool(description = "Find exact folded-frame identities by contains or regex.", output_schema = typed_output_schema::<FindOutput>())]
    pub(crate) async fn profile_find_symbols(
        &self,
        Parameters(input): Parameters<FindInput>,
    ) -> CallToolResult {
        self.query(self.profile(input.profile.as_deref()), move |loaded| {
            query::find_symbols(
                &loaded.profile,
                &input.query,
                parse_match(&input.mode)?,
                input.limit,
            )
            .and_then(|value| typed_output::<FindOutput>(tag_alias(value, &loaded.alias)))
        })
        .await
    }
    #[tool(description = "Rank self or inclusive folded-frame weights.", output_schema = typed_output_schema::<TopOutput>())]
    pub(crate) async fn profile_top(
        &self,
        Parameters(input): Parameters<TopInput>,
    ) -> CallToolResult {
        let frame = input.frame.map(FrameSelector::from);
        self.query(self.profile(input.profile.as_deref()), move |loaded| {
            query::top(
                &loaded.profile,
                parse_metric(&input.metric)?,
                input.limit,
                frame.as_ref(),
                input.name_regex.as_deref(),
            )
            .and_then(|value| typed_output::<TopOutput>(tag_alias(value, &loaded.alias)))
        })
        .await
    }
    #[tool(description = "Expand one deterministic top-down CCT node.", output_schema = typed_output_schema::<TreeOutput>())]
    pub(crate) async fn profile_tree(
        &self,
        Parameters(input): Parameters<TreeInput>,
    ) -> CallToolResult {
        let profile = match continuation_profile(
            input.profile,
            input.continuation.as_ref(),
            ContinuationTool::Tree,
            false,
        ) {
            Ok(profile) => profile,
            Err(error) => return failure(error),
        };
        self.query(self.profile(profile.as_deref()), move |loaded| {
            check_fingerprint(
                &loaded.profile.source.fingerprint,
                input.continuation.as_ref(),
            )?;
            let root = input.continuation.as_ref().map_or(0, |c| c.cursor[0]);
            query::tree(
                &loaded.profile,
                root,
                input.continuation.as_ref().map(|c| c.fingerprint.as_str()),
                input.max_depth,
                input.max_nodes,
                input.min_scope_percent,
            )
            .and_then(|value| {
                typed_output::<TreeOutput>(tag_continuations(
                    tag_alias(value, &loaded.alias),
                    &loaded.alias,
                    ContinuationTool::Tree,
                ))
            })
        })
        .await
    }
    #[tool(description = "Show callers of one exact folded-frame identity.", output_schema = typed_output_schema::<DirectionOutput>())]
    pub(crate) async fn profile_callers(
        &self,
        Parameters(input): Parameters<DirectionInput>,
    ) -> CallToolResult {
        self.direction(input, ContinuationTool::Callers).await
    }
    #[tool(description = "Show callees of one exact folded-frame identity.", output_schema = typed_output_schema::<DirectionOutput>())]
    pub(crate) async fn profile_callees(
        &self,
        Parameters(input): Parameters<DirectionInput>,
    ) -> CallToolResult {
        self.direction(input, ContinuationTool::Callees).await
    }

    async fn direction(&self, input: DirectionInput, tool: ContinuationTool) -> CallToolResult {
        let profile = match continuation_profile(
            input.profile,
            input.continuation.as_ref(),
            tool,
            input.frame.is_some(),
        ) {
            Ok(profile) => profile,
            Err(error) => return failure(error),
        };
        let frame = match (input.frame, input.continuation.as_ref()) {
            (Some(frame), None) => FrameSelector::from(frame),
            (None, Some(c)) => FrameSelector {
                frame_id: Some(c.cursor[0]),
                frame_name: None,
            },
            _ => {
                return failure(ApiError::new(
                    "invalid_frame_selector",
                    "Set frame or continuation",
                    serde_json::json!({}),
                    "Use an exact frame or copy a returned continuation.",
                ));
            }
        };
        self.query(self.profile(profile.as_deref()), move |loaded| {
            check_fingerprint(
                &loaded.profile.source.fingerprint,
                input.continuation.as_ref(),
            )?;
            let run = match tool {
                ContinuationTool::Callers => query::callers,
                ContinuationTool::Callees => query::callees,
                ContinuationTool::Tree => unreachable!("direction tool"),
            };
            run(
                &loaded.profile,
                &frame,
                input.max_depth,
                input.max_nodes,
                input.min_scope_percent,
                input
                    .continuation
                    .as_ref()
                    .map(|c| (&c.cursor[1..], c.fingerprint.as_str())),
            )
            .and_then(|value| {
                typed_output::<DirectionOutput>(tag_continuations(
                    tag_alias(value, &loaded.alias),
                    &loaded.alias,
                    tool,
                ))
            })
        })
        .await
    }
    #[tool(description = "Return heavy stacks containing frame; display may be cropped.", output_schema = typed_output_schema::<PathsOutput>())]
    pub(crate) async fn profile_paths(
        &self,
        Parameters(input): Parameters<PathsInput>,
    ) -> CallToolResult {
        let frame = FrameSelector::from(input.frame);
        let frame_window = input.frame_window.map(FrameWindow::from);
        self.query(self.profile(input.profile.as_deref()), move |loaded| {
            query::paths_with_window_budget(
                &loaded.profile,
                &frame,
                input.limit,
                frame_window,
                input.max_total_frames,
            )
            .and_then(|value| typed_output::<PathsOutput>(tag_alias(value, &loaded.alias)))
        })
        .await
    }
    #[tool(description = "Compare exact frame names between two folded profiles.", output_schema = typed_output_schema::<DiffOutput>())]
    pub(crate) async fn profile_diff(
        &self,
        Parameters(input): Parameters<DiffInput>,
    ) -> CallToolResult {
        self.query(
            async {
                let baseline = self.profile(Some(&input.baseline)).await?;
                let candidate = self.profile(Some(&input.candidate)).await?;
                Ok((baseline, candidate))
            },
            move |(baseline, candidate)| {
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
            },
        )
        .await
    }
}

fn continuation_profile(
    profile: Option<String>,
    continuation: Option<&Continuation>,
    tool: ContinuationTool,
    has_frame: bool,
) -> Result<Option<String>, ApiError> {
    let Some(c) = continuation else {
        return Ok(profile);
    };
    let valid_cursor = match tool {
        ContinuationTool::Tree => c.cursor.len() == 1,
        ContinuationTool::Callers | ContinuationTool::Callees => {
            (2..=4097).contains(&c.cursor.len())
        }
    };
    if c.tool != tool
        || profile.is_some()
        || has_frame
        || !valid_cursor
        || !registry::valid_alias(&c.profile)
        || c.fingerprint.len() != 64
        || !c
            .fingerprint
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ApiError::new(
            "invalid_continuation",
            "Invalid or conflicting continuation",
            serde_json::json!({}),
            "Copy a continuation from this tool unchanged; omit profile and frame.",
        ));
    }
    Ok(Some(c.profile.clone()))
}

fn check_fingerprint(
    fingerprint: &str,
    continuation: Option<&Continuation>,
) -> Result<(), ApiError> {
    if let Some(c) = continuation
        && c.fingerprint != fingerprint
    {
        return Err(ApiError::new(
            "profile_changed",
            "Continuation profile has changed",
            serde_json::json!({"expected_fingerprint":c.fingerprint,"current_fingerprint":fingerprint}),
            "Restart the query without continuation.",
        ));
    }
    Ok(())
}

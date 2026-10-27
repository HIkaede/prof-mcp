//! The eight MCP tool entry points.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};

use super::ProfileServer;
use super::inputs::{
    Continuation, ContinuationTool, DiffInput, DirectionInput, FindInput, PathsInput, ProfileInput,
    TopInput, TreeFormat, TreeInput,
};
use super::respond::{failure, success, tag_alias, tag_direction, tag_registry, tag_tree};
use crate::error::ApiError;
use crate::query::{self, FrameSelector, FrameWindow};
use crate::registry;

#[tool_router]
impl ProfileServer {
    pub(crate) fn build_tool_router() -> ToolRouter<Self> {
        Self::tool_router()
    }

    #[tool(description = "Summarize a folded stack profile and list registry aliases.")]
    pub(crate) async fn profile_summary(
        &self,
        Parameters(input): Parameters<ProfileInput>,
    ) -> CallToolResult {
        let workspace = self.cache.workspace().to_owned();
        self.query(
            async {
                let (resolved, status) =
                    registry::resolve_with_status(&workspace, input.profile.as_deref())?;
                Ok((self.cache.load_resolved(resolved).await?, status))
            },
            |(loaded, status)| {
                let mut output = query::summary(&loaded.profile)?;
                tag_alias(&mut output, &loaded.alias);
                tag_registry(&mut output, status);
                Ok(success(output))
            },
        )
        .await
    }
    #[tool(
        description = "Find exact folded-frame identities by contains or regex. `matches` is a table: `columns` names the fields, each `rows` entry holds the values in that order."
    )]
    pub(crate) async fn profile_find_symbols(
        &self,
        Parameters(input): Parameters<FindInput>,
    ) -> CallToolResult {
        self.query(self.profile(input.profile.as_deref()), move |loaded| {
            let mut output =
                query::find_symbols(&loaded.profile, &input.query, input.mode, input.limit)?;
            tag_alias(&mut output, &loaded.alias);
            Ok(success(output))
        })
        .await
    }
    #[tool(
        description = "Rank self or inclusive folded-frame weights. `rows` is a table: `columns` names the fields, each `rows` entry holds the values in that order."
    )]
    pub(crate) async fn profile_top(
        &self,
        Parameters(input): Parameters<TopInput>,
    ) -> CallToolResult {
        let frame = input.frame.map(FrameSelector::from);
        let workspace = self.top_workspace.clone();
        self.query(self.profile(input.profile.as_deref()), move |loaded| {
            let mut workspace = workspace.blocking_lock();
            let result = query::top_with_workspace(
                &loaded.profile,
                input.metric,
                input.limit,
                frame.as_ref(),
                input.name_regex.as_deref(),
                &mut workspace,
            );
            workspace.release_oversized();
            let mut output = result?;
            tag_alias(&mut output, &loaded.alias);
            Ok(success(output))
        })
        .await
    }
    #[tool(
        description = "Expand one deterministic top-down CCT node. Use profile_callees to expand the subtree of a specific frame. Zero-valued node fields are omitted; format=text renders one indented line per node as `name#frame_id total= self= pct%` plus `+omitted(weight)`."
    )]
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
            let root = input
                .continuation
                .as_ref()
                .map_or([0, 0], |c| [c.cursor[0], c.cursor[1]]);
            let mut output = query::tree(
                &loaded.profile,
                root,
                input.continuation.as_ref().map(|c| c.fingerprint.as_str()),
                input.max_depth,
                input.max_nodes,
                input.min_scope_percent,
            )?;
            tag_tree(&mut output, &loaded.alias);
            if input.format == TreeFormat::Text {
                output.data.root = output.data.root.into_text();
            }
            Ok(success(output))
        })
        .await
    }
    #[tool(description = "Show callers of one exact folded-frame identity.")]
    pub(crate) async fn profile_callers(
        &self,
        Parameters(input): Parameters<DirectionInput>,
    ) -> CallToolResult {
        self.direction(input, ContinuationTool::Callers).await
    }
    #[tool(description = "Show callees of one exact folded-frame identity.")]
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
            let mut output = run(
                &loaded.profile,
                &frame,
                input.max_depth,
                input.max_nodes,
                input.min_scope_percent,
                input
                    .continuation
                    .as_ref()
                    .map(|c| (&c.cursor[1..], c.fingerprint.as_str())),
            )?;
            tag_direction(&mut output, &loaded.alias);
            Ok(success(output))
        })
        .await
    }
    #[tool(description = "Return heavy stacks containing frame; display may be cropped.")]
    pub(crate) async fn profile_paths(
        &self,
        Parameters(input): Parameters<PathsInput>,
    ) -> CallToolResult {
        let frame = FrameSelector::from(input.frame);
        let frame_window = input.frame_window.map(FrameWindow::from);
        self.query(self.profile(input.profile.as_deref()), move |loaded| {
            let mut output = query::paths_with_window_budget(
                &loaded.profile,
                &frame,
                input.limit,
                frame_window,
                input.max_total_frames,
            )?;
            tag_alias(&mut output, &loaded.alias);
            Ok(success(output))
        })
        .await
    }
    #[tool(description = "Compare exact frame names between two folded profiles.")]
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
                let mut output = query::diff(
                    &baseline.profile,
                    &candidate.profile,
                    input.metric,
                    input.sort,
                    input.limit,
                    input.name_regex.as_deref(),
                )?;
                output.baseline.alias = &baseline.alias;
                output.candidate.alias = &candidate.alias;
                Ok(success(output))
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
        ContinuationTool::Tree => c.cursor.len() == 2,
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

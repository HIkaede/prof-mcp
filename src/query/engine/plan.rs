use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    error::ApiError,
    profile::{ContextRef, Profile},
    query::{
        TopSort, check_limit,
        selection::{FrameRef, Metric, Predicate},
    },
};

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryPlan {
    #[serde(default)]
    pub refine: Option<Predicate>,
    #[serde(default)]
    pub metric: Metric,
    pub view: View,
    #[serde(default)]
    pub cursor: Option<Cursor>,
}

impl QueryPlan {
    pub fn validate_alias(&self, profile: &Profile) -> Result<(), ApiError> {
        self.alias_refinement(profile).map(|_| ())
    }

    pub(crate) fn alias_refinement(
        &self,
        profile: &Profile,
    ) -> Result<Option<Predicate>, ApiError> {
        let refine = resolve_refinement(profile, self.refine.as_ref())?;
        self.view.validate_alias(profile)?;
        Ok(refine)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExplorePlan {
    #[serde(default)]
    pub refine: Option<Predicate>,
    #[serde(default)]
    pub metric: Metric,
    #[serde(default)]
    pub anchor: Option<Anchor>,
}

impl ExplorePlan {
    pub fn validate_alias(&self, profile: &Profile) -> Result<(), ApiError> {
        self.alias_refinement(profile).map(|_| ())
    }

    pub(crate) fn alias_refinement(
        &self,
        profile: &Profile,
    ) -> Result<Option<Predicate>, ApiError> {
        let refine = resolve_refinement(profile, self.refine.as_ref())?;
        if let Some(anchor) = &self.anchor {
            anchor.validate_alias(profile)?;
        }
        Ok(refine)
    }
}

fn resolve_refinement(
    profile: &Profile,
    refine: Option<&Predicate>,
) -> Result<Option<Predicate>, ApiError> {
    refine
        .map(|predicate| predicate.resolve_alias(profile))
        .transpose()
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(untagged, deny_unknown_fields)]
pub enum Anchor {
    Frame(FrameRef),
    Context { context: ContextRef },
}

impl Anchor {
    fn validate_alias(&self, profile: &Profile) -> Result<(), ApiError> {
        match self {
            Self::Frame(frame) => frame.resolve(profile, false).map(|_| ()),
            Self::Context { .. } => Err(bound_reference_required()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PathRange {
    #[default]
    Full,
    BelowAnchor,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Occurrence {
    Outermost,
    Innermost,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    pub(super) position: usize,
    pub(super) digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NameFilter {
    pub name_exact: Option<String>,
    pub name_regex: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PathWindow {
    Head {
        #[schemars(range(min = 1, max = 256))]
        frames: usize,
    },
    Tail {
        #[schemars(range(min = 1, max = 256))]
        frames: usize,
    },
    AroundTarget {
        target: Anchor,
        #[schemars(range(min = 0, max = 255))]
        before: usize,
        #[schemars(range(min = 0, max = 255))]
        after: usize,
    },
    Full,
}

impl Default for PathWindow {
    fn default() -> Self {
        Self::Tail { frames: 16 }
    }
}

pub const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 200;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum View {
    Frames {
        stat: TopSort,
        #[serde(rename = "where", default)]
        name_filter: Option<NameFilter>,
        #[serde(default)]
        include_zero: bool,
        #[serde(default = "default_limit")]
        #[schemars(range(min = 1, max = 200))]
        limit: usize,
        #[serde(default = "yes")]
        totals: bool,
        #[serde(default)]
        path_range: PathRange,
        #[serde(default)]
        anchor: Option<ContextRef>,
    },
    Contexts {
        frame: FrameRef,
        #[serde(default = "default_limit")]
        #[schemars(range(min = 1, max = 200))]
        limit: usize,
    },
    Children {
        #[serde(default)]
        context: Option<ContextRef>,
        #[serde(default = "default_limit")]
        #[schemars(range(min = 1, max = 200))]
        limit: usize,
    },
    Callers {
        anchor: Anchor,
        #[serde(default)]
        occurrence: Option<Occurrence>,
        #[serde(default = "default_limit")]
        #[schemars(range(min = 1, max = 200))]
        limit: usize,
    },
    Callees {
        anchor: Anchor,
        #[serde(default)]
        occurrence: Option<Occurrence>,
        #[serde(default = "default_limit")]
        #[schemars(range(min = 1, max = 200))]
        limit: usize,
    },
    Paths {
        #[serde(default = "default_limit")]
        #[schemars(range(min = 1, max = 200))]
        limit: usize,
        #[serde(default)]
        window: PathWindow,
    },
    Aggregate {
        #[serde(rename = "where", default)]
        name_filter: Option<NameFilter>,
        #[serde(default)]
        path_range: PathRange,
        #[serde(default)]
        anchor: Option<ContextRef>,
    },
}

impl View {
    pub(super) fn kind(&self) -> ViewKind {
        match self {
            Self::Frames { .. } => ViewKind::Frames,
            Self::Contexts { .. } => ViewKind::Contexts,
            Self::Children { .. } => ViewKind::Children,
            Self::Callers { .. } => ViewKind::Callers,
            Self::Callees { .. } => ViewKind::Callees,
            Self::Paths { .. } => ViewKind::Paths,
            Self::Aggregate { .. } => ViewKind::Aggregate,
        }
    }

    pub(super) fn limit(&self) -> Option<usize> {
        match self {
            Self::Frames { limit, .. }
            | Self::Contexts { limit, .. }
            | Self::Children { limit, .. }
            | Self::Callers { limit, .. }
            | Self::Callees { limit, .. }
            | Self::Paths { limit, .. } => Some(*limit),
            Self::Aggregate { .. } => None,
        }
    }

    pub(super) fn validate(&self) -> Result<(), ApiError> {
        if let Some(limit) = self.limit() {
            check_limit(limit, 1, MAX_LIMIT, "view.limit")?;
        }
        match self {
            Self::Paths {
                window: PathWindow::Head { frames } | PathWindow::Tail { frames },
                ..
            } => check_limit(*frames, 1, 256, "view.window.frames"),
            Self::Paths {
                window: PathWindow::AroundTarget { before, after, .. },
                ..
            } => {
                check_limit(*before, 0, 255, "view.window.before")?;
                check_limit(*after, 0, 255, "view.window.after")?;
                check_limit(before + after, 0, 255, "view.window.before + after")
            }
            Self::Frames {
                path_range, anchor, ..
            }
            | Self::Aggregate {
                path_range, anchor, ..
            } if matches!(path_range, PathRange::BelowAnchor) != anchor.is_some() => {
                Err(ApiError::new(
                    "invalid_view",
                    "Below-anchor projection requires one explicit context anchor",
                    json!({}),
                    "Supply path_range below_anchor together with a context anchor, or omit both.",
                ))
            }
            Self::Callers {
                anchor: Anchor::Context { .. },
                occurrence: Some(_),
                ..
            }
            | Self::Callees {
                anchor: Anchor::Context { .. },
                occurrence: Some(_),
                ..
            } => Err(ApiError::new(
                "invalid_view",
                "A context anchor determines its exact occurrence",
                json!({}),
                "Omit occurrence when using a context anchor.",
            )),
            _ => Ok(()),
        }
    }

    fn validate_alias(&self, profile: &Profile) -> Result<(), ApiError> {
        match self {
            Self::Contexts { frame, .. } => frame.resolve(profile, false).map(|_| ()),
            Self::Callers { anchor, .. } | Self::Callees { anchor, .. } => {
                anchor.validate_alias(profile)
            }
            Self::Paths {
                window: PathWindow::AroundTarget { target, .. },
                ..
            } => target.validate_alias(profile),
            Self::Children {
                context: Some(_), ..
            }
            | Self::Frames {
                anchor: Some(_), ..
            }
            | Self::Aggregate {
                anchor: Some(_), ..
            } => Err(bound_reference_required()),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ViewKind {
    Frames,
    Contexts,
    Children,
    Callers,
    Callees,
    Paths,
    Aggregate,
}

fn bound_reference_required() -> ApiError {
    ApiError::new(
        "invalid_selection",
        "Context references require a snapshot-bound selection",
        json!({}),
        "Copy the selection from the result that supplied the context reference.",
    )
}

const fn default_limit() -> usize {
    DEFAULT_LIMIT
}

const fn yes() -> bool {
    true
}

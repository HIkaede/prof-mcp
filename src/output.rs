use schemars::JsonSchema;
use serde::Serialize;

use crate::profile::{FrameId, FrameStats, Profile};

mod budget;
mod types;
#[cfg(test)]
pub(crate) use budget::MAX_RESPONSE_BYTES;
pub(crate) use budget::{ResponseBudget, response_too_large};
pub use types::*;

pub const SCHEMA_VERSION: &str = "3";

pub fn percent(weight: u64, scope: u64) -> f64 {
    if scope == 0 {
        0.0
    } else {
        (weight as f64) * 100.0 / (scope as f64)
    }
}

/// Percent for display. The extra digits of the exact value are noise to a
/// reader, and are only meaningful for the `min_*_percent` filters.
pub(crate) fn display_percent(weight: u64, scope: u64) -> f64 {
    round_percent(percent(weight, scope))
}

/// A share of the whole profile repeats `profile_percent`, so it is only worth
/// printing when the query had a narrower scope.
pub(crate) fn scope_percent(weight: u64, scope: u64, total: u64) -> Option<f64> {
    (scope != total).then(|| display_percent(weight, scope))
}

pub(crate) fn round_percent(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

pub(crate) fn typed_envelope<'a, T>(
    profile: &'a Profile,
    scope_weight: u64,
    truncation_reasons: Vec<TruncationReason>,
    warnings: Vec<&'static str>,
    data: T,
) -> OutputEnvelope<'a, T> {
    OutputEnvelope {
        schema_version: SCHEMA_VERSION,
        profile: output_profile(profile),
        scope_weight,
        truncated: !truncation_reasons.is_empty(),
        truncation_reasons,
        warnings,
        data,
    }
}

pub(crate) fn output_profile(profile: &Profile) -> OutputProfile<'_> {
    OutputProfile {
        fingerprint: &profile.source.fingerprint,
        alias: "",
        weight_semantics: WeightSemantics {
            unit: "opaque",
            basis: "folded_input",
        },
    }
}

#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct FrameRow<'a> {
    pub frame_id: FrameId,
    pub name: &'a str,
    pub self_weight: u64,
    pub inclusive_weight: u64,
    pub stack_count: u32,
    pub profile_percent: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_percent: Option<f64>,
}

/// A ranked list of frames. Every row has the same fields, so the names are
/// emitted once in `columns` instead of once per row: the repeated keys were
/// most of the bytes in a long list.
#[derive(Clone, Debug, JsonSchema, Serialize)]
pub struct FrameTable {
    pub columns: Vec<&'static str>,
    pub rows: Vec<Vec<serde_json::Value>>,
}

#[derive(Clone, Copy)]
enum FrameColumn {
    FrameId,
    Name,
    SelfWeight,
    InclusiveWeight,
    StackCount,
    Percent,
    ScopePercent,
}

impl FrameColumn {
    const ALL: [Self; 7] = [
        Self::FrameId,
        Self::Name,
        Self::SelfWeight,
        Self::InclusiveWeight,
        Self::StackCount,
        Self::Percent,
        Self::ScopePercent,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::FrameId => "frame_id",
            Self::Name => "name",
            Self::SelfWeight => "self_weight",
            Self::InclusiveWeight => "inclusive_weight",
            Self::StackCount => "stack_count",
            Self::Percent => "profile_percent",
            Self::ScopePercent => "scope_percent",
        }
    }
}

impl FrameTable {
    pub(crate) fn new(rows: &[FrameRow<'_>]) -> Self {
        // A focused query carries a narrower scope; only then is the second
        // percentage worth a column.
        let scoped = rows.iter().any(|row| row.scope_percent.is_some());
        let columns = FrameColumn::ALL
            .into_iter()
            .filter(|column| scoped || !matches!(column, FrameColumn::ScopePercent))
            .collect::<Vec<_>>();
        let rows = rows
            .iter()
            .map(|row| {
                columns
                    .iter()
                    .map(|column| match column {
                        FrameColumn::FrameId => serde_json::json!(row.frame_id),
                        FrameColumn::Name => serde_json::json!(row.name),
                        FrameColumn::SelfWeight => serde_json::json!(row.self_weight),
                        FrameColumn::InclusiveWeight => serde_json::json!(row.inclusive_weight),
                        FrameColumn::StackCount => serde_json::json!(row.stack_count),
                        FrameColumn::Percent => serde_json::json!(row.profile_percent),
                        FrameColumn::ScopePercent => serde_json::json!(row.scope_percent),
                    })
                    .collect()
            })
            .collect();
        Self {
            columns: columns.iter().map(|column| column.name()).collect(),
            rows,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    /// The `limit` leading names, for the one-line text summary.
    pub(crate) fn names(&self, limit: usize) -> String {
        let name = self
            .columns
            .iter()
            .position(|column| *column == FrameColumn::Name.name());
        let Some(name) = name else {
            return String::new();
        };
        self.rows
            .iter()
            .take(limit)
            .map(|row| row[name].as_str().unwrap_or("-").to_owned())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub fn frame_row<'a>(
    profile: &'a Profile,
    id: FrameId,
    stats: &FrameStats,
    scope_weight: u64,
) -> FrameRow<'a> {
    frame_row_with_percent_weight(profile, id, stats, scope_weight, stats.inclusive_weight)
}

pub fn frame_row_with_percent_weight<'a>(
    profile: &'a Profile,
    id: FrameId,
    stats: &FrameStats,
    scope_weight: u64,
    percent_weight: u64,
) -> FrameRow<'a> {
    FrameRow {
        frame_id: id,
        name: profile.frame_name(id),
        self_weight: stats.self_weight,
        inclusive_weight: stats.inclusive_weight,
        stack_count: stats.stack_count,
        profile_percent: display_percent(percent_weight, profile.total_weight),
        scope_percent: scope_percent(percent_weight, scope_weight, profile.total_weight),
    }
}

/// One indented line per node. A call tree read as lines keeps its shape
/// without repeating a field name per node, and is roughly a third of the
/// size of the nested form.
pub(crate) fn text_tree(root: &crate::output::TreeNode) -> String {
    let mut out = String::new();
    match (root.frame_id, root.name) {
        // The virtual root only restates the envelope's scope weight.
        (None, None) => {
            for child in &root.children {
                walk_tree(child, 0, &mut out);
            }
        }
        _ => walk_tree(root, 0, &mut out),
    }
    out
}

fn walk_tree(node: &crate::output::TreeNode, depth: usize, out: &mut String) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str(node.name.unwrap_or("-"));
    if let Some(frame_id) = node.frame_id {
        out.push('#');
        out.push_str(&frame_id.to_string());
    }
    out.push_str(&format!(
        " total={} self={} {}%",
        node.total_weight, node.self_weight, node.profile_percent
    ));
    if node.omitted_children > 0 {
        out.push_str(&format!(
            " +{}({})",
            node.omitted_children, node.omitted_weight
        ));
    }
    out.push('\n');
    for child in &node.children {
        walk_tree(child, depth + 1, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_kind_matches_serde_tag() {
        let reasons = [
            TruncationReason::RowLimit {
                limit: 1,
                returned: 1,
                available: 2,
                omitted: 1,
            },
            TruncationReason::DepthLimit {
                max_depth: 1,
                omitted_children: 1,
                omitted_weight: 1,
            },
            TruncationReason::NodeBudget {
                max_nodes: 1,
                omitted_children: 1,
                omitted_weight: 1,
            },
            TruncationReason::MinScopePercent {
                threshold: 1.0,
                omitted_children: 1,
                omitted_weight: 1,
            },
            TruncationReason::FrameWindow {
                mode: FrameWindowMode::Head,
                cropped_paths: 1,
                total_omitted_before: 1,
                total_omitted_after: 1,
            },
            TruncationReason::TotalFrameBudget(TotalFrameBudget {
                limit: 1,
                available: 2,
                returned: 1,
                omitted: 1,
                selected_paths: 1,
                returned_paths: 1,
                omitted_paths: 0,
                cropped_paths: 0,
            }),
            TruncationReason::RegistryProfileLimit {
                limit: 1,
                returned: 1,
                available: 2,
                omitted: 1,
            },
        ];
        for reason in reasons {
            assert_eq!(
                serde_json::to_value(&reason).unwrap()["kind"],
                reason.kind()
            );
        }
    }

    #[test]
    fn rows_borrow_model_strings() {
        let profile = crate::profile::ProfileBuilder::new(crate::profile::BuildLimits::default())
            .from_reader(
                std::io::Cursor::new(b"root;leaf 1\n"),
                "/rows.folded".into(),
            )
            .unwrap();
        let id = profile.frame_id("leaf").unwrap();
        let row = frame_row(&profile, id, &profile.frame_stats[id as usize], 1);
        let metadata = output_profile(&profile);
        assert_eq!(row.name.as_ptr(), profile.frame_name(id).as_ptr());
        assert_eq!(
            metadata.fingerprint.as_ptr(),
            profile.source.fingerprint.as_ptr()
        );
        assert_eq!(serde_json::to_value(row).unwrap()["name"], "leaf");
    }
}

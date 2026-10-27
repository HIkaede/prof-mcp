//! Structured-result tagging, text fallbacks, and error rendering.

use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

use crate::error::ApiError;
use crate::registry;

pub(crate) fn serialize(output: impl serde::Serialize) -> Result<Value, ApiError> {
    serde_json::to_value(output)
        .map_err(|_| ApiError::internal("Could not serialize typed tool output"))
}

pub(crate) fn success(
    output: impl crate::output::FallbackText + serde::Serialize,
) -> CallToolResult {
    let fallback = bounded_text(&output.fallback());
    let value = match serialize(output) {
        Ok(value) => value,
        Err(error) => return failure(error),
    };
    let mut result = CallToolResult::default();
    result.content = vec![ContentBlock::text(fallback)];
    result.structured_content = Some(value);
    result.is_error = Some(false);
    bounded_result(result)
}
pub(crate) fn tag_alias<'a, T>(value: &mut crate::output::OutputEnvelope<'a, T>, alias: &'a str) {
    value.profile.alias = alias;
}

pub(crate) fn tag_tree<'a>(value: &mut crate::output::TreeOutput<'a>, alias: &'a str) {
    value.profile.alias = alias;
    for row in &mut value.data.continuations {
        row.continuation.profile = alias.into();
    }
}

pub(crate) fn tag_direction<'a>(value: &mut crate::output::DirectionOutput<'a>, alias: &'a str) {
    value.profile.alias = alias;
    for row in &mut value.data.continuations {
        row.continuation.profile = alias.into();
    }
}

pub(crate) fn tag_registry(
    value: &mut crate::output::SummaryOutput<'_>,
    status: registry::RegistryStatus,
) {
    use crate::output::{RegistryOutput, TruncationReason};
    const REGISTRY_PROFILE_LIMIT: usize = 100;
    let registry::RegistryStatus {
        registry_root,
        active,
        profiles,
    } = status;
    let available = profiles.len();
    let returned = available.min(REGISTRY_PROFILE_LIMIT);
    let mut selected_profiles = Vec::with_capacity(returned);
    let mut active_profile = None;
    for profile in profiles {
        if profile.alias == active {
            active_profile = Some(profile);
        } else if selected_profiles.len() < REGISTRY_PROFILE_LIMIT {
            selected_profiles.push(profile);
        }
    }
    if let Some(active_profile) = active_profile {
        selected_profiles.insert(0, active_profile);
        selected_profiles.truncate(REGISTRY_PROFILE_LIMIT);
    }
    value.data.registry = RegistryOutput {
        registry_root: registry_root.display().to_string(),
        active,
        profiles: selected_profiles,
        profile_count: available,
    };
    if available > REGISTRY_PROFILE_LIMIT {
        value.truncated = true;
        value
            .truncation_reasons
            .push(TruncationReason::RegistryProfileLimit {
                limit: REGISTRY_PROFILE_LIMIT,
                returned,
                available,
                omitted: available - returned,
            });
    }
}

pub(crate) fn failure(error: ApiError) -> CallToolResult {
    let text = serde_json::to_string(&error).unwrap_or_else(|_| {
        r#"{"code":"internal_error","message":"Could not serialize error","details":null,"retry_hint":"Retry."}"#.into()
    });
    let mut result = CallToolResult::default();
    result.content = vec![ContentBlock::text(text)];
    result.is_error = Some(true);
    bounded_result(result)
}
#[cfg(test)]
use crate::output::MAX_RESPONSE_BYTES;
use crate::output::{ResponseBudget, response_too_large};

pub(crate) fn bounded_result(result: CallToolResult) -> CallToolResult {
    if serde_json::to_writer(ResponseBudget::new(), &result).is_err() {
        return failure(response_too_large());
    }
    result
}

pub(crate) fn bounded_text(input: &str) -> String {
    const TEXT_LIMIT_BYTES: usize = 2048;
    const ELLIPSIS: &str = "...";
    let mut output = String::with_capacity(input.len().min(TEXT_LIMIT_BYTES));
    let mut truncated = false;
    for character in input.chars() {
        let character = if character.is_control() {
            '�'
        } else {
            character
        };
        if output
            .len()
            .saturating_add(character.len_utf8())
            .saturating_add(ELLIPSIS.len())
            > TEXT_LIMIT_BYTES
        {
            truncated = true;
            break;
        }
        output.push(character);
    }
    if truncated {
        output.push_str(ELLIPSIS);
    }
    output
}

#[cfg(test)]
mod text_tests {
    use super::*;
    use crate::output::FallbackText;
    use crate::query::{FrameSelector, TopSort};

    fn profile() -> crate::profile::Profile {
        crate::profile::ProfileBuilder::new(crate::profile::BuildLimits::default())
            .from_reader(
                std::io::Cursor::new(b"root;leaf 3\nroot;other 1\n"),
                "/text.folded".into(),
            )
            .unwrap()
    }

    #[test]
    fn count_encoded_response_bytes() {
        use std::io::Write;
        let mut budget = ResponseBudget(3);
        assert_eq!(budget.write(b"abc").unwrap(), 3);
        assert!(budget.write(b"d").is_err());
        let profile = profile();
        let mut summary = crate::query::summary(&profile).unwrap();
        summary.data.registry.registry_root = "x".repeat(MAX_RESPONSE_BYTES);
        let result = success(summary);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&result.content[0].as_text().unwrap().text)
                .unwrap()["code"],
            "query_too_large"
        );
        let error = failure(ApiError::new(
            "bad_input",
            "small",
            json!({"input":"x".repeat(MAX_RESPONSE_BYTES)}),
            "retry",
        ));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&error.content[0].as_text().unwrap().text)
                .unwrap()["code"],
            "query_too_large"
        );
        assert!(serde_json::to_vec(&error).unwrap().len() <= MAX_RESPONSE_BYTES);
    }

    #[test]
    fn summary_text_reports_totals_and_top_frames() {
        let profile = profile();
        let mut summary = crate::query::summary(&profile).unwrap();
        tag_alias(&mut summary, "sample");
        let text = summary.fallback();
        assert!(
            text.contains("profile=sample; truncated=false; reasons=[]; total_weight=4, frames=3, stacks=2, top_self=[leaf:3"),
            "{text}"
        );
    }

    #[test]
    fn tree_text_reports_root_and_continuations() {
        let profile = profile();
        let root = [profile.cct.root, 0];
        let cut = crate::query::tree(&profile, root, None, 8, 1, 0.0).unwrap();
        let crate::output::TreeBody::Nodes(node) = &cut.data.root else {
            panic!("default format is nested nodes");
        };
        assert_eq!(node.name, None, "virtual root has no frame");
        let child = &cut.data.continuations[0];
        let mut child_tree = crate::query::tree(
            &profile,
            child.node_id,
            Some(&profile.source.fingerprint),
            8,
            100,
            0.0,
        )
        .unwrap();
        tag_tree(&mut child_tree, "sample");
        let text = child_tree.fallback();
        assert!(
            text.contains(&format!(
                "tree_root={}, continuations={}",
                child.name,
                child_tree.data.continuations.len()
            )),
            "{text}"
        );
    }

    #[test]
    fn direction_text_reports_frame_and_scope() {
        let profile = profile();
        let selector = FrameSelector {
            frame_id: Some(profile.frame_id("leaf").unwrap()),
            frame_name: None,
        };
        let mut direction = crate::query::callers(&profile, &selector, 8, 100, 0.0, None).unwrap();
        tag_direction(&mut direction, "sample");
        let text = direction.fallback();
        assert!(
            text.contains("frame=leaf, scope_weight=3, continuations="),
            "{text}"
        );
        assert!(text.contains("truncated=false"), "{text}");
    }

    #[test]
    fn find_and_top_text_list_names() {
        let profile = profile();
        let mut find =
            crate::query::find_symbols(&profile, "lea", crate::query::MatchMode::Contains, 10)
                .unwrap();
        tag_alias(&mut find, "sample");
        assert!(
            find.fallback().contains("matches=1 [leaf]"),
            "{}",
            find.fallback()
        );
        let mut top = crate::query::top_with_workspace(
            &profile,
            TopSort::SelfWeight,
            10,
            None,
            None,
            &mut crate::query::TopWorkspace::default(),
        )
        .unwrap();
        tag_alias(&mut top, "sample");
        assert!(
            top.fallback().contains("rows=3 [leaf, other, root]"),
            "{}",
            top.fallback()
        );
    }

    #[test]
    fn paths_text_reports_first_window() {
        let profile = profile();
        let selector = FrameSelector {
            frame_id: Some(profile.frame_id("leaf").unwrap()),
            frame_name: None,
        };
        let mut paths =
            crate::query::paths_with_window_budget(&profile, &selector, 10, None, 100).unwrap();
        tag_alias(&mut paths, "sample");
        let text = paths.fallback();
        assert!(text.contains("paths=1 first_window="), "{text}");
        assert!(text.contains("target_display=[1]"), "{text}");
    }

    #[test]
    fn diff_text_reports_changes() {
        let profile = profile();
        let mut diff = crate::query::diff(
            &profile,
            &profile,
            TopSort::SelfWeight,
            crate::query::DiffSort::Absolute,
            10,
            None,
        )
        .unwrap();
        diff.baseline.alias = "base";
        diff.candidate.alias = "next";
        let text = diff.fallback();
        assert!(
            text.starts_with("profile=next; truncated=false; reasons=[];"),
            "{text}"
        );
        assert!(text.contains("changes=["), "{text}");
    }

    #[test]
    fn bound_text_safely() {
        let long = format!("{}\u{0000}\u{0007}", "多字节🙂".repeat(1024));
        let text = bounded_text(&long);
        assert!(text.len() <= 2048);
        assert!(!text.chars().any(char::is_control));
        assert!(std::str::from_utf8(text.as_bytes()).is_ok());
        assert!(text.ends_with("..."));

        let failure = failure(ApiError::new(
            "invalid_test",
            long.clone(),
            json!({}),
            "retry",
        ));
        let fallback = &failure.content[0]
            .as_text()
            .expect("failure must include text content")
            .text;
        assert!(serde_json::to_vec(&failure).unwrap().len() <= MAX_RESPONSE_BYTES);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(fallback).unwrap()["message"],
            long
        );
        assert!(failure.structured_content.is_none());
        assert!(!fallback.chars().any(char::is_control));
    }

    #[test]
    fn bound_registry_summary() {
        let profiles = (0..101)
            .map(|index| registry::RegistryProfile {
                alias: format!("p{index}"),
                fingerprint: "a".repeat(64),
                source_name: "sample.folded".into(),
                byte_len: 1,
            })
            .collect();
        let profile = crate::profile::ProfileBuilder::new(crate::profile::BuildLimits::default())
            .from_reader(
                std::io::Cursor::new(b"root;leaf 1\n"),
                "/summary.folded".into(),
            )
            .unwrap();
        let mut value = crate::query::summary(&profile).unwrap();
        tag_registry(
            &mut value,
            registry::RegistryStatus {
                registry_root: std::path::PathBuf::from(".prof-mcp"),
                active: "p100".into(),
                profiles,
            },
        );
        let value = serde_json::to_value(value).unwrap();
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

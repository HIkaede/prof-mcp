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

pub(crate) fn success(value: Value) -> CallToolResult {
    let fallback = text_fallback(&value);
    let mut result = CallToolResult::default();
    result.content = vec![ContentBlock::text(fallback)];
    result.structured_content = Some(value);
    result.is_error = Some(false);
    bounded_result(result)
}
pub(crate) fn tag_alias<'a, T>(
    mut value: crate::output::OutputEnvelope<'a, T>,
    alias: &'a str,
) -> crate::output::OutputEnvelope<'a, T> {
    value.profile.alias = alias;
    value
}

pub(crate) fn tag_tree<'a>(
    mut value: crate::output::TreeOutput<'a>,
    alias: &'a str,
) -> crate::output::TreeOutput<'a> {
    value.profile.alias = alias;
    for row in &mut value.data.continuations {
        row.continuation.profile = alias.into();
    }
    value
}

pub(crate) fn tag_direction<'a>(
    mut value: crate::output::DirectionOutput<'a>,
    alias: &'a str,
) -> crate::output::DirectionOutput<'a> {
    value.profile.alias = alias;
    for row in &mut value.data.continuations {
        row.continuation.profile = alias.into();
    }
    value
}

pub(crate) fn tag_registry<'a>(
    mut value: crate::output::SummaryOutput<'a>,
    status: registry::RegistryStatus,
) -> crate::output::SummaryOutput<'a> {
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
    value
}

pub(crate) fn text_fallback(value: &Value) -> String {
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
        format!(
            "frame={frame}, scope_weight={}, continuations={}",
            value["scope_weight"],
            data["continuations"].as_array().map_or(0, Vec::len)
        )
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
        "profile={alias}; truncated={truncated}; reasons=[{reasons}]; {detail}"
    ))
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

    #[test]
    fn count_encoded_response_bytes() {
        use std::io::Write;
        let mut budget = ResponseBudget(3);
        assert_eq!(budget.write(b"abc").unwrap(), 3);
        assert!(budget.write(b"d").is_err());
        let result = success(json!({"data":"\u{0000}".repeat(MAX_RESPONSE_BYTES / 2)}));
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
    fn direction_text_reports_continuations() {
        let value = serde_json::json!({
            "profile": {"alias": "sample"},
            "truncated": true,
            "truncation_reasons": [{"kind": "node_budget"}],
            "scope_weight": 5,
            "data": {
                "frame": {"name": "anchor"},
                "root": {"name": "anchor"},
                "continuations": [{}, {}]
            }
        });
        let text = text_fallback(&value);
        assert!(
            text.contains("frame=anchor, scope_weight=5, continuations=2"),
            "{text}"
        );
        assert!(text.contains("truncated=true"), "{text}");
        assert!(text.contains("reasons=[node_budget]"), "{text}");
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
        let value = tag_registry(
            crate::query::summary(&profile).unwrap(),
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

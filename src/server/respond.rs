//! Structured-result tagging, text fallbacks, and error rendering.

use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::{Value, json};

use crate::error::ApiError;
use crate::registry;

pub(crate) fn success(mut value: Value, text: &str, next_steps: &[&str]) -> CallToolResult {
    if !next_steps.is_empty()
        && let Some(object) = value.as_object_mut()
    {
        object.insert(
            "next_steps".into(),
            Value::Array(
                next_steps
                    .iter()
                    .map(|step| Value::String((*step).to_string()))
                    .collect(),
            ),
        );
    }
    let mut result = CallToolResult::structured(value);
    let fallback = text_fallback(
        result
            .structured_content
            .as_ref()
            .expect("structured result retains content"),
        text,
    );
    result.content = vec![ContentBlock::text(fallback)];
    result
}
pub(crate) fn tag_alias(mut value: Value, alias: &str, sample_period_us: Option<u64>) -> Value {
    if let Some(profile) = value.get_mut("profile").and_then(Value::as_object_mut) {
        profile.insert("alias".into(), Value::String(alias.into()));
        if let Some(period) = sample_period_us {
            profile["weight_semantics"]["sample_period_us"] = json!(period);
        }
    }
    value
}
pub(crate) fn tag_diff_aliases(
    mut value: Value,
    baseline: &str,
    baseline_period: Option<u64>,
    candidate: &str,
    candidate_period: Option<u64>,
) -> Value {
    if let Some(profile) = value.get_mut("baseline").and_then(Value::as_object_mut) {
        profile.insert("alias".into(), Value::String(baseline.into()));
        if let Some(period) = baseline_period {
            profile["weight_semantics"]["sample_period_us"] = json!(period);
        }
    }
    if let Some(profile) = value.get_mut("candidate").and_then(Value::as_object_mut) {
        profile.insert("alias".into(), Value::String(candidate.into()));
        if let Some(period) = candidate_period {
            profile["weight_semantics"]["sample_period_us"] = json!(period);
        }
    }
    value
}
pub(crate) fn tag_registry(mut value: Value, status: registry::RegistryStatus) -> Value {
    const REGISTRY_PROFILE_LIMIT: usize = 100;
    let available = status.profiles.len();
    let returned = available.min(REGISTRY_PROFILE_LIMIT);
    let mut profiles = Vec::with_capacity(returned);
    if let Some(active) = status
        .profiles
        .iter()
        .find(|profile| profile.alias == status.active)
    {
        profiles.push(active.clone());
    }
    profiles.extend(
        status
            .profiles
            .iter()
            .filter(|profile| profile.alias != status.active)
            .take(REGISTRY_PROFILE_LIMIT.saturating_sub(profiles.len()))
            .cloned(),
    );
    let mut registry_value = serde_json::to_value(status).unwrap_or_else(|_| json!({}));
    if let Some(registry) = registry_value.as_object_mut() {
        registry.insert("profile_count".into(), json!(available));
        registry.insert(
            "profiles".into(),
            serde_json::to_value(profiles).unwrap_or_else(|_| json!([])),
        );
    }
    if let Some(data) = value.get_mut("data").and_then(Value::as_object_mut) {
        data.insert("registry".into(), registry_value);
    }
    if available > REGISTRY_PROFILE_LIMIT
        && let Some(root) = value.as_object_mut()
    {
        root.insert("truncated".into(), Value::Bool(true));
        root.entry("truncation_reasons")
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .expect("query envelopes have a truncation_reasons array")
            .push(json!({
                "kind":"registry_profile_limit",
                "limit":REGISTRY_PROFILE_LIMIT,
                "returned":returned,
                "available":available,
                "omitted":available-returned,
            }));
    }
    value
}

pub(crate) fn text_fallback(value: &Value, next: &str) -> String {
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
        format!("frame={frame}, scope_weight={}", value["scope_weight"])
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
        "profile={alias}; truncated={truncated}; reasons=[{reasons}]; {detail}; {next}"
    ))
}
pub(crate) fn failure(error: ApiError) -> CallToolResult {
    let mut result=CallToolResult::structured_error(serde_json::to_value(&error).unwrap_or_else(|_| json!({"code":"internal_error","message":"Could not serialize error","details":null,"retry_hint":"Retry."})));
    result.content = vec![ContentBlock::text(bounded_text(&format!(
        "{}: {}",
        error.code, error.message
    )))];
    result
}
pub(crate) fn bounded_text(input: &str) -> String {
    const TEXT_LIMIT_BYTES: usize = 2048;
    let mut output = String::with_capacity(input.len().min(TEXT_LIMIT_BYTES));
    for character in input.chars() {
        let character = if character.is_control() {
            '�'
        } else {
            character
        };
        if output.len().saturating_add(character.len_utf8()) > TEXT_LIMIT_BYTES {
            break;
        }
        output.push(character);
    }
    output
}

#[cfg(test)]
mod text_tests {
    use super::*;

    #[test]
    fn text_fallback_is_utf8_boundary_safe_and_control_safe() {
        let long = format!("{}\u{0000}\u{0007}", "多字节🙂".repeat(1024));
        let text = bounded_text(&long);
        assert!(text.len() <= 2048);
        assert!(!text.chars().any(char::is_control));
        assert!(std::str::from_utf8(text.as_bytes()).is_ok());
        assert!(text.ends_with('🙂') || text.ends_with('字') || text.ends_with('节'));

        let failure = failure(ApiError::new("invalid_test", long, json!({}), "retry"));
        let fallback = &failure.content[0]
            .as_text()
            .expect("failure must include text content")
            .text;
        assert!(fallback.len() <= 2048);
        assert!(!fallback.chars().any(char::is_control));
    }

    #[test]
    fn summary_bounds_registry_aliases_with_an_explicit_reason() {
        let profiles = (0..101)
            .map(|index| registry::RegistryProfile {
                alias: format!("p{index}"),
                fingerprint: "a".repeat(64),
                source_name: "sample.folded".into(),
                byte_len: 1,
                registered_unix_ms: 0,
                sample_period_us: None,
            })
            .collect();
        let value = tag_registry(
            json!({"truncated":false,"truncation_reasons":[],"data":{}}),
            registry::RegistryStatus {
                registry_root: std::path::PathBuf::from(".prof-mcp"),
                active: "p100".into(),
                profiles,
            },
        );
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

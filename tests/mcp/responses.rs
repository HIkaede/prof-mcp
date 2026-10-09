use super::{check, sample};
use prof_mcp::registry;
use rmcp::model::CallToolRequestParams;
use serde::Deserialize;
use std::fs;
use tempfile::tempdir;

#[derive(Deserialize)]
struct TypedEnvelope<T> {
    schema_version: String,
    truncated: bool,
    truncation_reasons: Vec<serde_json::Value>,
    data: T,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct FrameRow {
    frame_id: u32,
    name: String,
}

#[derive(Deserialize)]
struct TopData {
    metric: String,
    frame: Option<u32>,
    rows: Vec<FrameRow>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct TreeNode {
    node_id: Option<u32>,
    children: Vec<TreeNode>,
}

#[derive(Deserialize)]
struct TreeData {
    root: TreeNode,
}

#[derive(Deserialize)]
struct DirectionData {
    frame: FrameRow,
    root: TreeNode,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct PathRow {
    frames: Vec<String>,
    target_positions: Vec<usize>,
    display_target_positions: Vec<usize>,
    frame_start: usize,
    frame_end: usize,
    weight: u64,
}

#[derive(Deserialize)]
struct PathsData {
    paths: Vec<PathRow>,
    total_frame_budget: TotalFrameBudget,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct TotalFrameBudget {
    limit: usize,
    returned: usize,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct DiffRow {
    name: String,
    baseline_weight: u64,
    candidate_weight: u64,
    delta_pp: f64,
}

#[derive(Deserialize)]
struct DiffData {
    rows: Vec<DiffRow>,
}

#[tokio::test]
async fn responses_have_no_next_steps() {
    sample(async |client| {
        for (name, arguments) in [
            ("profile_summary", serde_json::json!({})),
            ("profile_find_symbols", serde_json::json!({"query":"A"})),
            ("profile_top", serde_json::json!({})),
            ("profile_tree", serde_json::json!({"max_nodes":1})),
            (
                "profile_callers",
                serde_json::json!({"frame":{"frame_name":"A"},"max_nodes":1}),
            ),
            (
                "profile_callees",
                serde_json::json!({"frame":{"frame_name":"A"},"max_nodes":1}),
            ),
            (
                "profile_paths",
                serde_json::json!({"frame":{"frame_name":"A"}}),
            ),
            (
                "profile_diff",
                serde_json::json!({"baseline":"sample","candidate":"sample"}),
            ),
        ] {
            let response = client
                .call_tool(
                    CallToolRequestParams::new(name)
                        .with_arguments(arguments.as_object().unwrap().clone()),
                )
                .await
                .unwrap();
            assert_eq!(response.is_error, Some(false), "{name}");
            let value = response.structured_content.as_ref().unwrap();
            assert!(value.get("next_steps").is_none(), "{name}");
            assert_eq!(value["schema_version"], "2");
            let text = &response.content[0].as_text().unwrap().text;
            assert!(
                text.starts_with("profile=sample; truncated="),
                "{name}: {text}"
            );
            assert!(text.contains("; reasons=["), "{name}: {text}");
            assert!(!text.contains("profile_"), "{name}: {text}");
            if name == "profile_tree" {
                assert_eq!(value["truncated"], true);
                assert!(
                    !value["data"]["continuations"]
                        .as_array()
                        .unwrap()
                        .is_empty()
                );
            }
        }
    })
    .await;
}

#[tokio::test]
async fn summary_metadata_and_text() {
    sample(async |client| {
        let result = client
            .call_tool(
                CallToolRequestParams::new("profile_summary")
                    .with_arguments(serde_json::json!({}).as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(false));
        assert!(result.structured_content.is_some());
        assert_eq!(
            result.structured_content.as_ref().unwrap()["schema_version"],
            "2"
        );
        assert!(
            result.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("top_self=[")
        );
        assert!(
            result.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("truncated=false")
        );
        let summary = result.structured_content.as_ref().unwrap();
        assert_eq!(summary["data"]["registry"]["active"], "sample");
        assert_eq!(
            summary["data"]["registry"]["profiles"][0]["alias"],
            "sample"
        );
    })
    .await;
}

#[tokio::test]
async fn deserialize_tool_responses() {
    sample(async |client| {
        for (name, arguments) in [
            (
                "profile_find_symbols",
                serde_json::json!({"profile":"sample","query":"A"}),
            ),
            ("profile_top", serde_json::json!({"profile":"sample"})),
            ("profile_tree", serde_json::json!({"profile":"sample"})),
            (
                "profile_callers",
                serde_json::json!({"profile":"sample","frame":{"frame_name":"A"}}),
            ),
            (
                "profile_callees",
                serde_json::json!({"profile":"sample","frame":{"frame_name":"A"}}),
            ),
            (
                "profile_diff",
                serde_json::json!({"baseline":"sample","candidate":"sample"}),
            ),
        ] {
            let response = client
                .call_tool(
                    CallToolRequestParams::new(name)
                        .with_arguments(arguments.as_object().unwrap().clone()),
                )
                .await
                .unwrap();
            assert_eq!(response.is_error, Some(false), "{name}");
            let structured = response
                .structured_content
                .expect("{name} structured content");
            match name {
                "profile_top" => {
                    assert!(structured["data"].get("grouped_rows").is_none());
                    let typed: TypedEnvelope<TopData> = serde_json::from_value(structured).unwrap();
                    assert_eq!(typed.schema_version, "2");
                    assert_eq!(typed.data.metric, "self");
                    assert_eq!(typed.data.frame, None);
                    assert!(!typed.data.rows.is_empty());
                    assert!(!typed.data.rows[0].name.is_empty());
                }
                "profile_tree" => {
                    let typed: TypedEnvelope<TreeData> =
                        serde_json::from_value(structured).unwrap();
                    assert_eq!(typed.schema_version, "2");
                    assert_eq!(typed.data.root.node_id, Some(0));
                }
                "profile_callers" | "profile_callees" => {
                    let typed: TypedEnvelope<DirectionData> =
                        serde_json::from_value(structured).unwrap();
                    assert_eq!(typed.data.frame.name, "A");
                    assert!(typed.data.root.node_id.is_none());
                }
                "profile_diff" => {
                    let typed: TypedEnvelope<DiffData> =
                        serde_json::from_value(structured).unwrap();
                    assert!(!typed.data.rows.is_empty());
                    assert_eq!(typed.data.rows[0].delta_pp, 0.0);
                }
                "profile_find_symbols" => {
                    let matches = structured["data"]["matches"].as_array().unwrap();
                    assert_eq!(matches.len(), 1);
                    assert_eq!(matches[0]["name"], "A");
                    assert_eq!(matches[0]["self_weight"], 3);
                    assert_eq!(matches[0]["inclusive_weight"], 5);
                    assert_eq!(matches[0]["stack_count"], 2);
                    assert!(matches[0].get("context_hint").is_none());
                }
                _ => unreachable!("unexpected tool in typed output loop"),
            }
        }
    })
    .await;
}

#[tokio::test]
async fn truncated_path_positions() {
    sample(async |client| {
        let truncated = client
            .call_tool(
                CallToolRequestParams::new("profile_paths").with_arguments(
                    serde_json::json!({"profile":"sample", "frame":{"frame_name":"A"}, "limit":1})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(truncated.is_error, Some(false));
        let truncated_structured = truncated.structured_content.unwrap();
        assert!(truncated_structured["truncated"].as_bool().unwrap());
        let typed_paths: TypedEnvelope<PathsData> =
            serde_json::from_value(truncated_structured).unwrap();
        assert!(typed_paths.truncated);
        assert_eq!(typed_paths.truncation_reasons[0]["kind"], "row_limit");
        assert_eq!(typed_paths.data.total_frame_budget.limit, 500);
        assert!(!typed_paths.data.paths.is_empty());
        assert!(
            truncated.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("truncated=true")
        );
        let windowed = client
            .call_tool(
                CallToolRequestParams::new("profile_paths").with_arguments(
                    serde_json::json!({
                        "profile":"sample",
                        "frame":{"frame_name":"A"},
                        "limit":2,
                        "frame_window":{"mode":"around_target","before":0,"after":1}
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(windowed.is_error, Some(false));
        let windowed = windowed.structured_content.unwrap();
        assert_eq!(windowed["schema_version"], "2");
        assert!(windowed["truncated"].as_bool().unwrap());
        assert_eq!(
            windowed["data"]["paths"][0]["target_positions"],
            serde_json::json!([1])
        );
        assert_eq!(
            windowed["data"]["paths"][0]["display_target_positions"],
            serde_json::json!([0])
        );
        assert_eq!(windowed["truncation_reasons"][0]["kind"], "frame_window");
        assert_eq!(windowed["data"]["paths"][0]["frame_start"], 1);
        assert_eq!(windowed["data"]["paths"][0]["frame_end"], 2);
        assert_eq!(windowed["data"]["paths"][0]["omitted_before"], 1);
        assert_eq!(windowed["data"]["paths"][0]["omitted_after"], 0);
        let typed: TypedEnvelope<PathsData> = serde_json::from_value(windowed).unwrap();
        let path = &typed.data.paths[0];
        assert_eq!(path.frames, ["A"]);
        assert_eq!(path.target_positions, [1]);
        assert_eq!(path.display_target_positions, [0]);
        assert_eq!((path.frame_start, path.frame_end, path.weight), (1, 2, 3));
    })
    .await;
}

#[tokio::test]
async fn business_and_protocol_errors() {
    sample(async |client| {
        for name in ["profile_top", "profile_find_symbols"] {
            let arguments = if name == "profile_top" {
                serde_json::json!({"normalize":true})
            } else {
                serde_json::json!({"query":"A","normalize":true})
            };
            assert!(
                client
                    .call_tool(
                        CallToolRequestParams::new(name)
                            .with_arguments(arguments.as_object().unwrap().clone()),
                    )
                    .await
                    .is_err(),
                "{name}"
            );
        }
        let error = client
            .call_tool(
                CallToolRequestParams::new("profile_summary").with_arguments(
                    serde_json::json!({"profile":"missing"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(error.is_error, Some(true));
        assert_eq!(
            error.structured_content.unwrap()["code"],
            "profile_alias_not_found"
        );
        assert!(
            client
                .call_tool(
                    CallToolRequestParams::new("profile_top").with_arguments(
                        serde_json::json!({"profile":"sample", "limti":1})
                            .as_object()
                            .unwrap()
                            .clone(),
                    ),
                )
                .await
                .is_err()
        );
        assert!(
            client
                .call_tool(
                    CallToolRequestParams::new("profile_top").with_arguments(
                        serde_json::json!({"profile":"sample", "limit":"bad"})
                            .as_object()
                            .unwrap()
                            .clone(),
                    ),
                )
                .await
                .is_err()
        );
        let invalid_budget = client
            .call_tool(
                CallToolRequestParams::new("profile_top").with_arguments(
                    serde_json::json!({"profile":"sample", "limit":201})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(invalid_budget.is_error, Some(true));
        assert_eq!(
            invalid_budget.structured_content.unwrap()["code"],
            "invalid_budget"
        );
        let control_error = client
            .call_tool(
                CallToolRequestParams::new("profile_paths").with_arguments(
                    serde_json::json!({"profile":"sample", "frame":{"frame_name":"evil\nname"}})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert!(
            !control_error.content[0]
                .as_text()
                .unwrap()
                .text
                .contains('\n')
        );
    })
    .await;
}

#[tokio::test]
async fn diff_raw_weights() {
    let workspace = tempdir().unwrap();
    let source = workspace.path().join("diff.folded");
    for (alias, content) in [
        ("baseline", "root;hot 100\n"),
        ("candidate", "root;hot 250\n"),
    ] {
        fs::write(&source, content).unwrap();
        registry::register(workspace.path(), &source, Some(alias), 1024).unwrap();
    }
    check(workspace.path(), true, async |client| {
        let response = client
            .call_tool(
                CallToolRequestParams::new("profile_diff").with_arguments(
                    serde_json::json!({"baseline":"baseline", "candidate":"candidate"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(response.is_error, Some(false));
        let typed: TypedEnvelope<DiffData> =
            serde_json::from_value(response.structured_content.unwrap()).unwrap();
        assert_eq!(typed.schema_version, "2");
        assert!(!typed.truncated);
        assert!(typed.truncation_reasons.is_empty());
        let hot = typed
            .data
            .rows
            .iter()
            .find(|row| row.name == "hot")
            .unwrap();
        assert_eq!((hot.baseline_weight, hot.candidate_weight), (100, 250));
        assert_eq!(hot.delta_pp, 0.0);
    })
    .await;
}

#[tokio::test]
async fn profile_identity() {
    sample(async |client| {
        for (name, args) in [
            ("profile_summary", serde_json::json!({})),
            ("profile_find_symbols", serde_json::json!({"query":"A"})),
            ("profile_top", serde_json::json!({})),
            ("profile_tree", serde_json::json!({})),
            (
                "profile_callers",
                serde_json::json!({"frame":{"frame_name":"A"}}),
            ),
            (
                "profile_callees",
                serde_json::json!({"frame":{"frame_name":"A"}}),
            ),
            (
                "profile_paths",
                serde_json::json!({"frame":{"frame_name":"A"}}),
            ),
            (
                "profile_diff",
                serde_json::json!({"baseline":"sample","candidate":"sample"}),
            ),
        ] {
            let response = client
                .call_tool(
                    CallToolRequestParams::new(name)
                        .with_arguments(args.as_object().unwrap().clone()),
                )
                .await
                .unwrap();
            assert_eq!(response.is_error, Some(false));
            let value = response.structured_content.unwrap();
            let sides: &[&str] = if name == "profile_diff" {
                &["baseline", "candidate"]
            } else {
                &["profile"]
            };
            for side in sides {
                let identity = &value[side];
                assert_eq!(identity.as_object().unwrap().len(), 3, "{name}");
                assert_eq!(identity["alias"], "sample");
                assert_eq!(identity["fingerprint"].as_str().unwrap().len(), 64);
                assert_eq!(
                    identity["weight_semantics"],
                    serde_json::json!({"unit":"opaque","basis":"folded_input"})
                );
            }
        }
    })
    .await;
}

async fn invoke(
    client: &rmcp::Peer<rmcp::RoleClient>,
    tool: &'static str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let response = client
        .call_tool(
            CallToolRequestParams::new(tool).with_arguments(arguments.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(response.is_error, Some(false), "{tool}: {response:?}");
    response.structured_content.unwrap()
}

#[tokio::test]
async fn roundtrip() {
    let workspace = tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    for (alias, content) in [
        ("base", "root;outer;A;B;leaf 3\n"),
        ("other", "other;X 1\n"),
    ] {
        fs::write(&source, content).unwrap();
        registry::register(workspace.path(), &source, Some(alias), 1024).unwrap();
    }
    check(workspace.path(), true, async |client| {
        for tool in ["profile_tree", "profile_callers", "profile_callees"] {
            let mut arguments = serde_json::json!({"profile":"base","max_depth":0});
            if tool != "profile_tree" {
                arguments["frame"] = serde_json::json!({"frame_name":"A"});
            }
            let first = invoke(&client, tool, arguments).await;
            let row = &first["data"]["continuations"][0];
            assert!(row.get("profile_fingerprint").is_none());
            assert!(row.get("node_path").is_none());
            let continuation = &row["continuation"];
            assert_eq!(continuation.as_object().unwrap().len(), 4);
            assert_eq!(continuation["profile"], "base");
            assert_eq!(continuation["tool"], tool);
            assert_eq!(continuation["fingerprint"], first["profile"]["fingerprint"]);
            assert_eq!(
                continuation["cursor"].as_array().unwrap().len(),
                if tool == "profile_tree" { 1 } else { 2 }
            );
            registry::set_active(workspace.path(), "other").unwrap();
            let resumed = invoke(
                &client,
                tool,
                serde_json::json!({"continuation":continuation}),
            )
            .await;
            assert_eq!(resumed["profile"]["alias"], "base");
            assert_eq!(resumed["data"]["root"]["name"], row["name"]);
            assert_eq!(resumed["data"]["root"]["total_weight"], row["total_weight"]);
            if tool != "profile_tree" {
                assert_eq!(
                    resumed["data"]["frame"]["frame_id"],
                    continuation["cursor"][0]
                );
            }
            let mut token = continuation.clone();
            for round in 0..2 {
                let current = invoke(
                    &client,
                    tool,
                    serde_json::json!({"continuation":token,"max_depth":0}),
                )
                .await;
                let cursor = token["cursor"].as_array().unwrap();
                assert_eq!(current["profile"]["alias"], "base");
                assert!(
                    current["data"]["root"]["children"]
                        .as_array()
                        .unwrap()
                        .is_empty()
                );
                if tool == "profile_tree" {
                    assert_eq!(current["data"]["root"]["node_id"], cursor[0]);
                } else {
                    assert_eq!(current["data"]["frame"]["frame_id"], cursor[0]);
                    assert_eq!(current["data"]["root"]["frame_id"], *cursor.last().unwrap());
                }
                let rows = current["data"]["continuations"].as_array().unwrap();
                if round == 1 && tool != "profile_tree" {
                    assert!(rows.is_empty());
                    assert_eq!(cursor.len(), 3);
                    continue;
                }
                let next = &rows[0]["continuation"];
                assert_eq!(rows[0]["reason"], "depth_limit");
                if tool == "profile_tree" {
                    assert_eq!(next["cursor"].as_array().unwrap().len(), 1);
                    assert_ne!(next["cursor"][0], cursor[0]);
                } else {
                    let next_cursor = next["cursor"].as_array().unwrap();
                    assert_eq!(&next_cursor[..cursor.len()], cursor.as_slice());
                    assert_eq!(next_cursor.len(), cursor.len() + 1);
                    assert_ne!(next_cursor.last().unwrap(), &cursor[0]);
                }
                token = next.clone();
            }
        }
    })
    .await;
}

#[tokio::test]
async fn changed() {
    let workspace = tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    fs::write(&source, "root;A;B 3\n").unwrap();
    registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
    check(workspace.path(), true, async |client| {
        let mut tokens = Vec::new();
        for tool in ["profile_tree", "profile_callers", "profile_callees"] {
            let mut arguments = serde_json::json!({"profile":"base","max_nodes":1});
            if tool != "profile_tree" {
                arguments["frame"] = serde_json::json!({"frame_name":"A"});
            }
            let first = invoke(&client, tool, arguments).await;
            tokens.push((
                tool,
                first["data"]["continuations"][0]["continuation"].clone(),
            ));
        }
        fs::write(&source, "root;A;C 4\n").unwrap();
        registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
        for (tool, token) in tokens {
            let response = client
                .call_tool(
                    CallToolRequestParams::new(tool).with_arguments(
                        serde_json::json!({"continuation":token})
                            .as_object()
                            .unwrap()
                            .clone(),
                    ),
                )
                .await
                .unwrap();
            assert_eq!(response.is_error, Some(true));
            assert_eq!(
                response.structured_content.unwrap()["code"],
                "profile_changed"
            );
        }
    })
    .await;
}

#[tokio::test]
async fn invalid() {
    sample(async |client| {
        for tool in ["profile_tree", "profile_callers", "profile_callees"] {
            let mut arguments = serde_json::json!({"max_nodes":1});
            if tool != "profile_tree" {
                arguments["frame"] = serde_json::json!({"frame_name":"A"});
                let missing = client
                    .call_tool(CallToolRequestParams::new(tool))
                    .await
                    .unwrap();
                assert_eq!(missing.is_error, Some(true));
                assert_eq!(
                    missing.structured_content.unwrap()["code"],
                    "invalid_frame_selector"
                );
            }
            let first = invoke(&client, tool, arguments).await;
            let token = first["data"]["continuations"][0]["continuation"].clone();
            let mut wrong_tool = token.clone();
            wrong_tool["tool"] = serde_json::json!(if tool == "profile_tree" {
                "profile_callers"
            } else {
                "profile_tree"
            });
            let mut empty_cursor = token.clone();
            empty_cursor["cursor"] = serde_json::json!([]);
            let mut wrong_shape = token.clone();
            wrong_shape["cursor"] = if tool == "profile_tree" {
                serde_json::json!([0, 1])
            } else {
                serde_json::json!([0])
            };
            for arguments in [
                serde_json::json!({"continuation":wrong_tool}),
                serde_json::json!({"continuation":empty_cursor}),
                serde_json::json!({"continuation":wrong_shape}),
                serde_json::json!({"continuation":token,"profile":"sample"}),
            ] {
                let response = client
                    .call_tool(
                        CallToolRequestParams::new(tool)
                            .with_arguments(arguments.as_object().unwrap().clone()),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.is_error, Some(true), "{tool}: {response:?}");
                assert_eq!(
                    response.structured_content.unwrap()["code"],
                    "invalid_continuation"
                );
            }
            let mut malformed = token.clone();
            malformed["unexpected"] = serde_json::json!(true);
            assert!(
                client
                    .call_tool(
                        CallToolRequestParams::new(tool).with_arguments(
                            serde_json::json!({"continuation":malformed})
                                .as_object()
                                .unwrap()
                                .clone(),
                        )
                    )
                    .await
                    .is_err()
            );
            if tool != "profile_tree" {
                let response = client
                    .call_tool(
                        CallToolRequestParams::new(tool).with_arguments(
                            serde_json::json!({"continuation":token,"frame":{"frame_name":"A"}})
                                .as_object()
                                .unwrap()
                                .clone(),
                        ),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.is_error, Some(true));
                assert_eq!(
                    response.structured_content.unwrap()["code"],
                    "invalid_continuation"
                );
            }
        }
        for (tool, arguments) in [
            ("profile_top", serde_json::json!({"sort":"self"})),
            (
                "profile_top",
                serde_json::json!({"focus":{"frame_name":"A"}}),
            ),
            (
                "profile_paths",
                serde_json::json!({"through":{"frame_name":"A"}}),
            ),
            ("profile_tree", serde_json::json!({"root_node_id":0})),
            (
                "profile_tree",
                serde_json::json!({"profile_fingerprint":"old"}),
            ),
        ] {
            assert!(
                client
                    .call_tool(
                        CallToolRequestParams::new(tool)
                            .with_arguments(arguments.as_object().unwrap().clone())
                    )
                    .await
                    .is_err(),
                "{tool}"
            );
        }
    })
    .await;
}

#[tokio::test]
async fn selectors() {
    sample(async |client| {
        let top = invoke(
            &client,
            "profile_top",
            serde_json::json!({"metric":"inclusive","frame":{"frame_name":"A"}}),
        )
        .await;
        assert_eq!(top["data"]["metric"], "inclusive");
        assert!(top["data"]["frame"].is_number());
        assert!(top["data"].get("sort").is_none());
        assert!(top["data"].get("focus").is_none());
        let paths = invoke(
            &client,
            "profile_paths",
            serde_json::json!({"frame":{"frame_id":top["data"]["frame"]}}),
        )
        .await;
        assert_eq!(paths["data"]["frame"], top["data"]["frame"]);
        assert!(paths["data"].get("through").is_none());
    })
    .await;
}

#[tokio::test]
async fn oversized_responses_are_structured_errors() {
    let root = tempdir().unwrap();
    let source = root.path().join("long.folded");
    fs::write(&source, format!("root;{} 1\n", "多".repeat(24_000))).unwrap();
    registry::register(root.path(), &source, Some("long"), 1024 * 1024).unwrap();
    check(root.path(), true, async |client| {
        for (tool, arguments) in [
            ("profile_summary", serde_json::json!({})),
            ("profile_find_symbols", serde_json::json!({"query":""})),
            ("profile_top", serde_json::json!({})),
            ("profile_tree", serde_json::json!({})),
            (
                "profile_callers",
                serde_json::json!({"frame":{"frame_id":1}}),
            ),
            (
                "profile_callees",
                serde_json::json!({"frame":{"frame_id":1}}),
            ),
            ("profile_paths", serde_json::json!({"frame":{"frame_id":0}})),
            (
                "profile_diff",
                serde_json::json!({"baseline":"long","candidate":"long"}),
            ),
        ] {
            let request = CallToolRequestParams::new(tool)
                .with_arguments(arguments.as_object().unwrap().clone());
            let result = client.call_tool(request.clone()).await.unwrap();
            assert_eq!(result.is_error, Some(true), "{tool}");
            assert!(serde_json::to_vec(&result).unwrap().len() <= 65536);
            let value = result.structured_content.unwrap();
            assert_eq!(value["code"], "query_too_large");
            assert_eq!(value["details"]["resource"], "response_bytes");
            assert_eq!(value["details"]["limit_bytes"], 65536);
            assert_eq!(
                client
                    .call_tool(request)
                    .await
                    .unwrap()
                    .structured_content
                    .unwrap(),
                value
            );
        }
        let small = client
            .call_tool(
                CallToolRequestParams::new("profile_find_symbols").with_arguments(
                    serde_json::json!({"query":"root"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(small.is_error, Some(false));
        assert_eq!(
            small.structured_content.unwrap()["data"]["matches"][0]["name"],
            "root"
        );
    })
    .await;
}

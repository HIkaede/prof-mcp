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
    total_weight_ratio: f64,
}

#[tokio::test]
async fn summary_has_registry_metadata_and_text_fallback() {
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
async fn successful_responses_deserialize_into_independent_client_types() {
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
                    let typed: TypedEnvelope<TopData> = serde_json::from_value(structured).unwrap();
                    assert_eq!(typed.schema_version, "2");
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
                    assert_eq!(typed.data.total_weight_ratio, 1.0);
                }
                "profile_find_symbols" => {}
                _ => unreachable!("unexpected tool in typed output loop"),
            }
        }
    })
    .await;
}

#[tokio::test]
async fn path_truncation_preserves_positions_and_reports_reasons() {
    sample(async |client| {
        let truncated = client
        .call_tool(
            CallToolRequestParams::new("profile_paths").with_arguments(
                serde_json::json!({"profile":"sample", "through":{"frame_name":"A"}, "limit":1})
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
                        "through":{"frame_name":"A"},
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
async fn business_and_protocol_errors_remain_distinct() {
    sample(async |client| {
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
                    serde_json::json!({"profile":"sample", "through":{"frame_name":"evil\nname"}})
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
async fn diff_conversion_keeps_ratio_and_raw_weights_for_different_totals() {
    let workspace = tempdir().unwrap();
    let source = workspace.path().join("diff.folded");
    for (alias, content) in [
        ("baseline", "root;hot 100\n"),
        ("candidate", "root;hot 250\n"),
    ] {
        fs::write(&source, content).unwrap();
        registry::register(workspace.path(), &source, Some(alias), 1024, None).unwrap();
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
        assert_eq!(typed.data.total_weight_ratio, 2.5);
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
async fn weight_semantics_are_opaque_and_period_metadata_survives_every_tool() {
    let workspace = tempdir().unwrap();
    let source = workspace.path().join("metadata.folded");
    for (alias, period) in [
        ("plain", None),
        ("declared", Some(100)),
        ("other", Some(200)),
    ] {
        fs::write(&source, "root;A 3\nroot;A;B 2\n").unwrap();
        registry::register(workspace.path(), &source, Some(alias), 1024, period).unwrap();
    }
    check(workspace.path(), true, async |client| {
        for (alias, period) in [("plain", None), ("declared", Some(100))] {
            for (name, mut args) in [
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
                    serde_json::json!({"through":{"frame_name":"A"}}),
                ),
            ] {
                args["profile"] = serde_json::json!(alias);
                let response = client
                    .call_tool(
                        CallToolRequestParams::new(name)
                            .with_arguments(args.as_object().unwrap().clone()),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.is_error, Some(false), "{name}");
                let value = response.structured_content.unwrap();
                let semantics = &value["profile"]["weight_semantics"];
                assert_eq!(semantics["unit"], "opaque", "{name}");
                assert_eq!(semantics["basis"], "folded_input", "{name}");
                assert_eq!(semantics["sample_period_us"].as_u64(), period, "{name}");
                assert_eq!(value["scope_weight"], 5, "metadata must not rescale {name}");
            }
        }
        let response = client
            .call_tool(
                CallToolRequestParams::new("profile_diff").with_arguments(
                    serde_json::json!({"baseline":"declared", "candidate":"other"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(response.is_error, Some(false));
        let value = response.structured_content.unwrap();
        for (side, period) in [("baseline", 100), ("candidate", 200)] {
            assert_eq!(value[side]["weight_semantics"]["unit"], "opaque");
            assert_eq!(value[side]["weight_semantics"]["sample_period_us"], period);
            assert_eq!(value["scope_weight"][side], 5);
        }
    })
    .await;
}

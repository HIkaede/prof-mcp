use std::fs;

use prof_mcp::{config::Config, registry, server::ProfileServer};
use rmcp::{
    ClientHandler, ServiceExt,
    model::{CallToolRequestParams, ClientInfo},
};
use tempfile::tempdir;

#[derive(Clone, Debug, Default)]
struct TestClient;
impl ClientHandler for TestClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
}

#[tokio::test]
async fn mcp_lists_exact_tools_with_object_schemas_and_returns_structured_results() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("sample.folded"), "root;A 3\nroot;A;B 2\n").unwrap();
    registry::register(
        root.path(),
        &root.path().join("sample.folded"),
        Some("sample"),
        1024 * 1024,
    )
    .unwrap();
    let server = ProfileServer::new_in_workspace(
        Config {
            profile: None,
            name: None,
            max_file_size_mib: 1,
            cache_capacity: 2,
            log_level: "warn".into(),
        },
        root.path().to_owned(),
    )
    .unwrap();
    let (server_transport, client_transport) = tokio::io::duplex(65536);
    let server_task = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });
    let client = TestClient.serve(client_transport).await.unwrap();
    let tools = client.list_tools(None).await.unwrap();
    let names: Vec<_> = tools.tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(
        names,
        vec![
            "profile_summary",
            "profile_find_symbols",
            "profile_top",
            "profile_tree",
            "profile_callers",
            "profile_callees",
            "profile_paths",
            "profile_diff"
        ]
    );
    assert!(tools.tools.iter().all(|tool| {
        tool.output_schema
            .as_ref()
            .and_then(|schema| schema.get("type"))
            .and_then(|value| value.as_str())
            == Some("object")
    }));
    let summary_schema = tools.tools[0].output_schema.as_ref().unwrap();
    assert!(
        summary_schema["anyOf"][0]["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("profile"))
    );
    assert!(summary_schema["properties"]["data"].is_object());
    assert_eq!(
        summary_schema["properties"]["schema_version"]["type"],
        "string"
    );
    assert_eq!(summary_schema["properties"]["schema_version"]["const"], "2");
    assert!(
        summary_schema["anyOf"][0]["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("truncation_reasons"))
    );
    assert!(
        summary_schema["anyOf"][1]["required"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("retry_hint"))
    );
    let find_schema = &tools.tools[1].input_schema;
    assert_eq!(find_schema["properties"]["limit"]["minimum"], 1);
    assert_eq!(find_schema["properties"]["limit"]["maximum"], 100);
    assert!(
        find_schema["$defs"]["FindModeSchema"]["enum"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("regex"))
    );
    for tool_index in [2, 4, 5, 6] {
        let selector =
            &tools.tools[tool_index].input_schema["$defs"]["FrameSelectorInput"]["oneOf"];
        assert!(
            selector.is_array(),
            "{}",
            serde_json::to_string_pretty(&tools.tools[tool_index].input_schema).unwrap()
        );
        assert_eq!(selector.as_array().unwrap().len(), 2);
        assert!(
            selector[0]["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("frame_id"))
        );
        assert!(
            selector[1]["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("frame_name"))
        );
        assert_eq!(selector[0]["additionalProperties"], false);
        assert_eq!(selector[1]["additionalProperties"], false);
    }
    let paths_schema = &tools.tools[6].input_schema["$defs"]["FrameWindowInput"]["oneOf"];
    assert_eq!(paths_schema[0]["properties"]["lines"]["minimum"], 1);
    assert_eq!(paths_schema[0]["properties"]["lines"]["maximum"], 4096);
    assert_eq!(paths_schema[1]["properties"]["lines"]["minimum"], 1);
    assert_eq!(paths_schema[1]["properties"]["lines"]["maximum"], 4096);
    assert_eq!(paths_schema[2]["properties"]["before"]["minimum"], 0);
    assert_eq!(paths_schema[2]["properties"]["before"]["maximum"], 4096);
    assert_eq!(paths_schema[2]["properties"]["after"]["minimum"], 0);
    assert_eq!(paths_schema[2]["properties"]["after"]["maximum"], 4096);
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
        assert!(response.structured_content.is_some(), "{name}");
    }
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
    assert!(
        truncated.structured_content.unwrap()["truncated"]
            .as_bool()
            .unwrap()
    );
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
    client.cancel().await.unwrap();
    server_task.await.unwrap().unwrap();
}

#[tokio::test]
async fn mcp_stays_available_without_registry_and_observes_registration_after_start() {
    let workspace = tempdir().unwrap();
    let server = ProfileServer::new_in_workspace(
        Config {
            profile: None,
            name: None,
            max_file_size_mib: 1,
            cache_capacity: 2,
            log_level: "warn".into(),
        },
        workspace.path().to_owned(),
    )
    .unwrap();
    let (server_transport, client_transport) = tokio::io::duplex(65536);
    let server_task = tokio::spawn(async move {
        server.serve(server_transport).await?.waiting().await?;
        anyhow::Ok(())
    });
    let client = TestClient.serve(client_transport).await.unwrap();
    assert_eq!(client.list_tools(None).await.unwrap().tools.len(), 8);
    let unavailable = client
        .call_tool(CallToolRequestParams::new("profile_summary"))
        .await
        .unwrap();
    assert_eq!(unavailable.is_error, Some(true));
    assert_eq!(
        unavailable.structured_content.unwrap()["code"],
        "workspace_not_registered"
    );

    let source = workspace.path().join("started.folded");
    fs::write(&source, "root;visible 1\n").unwrap();
    registry::register(workspace.path(), &source, Some("visible"), 1024 * 1024).unwrap();
    let available = client
        .call_tool(CallToolRequestParams::new("profile_summary"))
        .await
        .unwrap();
    assert_eq!(available.is_error, Some(false));
    assert_eq!(
        available.structured_content.unwrap()["profile"]["alias"],
        "visible"
    );
    client.cancel().await.unwrap();
    server_task.await.unwrap().unwrap();
}

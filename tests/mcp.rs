use std::fs;

use rmcp::{
    ClientHandler, ServiceExt,
    model::{CallToolRequestParams, ClientInfo},
};
use rmdb_prof_mcp::{config::Config, server::ProfileServer};
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
    let server = ProfileServer::new(Config {
        root: vec![root.path().to_owned()],
        max_file_size_mib: 1,
        cache_capacity: 2,
        log_level: "warn".into(),
    })
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
    let result = client
        .call_tool(
            CallToolRequestParams::new("profile_summary").with_arguments(
                serde_json::json!({"profile":"sample.folded"})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    assert!(result.structured_content.is_some());
    assert!(
        !result.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("top_self")
    );
    assert!(
        result.content[0]
            .as_text()
            .unwrap()
            .text
            .contains("truncated=false")
    );
    for (name, arguments) in [
        (
            "profile_find_symbols",
            serde_json::json!({"profile":"sample.folded","query":"A"}),
        ),
        (
            "profile_top",
            serde_json::json!({"profile":"sample.folded"}),
        ),
        (
            "profile_tree",
            serde_json::json!({"profile":"sample.folded"}),
        ),
        (
            "profile_callers",
            serde_json::json!({"profile":"sample.folded","frame":{"frame_name":"A"}}),
        ),
        (
            "profile_callees",
            serde_json::json!({"profile":"sample.folded","frame":{"frame_name":"A"}}),
        ),
        (
            "profile_diff",
            serde_json::json!({"baseline":"sample.folded","candidate":"sample.folded"}),
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
                serde_json::json!({"profile":"sample.folded", "through":{"frame_name":"A"}, "limit":1})
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
    let error = client
        .call_tool(
            CallToolRequestParams::new("profile_summary").with_arguments(
                serde_json::json!({"profile":"missing.folded"})
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
        "profile_not_found"
    );
    assert!(
        client
            .call_tool(
                CallToolRequestParams::new("profile_top").with_arguments(
                    serde_json::json!({"profile":"sample.folded", "limti":1})
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
                    serde_json::json!({"profile":"sample.folded", "limit":"bad"})
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
                serde_json::json!({"profile":"sample.folded", "limit":201})
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
                serde_json::json!({"profile":"sample.folded", "through":{"frame_name":"evil\nname"}})
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

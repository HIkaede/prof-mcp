use super::sample;
use rmcp::model::CacheScope;

#[tokio::test]
async fn tool_order_and_cache_metadata() {
    sample(async |client| {
        let tools = client.list_tools(None).await.unwrap();
        assert_eq!(tools.ttl_ms, Some(0));
        assert_eq!(tools.cache_scope, Some(CacheScope::Private));
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
    })
    .await;
}

#[tokio::test]
async fn tool_schemas() {
    sample(async |client| {
        let tools = client.list_tools(None).await.unwrap();
        assert!(tools.tools.iter().all(|tool| {
            tool.output_schema
                .as_ref()
                .and_then(|schema| schema.get("type"))
                .and_then(|value| value.as_str())
                == Some("object")
        }));
        for tool in &tools.tools {
            let schema = tool.output_schema.as_ref().unwrap();
            assert!(
                schema["properties"].get("next_steps").is_none(),
                "{}",
                tool.name
            );
        }
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
        for tool_index in [2, 3, 4, 5, 6, 7] {
            let schema = tools.tools[tool_index].output_schema.as_ref().unwrap();
            assert_ne!(
                schema["properties"]["data"],
                serde_json::json!({"type":"object"})
            );
            assert!(schema["properties"]["data"].get("$ref").is_some());
            assert!(
                schema["anyOf"][1]["required"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!("retry_hint"))
            );
        }
        assert!(
            summary_schema["anyOf"][1]["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("retry_hint"))
        );
        for (index, definition, fields) in [
            (0, "SummaryData", vec!["total_weight", "frame_count", "unique_stack_count", "max_depth", "stack_concentration", "recursion_detected", "top_self", "top_inclusive", "registry"]),
            (1, "FindData", vec!["query", "mode", "matches"]),
        ] {
            let schema = tools.tools[index].output_schema.as_ref().unwrap();
            assert_eq!(schema["properties"]["data"]["$ref"], format!("#/$defs/{definition}"));
            let data = &schema["$defs"][definition];
            for field in fields {
                assert!(data["properties"].get(field).is_some(), "{definition}.{field}");
                assert!(data["required"].as_array().unwrap().contains(&serde_json::json!(field)));
            }
        }
        let registry = &summary_schema["$defs"]["RegistryOutput"]["properties"];
        for field in ["registry_root", "active", "profile_count", "profiles"] {
            assert!(registry.get(field).is_some());
        }
        assert_eq!(summary_schema["$defs"]["RegistryProfile"]["properties"]["fingerprint"]["type"], "string");
        let matches = &tools.tools[1].output_schema.as_ref().unwrap()["$defs"]["FindData"]["properties"]["matches"];
        assert_eq!(matches["items"]["$ref"], "#/$defs/FrameRow");
        let find_schema = &tools.tools[1].input_schema;
        for index in [1, 2] {
            assert!(
                tools.tools[index].input_schema["properties"]
                    .get("normalize")
                    .is_none()
            );
        }
        let top_schema = tools.tools[2].output_schema.as_ref().unwrap();
        assert!(
            top_schema["$defs"]["TopData"]["properties"]
                .get("grouped_rows")
                .is_none()
        );
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
        for index in [3, 4, 5] {
            let input = &tools.tools[index].input_schema;
            let continuation = &input["$defs"]["Continuation"];
            assert_eq!(continuation["type"], "object");
            assert_eq!(continuation["additionalProperties"], false);
            let required = continuation["required"].as_array().unwrap();
            assert_eq!(required.len(), 4);
            for field in ["profile", "fingerprint", "tool", "cursor"] {
                assert!(required.contains(&serde_json::json!(field)));
            }
            assert_eq!(continuation["properties"]["cursor"]["minItems"], 1);
            assert_eq!(continuation["properties"]["cursor"]["maxItems"], 4097);
            let kinds = input["$defs"]["ContinuationTool"]["enum"]
                .as_array()
                .unwrap();
            assert_eq!(kinds.len(), 3);
            for tool in ["profile_tree", "profile_callers", "profile_callees"] {
                assert!(kinds.contains(&serde_json::json!(tool)));
            }
            assert!(
                input["properties"]["continuation"]
                    .to_string()
                    .contains("#/$defs/Continuation")
            );
            assert!(input["properties"].get("root_node_id").is_none());
            assert!(input["properties"].get("profile_fingerprint").is_none());
            assert_eq!(input["properties"]["max_depth"]["default"], 4);
            let output = tools.tools[index].output_schema.as_ref().unwrap();
            assert_eq!(output["$defs"]["Continuation"], *continuation);
            for definition in output["$defs"].as_object().unwrap().values() {
                if definition["properties"].get("reason").is_some() {
                    let properties = &definition["properties"];
                    assert!(
                        properties["continuation"]
                            .to_string()
                            .contains("#/$defs/Continuation")
                    );
                    assert!(properties.get("profile_fingerprint").is_none());
                    assert!(properties.get("node_path").is_none());
                }
            }
        }
        for index in [2, 6] {
            let input = &tools.tools[index].input_schema["properties"];
            assert!(input.get("frame").is_some());
            assert!(input.get("focus").is_none());
            assert!(input.get("through").is_none());
        }
        assert!(
            tools.tools[2].input_schema["properties"]
                .get("metric")
                .is_some()
        );
        assert!(
            tools.tools[2].input_schema["properties"]
                .get("sort")
                .is_none()
        );
        let top_data = &top_schema["$defs"]["TopData"]["properties"];
        assert!(top_data.get("metric").is_some());
        assert!(top_data.get("frame").is_some());
        assert!(top_data.get("sort").is_none());
        assert!(top_data.get("focus").is_none());
        let paths_output = tools.tools[6].output_schema.as_ref().unwrap();
        let paths_data = &paths_output["$defs"]["PathsData"]["properties"];
        assert!(paths_data.get("frame").is_some());
        assert!(paths_data.get("through").is_none());
        let paths_schema = &tools.tools[6].input_schema["$defs"]["FrameWindowInput"]["oneOf"];
        assert_eq!(paths_schema[0]["properties"]["lines"]["minimum"], 1);
        assert_eq!(paths_schema[0]["properties"]["lines"]["maximum"], 4096);
        assert_eq!(paths_schema[1]["properties"]["lines"]["minimum"], 1);
        assert_eq!(paths_schema[1]["properties"]["lines"]["maximum"], 4096);
        assert_eq!(paths_schema[2]["properties"]["before"]["minimum"], 0);
        assert_eq!(paths_schema[2]["properties"]["before"]["maximum"], 4096);
        assert_eq!(paths_schema[2]["properties"]["after"]["minimum"], 0);
        assert_eq!(paths_schema[2]["properties"]["after"]["maximum"], 4096);
        assert_eq!(
            tools.tools[6].input_schema["properties"]["max_total_frames"]["minimum"],
            1
        );
        assert_eq!(
            tools.tools[6].input_schema["properties"]["max_total_frames"]["maximum"],
            5000
        );
    })
    .await;
}

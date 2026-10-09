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
        for index in [4, 5] {
            let path = &tools.tools[index].input_schema["$defs"]["DirectionContinuationInput"]["properties"]["node_path"];
            assert_eq!(path["minItems"], 1);
            assert_eq!(path["maxItems"], 4096);
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

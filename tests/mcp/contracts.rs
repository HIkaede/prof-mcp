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
        // Responses are shaped by `respond::success`; advertised output schemas
        // only cost the caller context.
        assert!(tools.tools.iter().all(|tool| tool.output_schema.is_none()));
        let find_schema = &tools.tools[1].input_schema;
        for index in [1, 2] {
            assert!(
                tools.tools[index].input_schema["properties"]
                    .get("normalize")
                    .is_none()
            );
        }
        assert_eq!(find_schema["properties"]["limit"]["minimum"], 1);
        assert_eq!(find_schema["properties"]["limit"]["maximum"], 100);
        assert!(
            find_schema["$defs"]["MatchMode"]["enum"]
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
            assert_eq!(continuation["properties"]["cursor"]["minItems"], 2);
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

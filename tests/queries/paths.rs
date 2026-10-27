use crate::support;
use prof_mcp::query::{self, FrameSelector, FrameWindow, TopSort};

#[test]
fn path_selection_orders_weight_ties_by_full_unicode_names() {
    let profile =
        support::profile("根;é;z 7\n根;a;中 7\n根;é;é 7\n根;a;a 7\n根;z 14\n根;a 7\n根;a;a;尾 7\n");
    let selector = FrameSelector {
        frame_name: Some("根".into()),
        frame_id: None,
    };
    let expected = [
        "根;z",
        "根;a",
        "根;a;a",
        "根;a;a;尾",
        "根;a;中",
        "根;é;z",
        "根;é;é",
    ];
    for limit in [1, 2, 4, 7, 20] {
        let result = support::json(
            prof_mcp::query::paths_with_window_budget(
                &profile,
                &selector,
                limit,
                None,
                prof_mcp::query::DEFAULT_MAX_TOTAL_FRAMES,
            )
            .unwrap(),
        );
        let names: Vec<_> = result["data"]["paths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                row["frames"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|name| name.as_str().unwrap())
                    .collect::<Vec<_>>()
                    .join(";")
            })
            .collect();
        assert_eq!(names, expected[..limit.min(expected.len())]);
        assert_eq!(result["scope_weight"], 56);
        assert_eq!(result["truncated"], limit < expected.len());
    }
}

#[test]
fn recursive_path_positions() {
    let profile = support::profile("root;foo;foo;bar 10\nroot;foo;z 5\n");
    let foo = support::frame(&profile, "foo");
    let paths = support::json(
        prof_mcp::query::paths_with_window_budget(
            &profile,
            &FrameSelector {
                frame_id: Some(foo),
                frame_name: None,
            },
            1,
            None,
            prof_mcp::query::DEFAULT_MAX_TOTAL_FRAMES,
        )
        .unwrap(),
    );
    assert_eq!(paths["data"]["frame"], foo);
    assert!(paths["data"].get("through").is_none());
    assert!(paths["truncated"].as_bool().unwrap());
    assert_eq!(
        paths["data"]["paths"][0]["target_positions"],
        serde_json::json!([1, 2])
    );
    assert_eq!(paths["truncation_reasons"][0]["kind"], "row_limit");
    assert_eq!(paths["truncation_reasons"][0]["available"], 2);
}

#[test]
fn recursive_display_windows() {
    let profile = support::profile("root;a;foo;foo;b;leaf 10\n");
    let foo = support::frame(&profile, "foo");
    let selector = FrameSelector {
        frame_id: Some(foo),
        frame_name: None,
    };
    let full = support::json(
        prof_mcp::query::paths_with_window_budget(
            &profile,
            &selector,
            10,
            None,
            prof_mcp::query::DEFAULT_MAX_TOTAL_FRAMES,
        )
        .unwrap(),
    );
    let head = support::json(
        prof_mcp::query::paths_with_window_budget(
            &profile,
            &selector,
            10,
            Some(FrameWindow::Head { lines: 2 }),
            prof_mcp::query::DEFAULT_MAX_TOTAL_FRAMES,
        )
        .unwrap(),
    );
    let tail = support::json(
        prof_mcp::query::paths_with_window_budget(
            &profile,
            &selector,
            10,
            Some(FrameWindow::Tail { lines: 2 }),
            prof_mcp::query::DEFAULT_MAX_TOTAL_FRAMES,
        )
        .unwrap(),
    );
    let around = support::json(
        prof_mcp::query::paths_with_window_budget(
            &profile,
            &selector,
            10,
            Some(FrameWindow::AroundTarget {
                before: 1,
                after: 1,
            }),
            prof_mcp::query::DEFAULT_MAX_TOTAL_FRAMES,
        )
        .unwrap(),
    );
    assert_eq!(head["scope_weight"], full["scope_weight"]);
    assert_eq!(
        head["data"]["paths"][0]["weight"],
        full["data"]["paths"][0]["weight"]
    );
    assert_eq!(
        head["data"]["paths"][0]["frames"],
        serde_json::json!(["root", "a"])
    );
    assert_eq!(head["data"]["paths"][0]["frame_start"], 0);
    assert_eq!(head["data"]["paths"][0]["frame_end"], 2);
    assert_eq!(head["data"]["paths"][0]["omitted_before"], 0);
    assert_eq!(head["data"]["paths"][0]["omitted_after"], 4);
    assert_eq!(
        tail["data"]["paths"][0]["frames"],
        serde_json::json!(["b", "leaf"])
    );
    assert_eq!(tail["data"]["paths"][0]["frame_start"], 4);
    assert_eq!(tail["data"]["paths"][0]["frame_end"], 6);
    assert_eq!(tail["data"]["paths"][0]["omitted_before"], 4);
    assert_eq!(tail["data"]["paths"][0]["omitted_after"], 0);
    assert_eq!(
        around["data"]["paths"][0]["frames"],
        serde_json::json!(["a", "foo", "foo", "b"])
    );
    assert_eq!(
        around["data"]["paths"][0]["target_positions"],
        serde_json::json!([2, 3])
    );
    assert_eq!(
        around["data"]["paths"][0]["display_target_positions"],
        serde_json::json!([1, 2])
    );
    assert_eq!(around["data"]["paths"][0]["frame_start"], 1);
    assert_eq!(around["data"]["paths"][0]["frame_end"], 5);
    assert_eq!(around["data"]["paths"][0]["total_depth"], 6);
    assert!(around["truncated"].as_bool().unwrap());
    assert_eq!(around["truncation_reasons"][0]["kind"], "frame_window");
    assert!(!full["truncated"].as_bool().unwrap());
    assert_eq!(
        prof_mcp::query::paths_with_window_budget(
            &profile,
            &selector,
            10,
            Some(FrameWindow::AroundTarget {
                before: 0,
                after: 0
            }),
            prof_mcp::query::DEFAULT_MAX_TOTAL_FRAMES
        )
        .unwrap_err()
        .code,
        "invalid_budget"
    );
    for window in [
        FrameWindow::Head { lines: 0 },
        FrameWindow::Tail { lines: 4097 },
        FrameWindow::AroundTarget {
            before: 4097,
            after: 0,
        },
        FrameWindow::AroundTarget {
            before: 0,
            after: 4097,
        },
    ] {
        assert_eq!(
            prof_mcp::query::paths_with_window_budget(
                &profile,
                &selector,
                10,
                Some(window),
                prof_mcp::query::DEFAULT_MAX_TOTAL_FRAMES
            )
            .unwrap_err()
            .code,
            "invalid_budget"
        );
    }
}

#[test]
fn path_frame_budget() {
    let profile = support::profile("root;a;b;foo;c;d;e 10\nroot;x;foo;y;z 5\n");
    let foo = support::frame(&profile, "foo");
    let selector = FrameSelector {
        frame_id: Some(foo),
        frame_name: None,
    };
    let bounded =
        support::json(query::paths_with_window_budget(&profile, &selector, 10, None, 5).unwrap());
    let paths = bounded["data"]["paths"].as_array().unwrap();
    assert_eq!(paths.len(), 1);
    assert_eq!(
        paths[0]["frames"],
        serde_json::json!(["a", "b", "foo", "c", "d"])
    );
    assert_eq!(paths[0]["target_positions"], serde_json::json!([3]));
    assert_eq!(paths[0]["display_target_positions"], serde_json::json!([2]));
    assert_eq!(paths[0]["requested_frame_start"], 0);
    assert_eq!(paths[0]["requested_frame_end"], 7);
    assert_eq!(paths[0]["frame_start"], 1);
    assert_eq!(paths[0]["frame_end"], 6);
    assert_eq!(paths[0]["budget_omitted_before"], 1);
    assert_eq!(paths[0]["budget_omitted_after"], 1);
    assert_eq!(
        bounded["data"]["total_frame_budget"],
        serde_json::json!({
            "limit":5,
            "available":12,
            "returned":5,
            "omitted":7,
            "selected_paths":2,
            "returned_paths":1,
            "omitted_paths":1,
            // The dropped path counts as cropped too: all of its frames were omitted.
            "cropped_paths":2,
        })
    );
    assert_eq!(
        bounded["truncation_reasons"][0]["kind"],
        "total_frame_budget"
    );

    let exact_row =
        support::json(query::paths_with_window_budget(&profile, &selector, 10, None, 7).unwrap());
    assert_eq!(exact_row["data"]["paths"].as_array().unwrap().len(), 1);
    assert_eq!(
        exact_row["data"]["paths"][0]["frames"],
        serde_json::json!(["root", "a", "b", "foo", "c", "d", "e"])
    );
    assert_eq!(
        exact_row["data"]["total_frame_budget"],
        serde_json::json!({
            "limit":7,
            "available":12,
            "returned":7,
            "omitted":5,
            "selected_paths":2,
            "returned_paths":1,
            "omitted_paths":1,
            // The second path is dropped once the budget is spent, so it is cropped.
            "cropped_paths":1,
        })
    );
    assert_eq!(
        exact_row["truncation_reasons"][0]["kind"],
        "total_frame_budget"
    );

    for response in [&bounded, &exact_row] {
        let mut reason = response["truncation_reasons"][0]
            .as_object()
            .unwrap()
            .clone();
        reason.remove("kind");
        assert_eq!(
            serde_json::Value::Object(reason),
            response["data"]["total_frame_budget"]
        );
    }

    let head = support::json(
        query::paths_with_window_budget(
            &profile,
            &selector,
            10,
            Some(FrameWindow::Head { lines: 7 }),
            3,
        )
        .unwrap(),
    );
    assert_eq!(
        head["data"]["paths"][0]["frames"],
        serde_json::json!(["root", "a", "b"])
    );
    let tail = support::json(
        query::paths_with_window_budget(
            &profile,
            &selector,
            10,
            Some(FrameWindow::Tail { lines: 7 }),
            3,
        )
        .unwrap(),
    );
    assert_eq!(
        tail["data"]["paths"][0]["frames"],
        serde_json::json!(["c", "d", "e"])
    );
    for invalid in [0, 5_001] {
        assert_eq!(
            query::paths_with_window_budget(&profile, &selector, 10, None, invalid)
                .unwrap_err()
                .code,
            "invalid_budget"
        );
    }
}

#[test]
fn path_window_boundaries() {
    let profile = support::profile("foo;middle;leaf 2\nroot;foo;branch 10\nroot;x;foo;branch 20\n");
    let foo = support::frame(&profile, "foo");
    let leaf = support::frame(&profile, "leaf");
    let foo_selector = FrameSelector {
        frame_id: Some(foo),
        frame_name: None,
    };
    let leaf_selector = FrameSelector {
        frame_id: Some(leaf),
        frame_name: None,
    };
    let root_window = support::json(
        prof_mcp::query::paths_with_window_budget(
            &profile,
            &foo_selector,
            10,
            Some(FrameWindow::AroundTarget {
                before: 5,
                after: 1,
            }),
            prof_mcp::query::DEFAULT_MAX_TOTAL_FRAMES,
        )
        .unwrap(),
    );
    let root_row = root_window["data"]["paths"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["target_positions"] == serde_json::json!([0]))
        .unwrap();
    assert_eq!(root_row["frame_start"], 0);
    assert_eq!(root_row["frame_end"], 2);
    assert_eq!(root_row["omitted_before"], 0);
    assert_eq!(root_row["omitted_after"], 1);

    let leaf_window = support::json(
        prof_mcp::query::paths_with_window_budget(
            &profile,
            &leaf_selector,
            10,
            Some(FrameWindow::AroundTarget {
                before: 1,
                after: 5,
            }),
            prof_mcp::query::DEFAULT_MAX_TOTAL_FRAMES,
        )
        .unwrap(),
    );
    let leaf_row = &leaf_window["data"]["paths"][0];
    assert_eq!(leaf_row["target_positions"], serde_json::json!([2]));
    assert_eq!(leaf_row["frame_start"], 1);
    assert_eq!(leaf_row["frame_end"], 3);
    assert_eq!(leaf_row["omitted_before"], 1);
    assert_eq!(leaf_row["omitted_after"], 0);

    let baseline = support::json(
        prof_mcp::query::paths_with_window_budget(
            &profile,
            &foo_selector,
            2,
            None,
            prof_mcp::query::DEFAULT_MAX_TOTAL_FRAMES,
        )
        .unwrap(),
    );
    let top_before =
        support::json(query::top(&profile, TopSort::SelfWeight, 20, None, None).unwrap());
    let cropped = support::json(
        prof_mcp::query::paths_with_window_budget(
            &profile,
            &foo_selector,
            2,
            Some(FrameWindow::Head { lines: 1 }),
            prof_mcp::query::DEFAULT_MAX_TOTAL_FRAMES,
        )
        .unwrap(),
    );
    let top_after =
        support::json(query::top(&profile, TopSort::SelfWeight, 20, None, None).unwrap());
    assert_eq!(cropped["scope_weight"], baseline["scope_weight"]);
    assert_eq!(cropped["scope_weight"], 32);
    assert_eq!(
        cropped["data"]["paths"][0]["weight"],
        baseline["data"]["paths"][0]["weight"]
    );
    assert_eq!(
        cropped["data"]["paths"][1]["weight"],
        baseline["data"]["paths"][1]["weight"]
    );
    assert_eq!(
        cropped["data"]["paths"][0]["target_positions"],
        serde_json::json!([2])
    );
    assert_eq!(
        cropped["data"]["paths"][1]["target_positions"],
        serde_json::json!([1])
    );
    assert_eq!(top_before, top_after);
    assert_eq!(profile.frame_stats()[foo as usize].inclusive_weight, 32);
}

#[test]
fn uncropped_path_rows() {
    let profile = support::profile("root;a;b;foo 10\n");
    let foo = support::frame(&profile, "foo");
    let result = support::json(
        query::paths_with_window_budget(
            &profile,
            &FrameSelector {
                frame_id: Some(foo),
                frame_name: None,
            },
            5,
            None,
            500,
        )
        .unwrap(),
    );
    let row = &result["data"]["paths"][0];
    assert_eq!(row["omitted_before"], 0);
    assert_eq!(row["budget_omitted_before"], 0);
    assert_eq!(row["requested_frame_start"], 0);
    assert_eq!(row["frame_start"], 0);
    assert_eq!(row["frame_end"], row["total_depth"]);
}

#[test]
fn path_construction_rejects_encoded_names_before_returning_json() {
    let name = "\\\"".repeat(20_000);
    assert!(name.len() < 65536);
    let profile = support::profile(&format!("root;{name} 1\n"));
    let selector = FrameSelector {
        frame_name: Some("root".into()),
        frame_id: None,
    };
    let error = prof_mcp::query::paths_with_window_budget(
        &profile,
        &selector,
        1,
        None,
        prof_mcp::query::DEFAULT_MAX_TOTAL_FRAMES,
    )
    .unwrap_err();
    assert_eq!(error.code, "query_too_large");
    assert_eq!(error.details["resource"], "response_bytes");
    let cropped = support::json(
        prof_mcp::query::paths_with_window_budget(
            &profile,
            &selector,
            1,
            Some(FrameWindow::Head { lines: 1 }),
            prof_mcp::query::DEFAULT_MAX_TOTAL_FRAMES,
        )
        .unwrap(),
    );
    assert_eq!(
        cropped["data"]["paths"][0]["frames"],
        serde_json::json!(["root"])
    );
    assert_eq!(cropped["data"]["paths"][0]["weight"], 1);
}

#[test]
fn mixed_path_truncations_keep_flat_fields_and_reason_order() {
    let profile = support::profile("head;foo;tail 10\nhead;foo;tail2 5\nhead;foo;tail3 1\n");
    let selector = FrameSelector {
        frame_id: Some(support::frame(&profile, "foo")),
        frame_name: None,
    };
    let paths = support::json(
        query::paths_with_window_budget(
            &profile,
            &selector,
            2,
            Some(FrameWindow::Head { lines: 2 }),
            1,
        )
        .unwrap(),
    );
    assert_eq!(
        paths["truncation_reasons"],
        serde_json::json!([
            {"kind":"row_limit","limit":2,"returned":1,"available":3,"omitted":2},
            {"kind":"frame_window","mode":"head","cropped_paths":2,"total_omitted_before":0,"total_omitted_after":2},
            {"kind":"total_frame_budget","limit":1,"available":4,"returned":1,"omitted":3,"selected_paths":2,"returned_paths":1,"omitted_paths":1,"cropped_paths":2}
        ])
    );
}

use crate::support;
use prof_mcp::query::{self, FrameSelector, FrameWindow, TopSort};

#[test]
fn paths_report_recursive_positions_and_row_limits() {
    let profile = support::profile("root;foo;foo;bar 10\nroot;foo;z 5\n");
    let foo = support::frame(&profile, "foo");
    let paths = query::paths(
        &profile,
        &FrameSelector {
            frame_id: Some(foo),
            frame_name: None,
        },
        1,
    )
    .unwrap();
    assert!(paths["truncated"].as_bool().unwrap());
    assert_eq!(
        paths["data"]["paths"][0]["target_positions"],
        serde_json::json!([1, 2])
    );
    assert_eq!(paths["truncation_reasons"][0]["kind"], "row_limit");
    assert_eq!(paths["truncation_reasons"][0]["available"], 2);
}

#[test]
fn path_windows_crop_display_only_and_keep_all_recursive_positions() {
    let profile = support::profile("root;a;foo;foo;b;leaf 10\n");
    let foo = support::frame(&profile, "foo");
    let selector = FrameSelector {
        frame_id: Some(foo),
        frame_name: None,
    };
    let full = query::paths(&profile, &selector, 10).unwrap();
    let head = query::paths_with_window(
        &profile,
        &selector,
        10,
        Some(FrameWindow::Head { lines: 2 }),
    )
    .unwrap();
    let tail = query::paths_with_window(
        &profile,
        &selector,
        10,
        Some(FrameWindow::Tail { lines: 2 }),
    )
    .unwrap();
    let around = query::paths_with_window(
        &profile,
        &selector,
        10,
        Some(FrameWindow::AroundTarget {
            before: 1,
            after: 1,
        }),
    )
    .unwrap();
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
        query::paths_with_window(
            &profile,
            &selector,
            10,
            Some(FrameWindow::AroundTarget {
                before: 0,
                after: 0
            })
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
            query::paths_with_window(&profile, &selector, 10, Some(window))
                .unwrap_err()
                .code,
            "invalid_budget"
        );
    }
}

#[test]
fn paths_total_frame_budget_is_ordered_bounded_and_reports_requested_ranges() {
    let profile = support::profile("root;a;b;foo;c;d;e 10\nroot;x;foo;y;z 5\n");
    let foo = support::frame(&profile, "foo");
    let selector = FrameSelector {
        frame_id: Some(foo),
        frame_name: None,
    };
    let bounded = query::paths_with_window_budget(&profile, &selector, 10, None, 5).unwrap();
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
            "cropped_paths":1,
        })
    );
    assert_eq!(
        bounded["truncation_reasons"][0]["kind"],
        "total_frame_budget"
    );

    let exact_row = query::paths_with_window_budget(&profile, &selector, 10, None, 7).unwrap();
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
            "cropped_paths":0,
        })
    );
    assert_eq!(
        exact_row["truncation_reasons"][0]["kind"],
        "total_frame_budget"
    );

    let head = query::paths_with_window_budget(
        &profile,
        &selector,
        10,
        Some(FrameWindow::Head { lines: 7 }),
        3,
    )
    .unwrap();
    assert_eq!(
        head["data"]["paths"][0]["frames"],
        serde_json::json!(["root", "a", "b"])
    );
    let tail = query::paths_with_window_budget(
        &profile,
        &selector,
        10,
        Some(FrameWindow::Tail { lines: 7 }),
        3,
    )
    .unwrap();
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
fn path_windows_cover_root_leaf_and_multi_stack_boundaries_without_changing_selection() {
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
    let root_window = query::paths_with_window(
        &profile,
        &foo_selector,
        10,
        Some(FrameWindow::AroundTarget {
            before: 5,
            after: 1,
        }),
    )
    .unwrap();
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

    let leaf_window = query::paths_with_window(
        &profile,
        &leaf_selector,
        10,
        Some(FrameWindow::AroundTarget {
            before: 1,
            after: 5,
        }),
    )
    .unwrap();
    let leaf_row = &leaf_window["data"]["paths"][0];
    assert_eq!(leaf_row["target_positions"], serde_json::json!([2]));
    assert_eq!(leaf_row["frame_start"], 1);
    assert_eq!(leaf_row["frame_end"], 3);
    assert_eq!(leaf_row["omitted_before"], 1);
    assert_eq!(leaf_row["omitted_after"], 0);

    let baseline = query::paths(&profile, &foo_selector, 2).unwrap();
    let top_before = query::top(&profile, TopSort::SelfWeight, 20, None, None, false).unwrap();
    let cropped = query::paths_with_window(
        &profile,
        &foo_selector,
        2,
        Some(FrameWindow::Head { lines: 1 }),
    )
    .unwrap();
    let top_after = query::top(&profile, TopSort::SelfWeight, 20, None, None, false).unwrap();
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
    assert_eq!(profile.frame_stats[foo as usize].inclusive_weight, 32);
}

#[test]
fn paths_rows_omit_zero_bookkeeping_when_nothing_is_cropped() {
    let profile = support::profile("root;a;b;foo 10\n");
    let foo = support::frame(&profile, "foo");
    let result = query::paths_with_window_budget(
        &profile,
        &FrameSelector {
            frame_id: Some(foo),
            frame_name: None,
        },
        5,
        None,
        500,
    )
    .unwrap();
    let row = &result["data"]["paths"][0];
    assert!(row.get("omitted_before").is_none());
    assert!(row.get("budget_omitted_before").is_none());
    assert!(row.get("requested_frame_start").is_none());
    assert_eq!(row["frame_start"], 0);
    assert_eq!(row["frame_end"], row["total_depth"]);
}

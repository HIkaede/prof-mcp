mod support;

use prof_mcp::query::{self, FrameSelector, FrameWindow, MatchMode, TopSort};

#[test]
fn self_inclusive_find_and_focused_top_have_exact_scopes() {
    let profile = support::profile("root;A;B 30\nroot;A;C 20\nroot;A 5\n");
    let a = support::frame(&profile, "A");
    assert_eq!(profile.frame_stats[a as usize].self_weight, 5);
    assert_eq!(profile.frame_stats[a as usize].inclusive_weight, 55);
    let find = query::find_symbols(&profile, "A", MatchMode::Contains, 20).unwrap();
    assert_eq!(find["data"]["matches"][0]["name"], "A");
    let top = query::top(
        &profile,
        TopSort::SelfWeight,
        20,
        Some(&FrameSelector {
            frame_id: Some(a),
            frame_name: None,
        }),
        Some("B|C"),
    )
    .unwrap();
    assert_eq!(top["scope_weight"], 55);
    assert_eq!(top["data"]["rows"].as_array().unwrap().len(), 2);
    assert_eq!(top["data"]["rows"][0]["name"], "B");
    let self_top = query::top(&profile, TopSort::SelfWeight, 20, None, Some("^A$")).unwrap();
    assert_eq!(
        self_top["data"]["rows"][0]["profile_percent"],
        serde_json::json!(100.0 * 5.0 / 55.0)
    );
}

#[test]
fn tree_pruning_is_deterministic_and_continuations_are_guarded() {
    let profile = support::profile("A 5\nB 4\nC 1\n");
    let one = query::tree(&profile, 0, None, 0, 1, 0.0).unwrap();
    assert!(one["truncated"].as_bool().unwrap());
    assert_eq!(one["data"]["root"]["omitted_children"], 3);
    let threshold = query::tree(&profile, 0, None, 4, 64, 20.0).unwrap();
    assert_eq!(threshold["data"]["root"]["omitted_children"], 1);
    let tree = query::tree(&profile, 0, None, 4, 2, 0.0).unwrap();
    let child_id = tree["data"]["root"]["children"][0]["node_id"]
        .as_u64()
        .unwrap() as u32;
    let error = query::tree(&profile, child_id, None, 4, 64, 0.0).unwrap_err();
    assert_eq!(error.code, "profile_changed");
    let page = query::tree(
        &profile,
        child_id,
        Some(&profile.source.fingerprint),
        4,
        64,
        0.0,
    )
    .unwrap();
    assert_eq!(page["scope_weight"], 5);
    let repeat = query::tree(&profile, 0, None, 4, 2, 0.0).unwrap();
    assert_eq!(tree, repeat);
}

#[test]
fn paths_positions_limits_selectors_and_regex_budgets_are_enforced() {
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
    assert_eq!(
        query::find_symbols(&profile, "[", MatchMode::Regex, 20)
            .unwrap_err()
            .code,
        "invalid_regex"
    );
    assert_eq!(
        query::find_symbols(&profile, &"a".repeat(4097), MatchMode::Regex, 20)
            .unwrap_err()
            .code,
        "invalid_regex"
    );
    assert_eq!(
        query::paths(
            &profile,
            &FrameSelector {
                frame_id: Some(foo),
                frame_name: Some("foo".into())
            },
            1
        )
        .unwrap_err()
        .code,
        "invalid_frame_selector"
    );
    assert_eq!(
        query::top(&profile, TopSort::SelfWeight, 201, None, None)
            .unwrap_err()
            .code,
        "invalid_budget"
    );
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
    assert_eq!(around["data"]["paths"][0]["frame_start"], 1);
    assert_eq!(around["data"]["paths"][0]["frame_end"], 5);
    assert_eq!(around["data"]["paths"][0]["total_depth"], 6);
    assert!(around["truncated"].as_bool().unwrap());
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
    let top_before = query::top(&profile, TopSort::SelfWeight, 20, None, None).unwrap();
    let cropped = query::paths_with_window(
        &profile,
        &foo_selector,
        2,
        Some(FrameWindow::Head { lines: 1 }),
    )
    .unwrap();
    let top_after = query::top(&profile, TopSort::SelfWeight, 20, None, None).unwrap();
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

mod support;

use prof_mcp::query::{self, FrameSelector, FrameWindow, MatchMode, TopSort};

#[test]
fn self_inclusive_find_and_focused_top_have_exact_scopes() {
    let profile = support::profile("root;A;B 30\nroot;A;C 20\nroot;A 5\n");
    let a = support::frame(&profile, "A");
    assert_eq!(profile.frame_stats[a as usize].self_weight, 5);
    assert_eq!(profile.frame_stats[a as usize].inclusive_weight, 55);
    let find = query::find_symbols(&profile, "A", MatchMode::Contains, 20, false).unwrap();
    assert_eq!(find["data"]["matches"][0]["name"], "A");
    assert_eq!(
        find["data"]["symbol_metadata"],
        "folded_frames_and_observed_context_only"
    );
    assert_eq!(
        find["data"]["matches"][0]["context_hint"]["top_callees"][0]["name"],
        "B"
    );
    let top = query::top(
        &profile,
        TopSort::SelfWeight,
        20,
        Some(&FrameSelector {
            frame_id: Some(a),
            frame_name: None,
        }),
        Some("B|C"),
        false,
    )
    .unwrap();
    assert_eq!(top["scope_weight"], 55);
    assert_eq!(top["data"]["rows"].as_array().unwrap().len(), 2);
    assert_eq!(top["data"]["rows"][0]["name"], "B");
    let self_top = query::top(&profile, TopSort::SelfWeight, 20, None, Some("^A$"), false).unwrap();
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
    assert_eq!(one["truncation_reasons"][0]["kind"], "depth_limit");
    assert_eq!(one["truncation_reasons"][0]["omitted_children"], 3);
    assert_eq!(one["truncation_reasons"][0]["omitted_weight"], 10);
    assert_eq!(one["data"]["continuations"].as_array().unwrap().len(), 3);
    assert_eq!(one["data"]["continuations_available"], 3);
    assert_eq!(one["data"]["continuations_omitted"], 0);
    assert_eq!(
        one["data"]["continuations"][0]["profile_fingerprint"],
        profile.source.fingerprint
    );
    let continuation = one["data"]["continuations"][0]["node_id"].as_u64().unwrap() as u32;
    let continuation_page = query::tree(
        &profile,
        continuation,
        Some(&profile.source.fingerprint),
        4,
        64,
        0.0,
    )
    .unwrap();
    assert_eq!(continuation_page["data"]["root"]["node_id"], continuation);
    let threshold = query::tree(&profile, 0, None, 4, 64, 20.0).unwrap();
    assert_eq!(threshold["data"]["root"]["omitted_children"], 1);
    assert_eq!(
        threshold["truncation_reasons"][0]["kind"],
        "min_scope_percent"
    );
    let tree = query::tree(&profile, 0, None, 4, 2, 0.0).unwrap();
    assert_eq!(tree["truncation_reasons"][0]["kind"], "node_budget");
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
    assert_eq!(paths["truncation_reasons"][0]["kind"], "row_limit");
    assert_eq!(paths["truncation_reasons"][0]["available"], 2);
    assert_eq!(
        query::find_symbols(&profile, "[", MatchMode::Regex, 20, false)
            .unwrap_err()
            .code,
        "invalid_regex"
    );
    assert_eq!(
        query::find_symbols(&profile, &"a".repeat(4097), MatchMode::Regex, 20, false)
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
        query::top(&profile, TopSort::SelfWeight, 201, None, None, false)
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
fn summary_reports_concentration_and_recursion() {
    let profile = support::profile("root;foo;foo;foo;bar 10\nroot;a;b;c 5\n");
    let summary = query::summary(&profile);
    let concentration = &summary["data"]["stack_concentration"];
    assert_eq!(concentration["unique_stack_count"], 2);
    assert!(concentration["top_10_stacks_percent"].as_f64().unwrap() > 60.0);
    let recursion = summary["data"]["recursion_detected"].as_array().unwrap();
    assert_eq!(recursion.len(), 1);
    assert_eq!(recursion[0]["name"], "foo");
    assert_eq!(recursion[0]["max_occurrences_per_stack"], 3);
    assert_eq!(recursion[0]["affected_weight"], 10);
}

#[test]
fn top_grouped_mode_aggregates_template_variants() {
    let profile = support::profile("root;construct<A, B> 10\nroot;construct<C> 5\nroot;other 20\n");
    let grouped = query::top(&profile, TopSort::SelfWeight, 10, None, None, true).unwrap();
    let rows = grouped["data"]["grouped_rows"].as_array().unwrap();
    assert_eq!(grouped["data"]["rows"].as_array().unwrap().len(), 0);
    let construct = rows
        .iter()
        .find(|row| row["normalized_name"] == "construct")
        .expect("construct group");
    assert_eq!(construct["member_count"], 2);
    assert_eq!(construct["self_weight"], 15);
    assert_eq!(construct["members"].as_array().unwrap().len(), 2);
}

#[test]
fn find_symbols_grouped_mode_sums_variant_weights() {
    let profile = support::profile("root;construct<A> 10\nroot;_Construct<B> 5\n");
    let grouped =
        query::find_symbols(&profile, "onstruct", MatchMode::Contains, 10, false).unwrap();
    assert_eq!(grouped["data"]["matches"].as_array().unwrap().len(), 2);
    let normalized =
        query::find_symbols(&profile, "onstruct", MatchMode::Contains, 10, true).unwrap();
    let matches = normalized["data"]["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 2);
    for row in matches {
        assert_eq!(row["member_count"], 1);
        assert!(row["members"].as_array().unwrap().len() <= 5);
    }
}

#[test]
fn normalize_frame_name_strips_balanced_templates_only() {
    use prof_mcp::query;
    // Public behavior is exercised through top/find_symbols; this asserts
    // the grouping key through a grouped query with tricky names.
    let profile = support::profile("root;operator<< 7\nroot;make_unique<T> 3\n");
    let grouped = query::top(&profile, TopSort::SelfWeight, 10, None, None, true).unwrap();
    let rows = grouped["data"]["grouped_rows"].as_array().unwrap();
    let names: Vec<_> = rows
        .iter()
        .map(|row| row["normalized_name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"operator<<"));
    assert!(names.contains(&"make_unique"));
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

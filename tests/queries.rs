mod support;

use rmdb_prof_mcp::query::{self, FrameSelector, MatchMode, TopSort};

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

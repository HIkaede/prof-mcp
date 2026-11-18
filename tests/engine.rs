use prof_mcp::{
    profile::{BuildLimits, Profile, ProfileBuilder},
    query::{
        engine::{ExplorePlan, QueryPlan, explore, query},
        selection::{Predicate, Selection},
    },
};
use serde_json::{Value, json};

fn profile(input: &str) -> Profile {
    ProfileBuilder::new(BuildLimits::default())
        .from_reader(std::io::Cursor::new(input), "/engine.folded".into())
        .unwrap()
}

fn select(profile: &Profile, predicate: Value) -> Selection {
    Selection::new(
        profile,
        serde_json::from_value::<Predicate>(predicate).unwrap(),
    )
    .unwrap()
}

fn run(profile: &Profile, selection: &Selection, input: Value) -> Value {
    let plan = serde_json::from_value::<QueryPlan>(input).unwrap();
    serde_json::to_value(query(profile, selection.clone(), plan).unwrap()).unwrap()
}

fn default_selection(profile: &Profile) -> Selection {
    Selection::new(profile, Predicate::default()).unwrap()
}

#[test]
fn filter_totals_and_coverage_use_the_complete_selection() {
    let profile = profile(
        "main;exec;alloc 50\nmain;exec;serialize;alloc 30\nmain;exec;compute 20\nmain;bg;alloc 100\n",
    );
    let selection = select(&profile, json!({"contains_all":[{"frame_name":"exec"}]}));
    let refine = json!({"exclude_any":[{"frame_name":"serialize"}]});
    let first = run(
        &profile,
        &selection,
        json!({
            "refine":refine,
            "view":{"kind":"frames","stat":"self","limit":1}
        }),
    );
    assert_eq!(
        first["scope"],
        json!({"profile_weight":200,"selected_weight":70,"stack_shapes":2})
    );
    assert_eq!(
        first["totals"],
        json!({
            "rows_additive":true, "matched_weight":70, "returned_weight":50,
            "omitted_weight":20, "filtered_weight":0
        })
    );
    let filtered = run(
        &profile,
        &selection,
        json!({
            "refine":refine,
            "view":{"kind":"frames","stat":"self","where":{"name_exact":"alloc"},"limit":1}
        }),
    );
    assert_eq!(filtered["scope"], first["scope"]);
    assert_eq!(filtered["totals"]["matched_weight"], 50);
    assert_eq!(filtered["totals"]["omitted_weight"], 0);
    assert_eq!(filtered["totals"]["filtered_weight"], 20);
    assert_eq!(filtered["table"]["rows"][0][3], 71.43);
    assert_eq!(filtered["table"]["rows"][0][4], 25.0);
    let inclusive = run(
        &profile,
        &selection,
        json!({
            "refine":refine,
            "view":{"kind":"frames","stat":"inclusive","where":{"name_regex":"main|exec"},"limit":1}
        }),
    );
    assert_eq!(inclusive["totals"]["rows_additive"], false);
    assert!(inclusive["totals"].get("omitted_weight").is_none());
    assert_eq!(inclusive["totals"]["stack_union_weight"], 70);
    let aggregate = run(
        &profile,
        &selection,
        json!({
            "refine":refine,
            "view":{"kind":"aggregate","where":{"name_regex":"alloc|exec"}}
        }),
    );
    assert_eq!(
        aggregate["aggregate"],
        json!({
            "self_sum":50,"stack_union_weight":70,"fact_count":2,"stack_shapes":2
        })
    );
}

#[test]
fn recursive_contexts_and_direction_anchor_each_choose_an_occurrence() {
    let profile = profile("root;A;B;A;C 10\nroot;A;D 20\n");
    let selection = default_selection(&profile);
    let contexts = run(
        &profile,
        &selection,
        json!({
            "view":{"kind":"contexts","frame":{"frame_name":"A"}}
        }),
    );
    let rows = contexts["table"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][5], 30);
    assert_eq!(rows[1][5], 10);
    assert_eq!(contexts["totals"]["rows_additive"], false);

    for (kind, occurrence, expected) in [
        ("callees", "outermost", vec![("D", 20), ("B", 10)]),
        ("callees", "innermost", vec![("D", 20), ("C", 10)]),
        ("callers", "outermost", vec![("root", 30)]),
        ("callers", "innermost", vec![("root", 20), ("B", 10)]),
    ] {
        let result = run(
            &profile,
            &selection,
            json!({
                "view":{"kind":kind,"anchor":{"frame_name":"A"},"occurrence":occurrence}
            }),
        );
        assert_eq!(result["anchor"]["weight"], 30);
        assert_eq!(result["anchor"]["occurrence"], occurrence);
        let pairs = result["table"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| (row[1].as_str().unwrap(), row[2].as_u64().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(pairs, expected);
    }
}

#[test]
fn direct_children_preserve_self_inside_anchor_with_broader_selection() {
    let profile = profile("root;A 10\nroot;A;B 30\nelse 5\n");
    let selection = default_selection(&profile);
    let context = profile
        .context_for_prefix(&[
            profile.frame_id("root").unwrap(),
            profile.frame_id("A").unwrap(),
        ])
        .unwrap();
    let children = run(
        &profile,
        &selection,
        json!({
            "view":{"kind":"children","context":context}
        }),
    );
    assert_eq!(children["scope"]["selected_weight"], 45);
    assert_eq!(children["anchor"]["weight"], 40);
    assert_eq!(children["anchor"]["self_weight"], 10);
    assert_eq!(children["table"]["rows"][0][5], 30);
    assert_eq!(children["totals"]["matched_weight"], 30);
    let suffix = run(
        &profile,
        &selection,
        json!({
            "view":{"kind":"frames","stat":"inclusive","path_range":"below_anchor","anchor":context}
        }),
    );
    let rows = suffix["table"]["rows"].as_array().unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row[1].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["A", "B"]
    );
    assert_eq!(suffix["scope"]["selected_weight"], 45);
}

#[test]
fn path_display_window_preserves_weights_and_offsets() {
    let profile = profile("entry1;A;leaf 10\nthread_start;B;leaf 20\npartial;leaf 30\n");
    let selection = default_selection(&profile);
    let result = run(
        &profile,
        &selection,
        json!({
            "view":{"kind":"paths","window":{"kind":"tail","frames":1}}
        }),
    );
    assert_eq!(result["completeness"]["paths_complete"], false);
    let rows = result["table"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    for (row, (weight, depth)) in rows.iter().zip([(30, 2), (20, 3), (10, 3)]) {
        assert_eq!(row[1][0][1], "leaf");
        assert_eq!(row[2], weight);
        assert_eq!(row[5], depth - 1);
        assert_eq!(row[6], depth);
        assert_eq!(row[7], depth);
    }
    assert_eq!(result["totals"]["matched_weight"], 60);
}

#[test]
fn pagination_reuses_exact_sort_and_binds_plan_snapshot_and_position() {
    let profile = profile(
        &((0..300)
            .map(|i| format!("root;f{i:03} {}\n", i % 5 + 1))
            .collect::<String>()),
    );
    let selection = default_selection(&profile);
    let mut names = Vec::new();
    let mut cursor = Value::Null;
    loop {
        let page = run(
            &profile,
            &selection,
            json!({
                "view":{"kind":"frames","stat":"self","limit":if names.is_empty() {17} else {62}},
                "cursor":cursor
            }),
        );
        assert_eq!(page["totals"]["matched_weight"], 900);
        names.extend(
            page["table"]["rows"]
                .as_array()
                .unwrap()
                .iter()
                .map(|row| row[1].as_str().unwrap().to_string()),
        );
        if page["cursor"].is_null() {
            break;
        }
        cursor = page["cursor"].clone();
        assert!(names.len() <= 300);
    }
    let mut expected: Vec<_> = (0..300).collect();
    expected.sort_by_key(|i| (std::cmp::Reverse(i % 5 + 1), *i));
    assert_eq!(
        names,
        expected
            .iter()
            .map(|i| format!("f{i:03}"))
            .collect::<Vec<_>>()
    );

    let plan = serde_json::from_value::<QueryPlan>(json!({
        "view":{"kind":"frames","stat":"inclusive"},"cursor":cursor
    }))
    .unwrap();
    assert_eq!(
        query(&profile, selection, plan).unwrap_err().code,
        "invalid_cursor"
    );
}

#[test]
fn named_path_prefix_is_available_in_one_query_and_survives_pagination() {
    let profile = profile(concat!(
        "root;branch;first 20\n",
        "root;branch;second 10\n",
        "root;branch;retry 30\n",
        "elsewhere;root;branch;outside 40\n",
    ));
    let root = default_selection(&profile);
    let view = json!({"kind":"frames","stat":"self","limit":1});
    let first = run(
        &profile,
        &root,
        json!({
            "refine":{"all":[{"path_prefix":["root","branch"]},{"exclude_any":[{"frame_name":"retry"}]}]},
            "view":view
        }),
    );
    assert_eq!(first["scope"]["selected_weight"], 30);
    assert_eq!(first["table"]["rows"][0][1], "first");
    let pinned = serde_json::from_value::<Selection>(first["selection"].clone()).unwrap();
    let second = run(
        &profile,
        &pinned,
        json!({"cursor":first["cursor"],"view":view}),
    );
    assert_eq!(second["selection"], first["selection"]);
    assert_eq!(second["scope"], first["scope"]);
    assert_eq!(second["table"]["rows"][0][1], "second");
    assert!(second.get("cursor").is_none());

    for (prefix, weight) in [(json!([]), 100), (json!(["root", "outside"]), 0)] {
        let page = run(
            &profile,
            &root,
            json!({"refine":{"path_prefix":prefix},"view":view}),
        );
        let pinned = serde_json::from_value::<Selection>(page["selection"].clone()).unwrap();
        let next = run(
            &profile,
            &pinned,
            json!({"cursor":page["cursor"],"view":view}),
        );
        assert_eq!(page["scope"]["selected_weight"], weight);
        assert_eq!(next["scope"], page["scope"]);
        if weight == 0 {
            assert_eq!(page["table"]["rows"], json!([]));
            assert!(page.get("cursor").is_none());
            assert_eq!(next, page);
            let plan =
                serde_json::from_value(json!({"cursor":first["cursor"],"view":view})).unwrap();
            assert_eq!(
                query(&profile, pinned, plan).unwrap_err().code,
                "invalid_cursor"
            );
        } else {
            assert_eq!(pinned, root);
            assert_eq!(page["table"]["rows"][0][1], "outside");
            assert_eq!(next["table"]["rows"][0][1], "retry");
        }
    }

    for prefix in [json!([0]), json!([{"frame_name":"root"}])] {
        assert!(serde_json::from_value::<Predicate>(json!({"path_prefix":prefix})).is_err());
    }
}

#[test]
fn explore_focus_refines_selection_and_panels_keep_it() {
    let profile = profile("root;A;B 10\nroot;A;C 20\nroot;D 30\n");
    let selection = default_selection(&profile);
    let plan = serde_json::from_value::<ExplorePlan>(json!({"anchor":{"frame_name":"A"}})).unwrap();
    let result = serde_json::to_value(explore(&profile, selection, plan).unwrap()).unwrap();
    assert_eq!(result["scope"]["selected_weight"], 30);
    assert_eq!(result["anchor"]["weight"], 30);
    let panels = result["panels"].as_array().unwrap();
    assert_eq!(
        panels
            .iter()
            .map(|p| p["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["contexts", "callers", "paths"]
    );
}

#[test]
fn query_cancellation_never_returns_partial_exact_results() {
    use prof_mcp::query::{Cancellation, engine::query_cancellable};
    use std::cell::Cell;

    let profile = profile(
        &(0..3000)
            .map(|i| format!("root;f{i:04} 1\n"))
            .collect::<String>(),
    );
    let selection = default_selection(&profile);
    let plan = serde_json::from_value::<QueryPlan>(json!({
        "view":{"kind":"frames","stat":"self","where":{"name_regex":"f"}}
    }))
    .unwrap();
    let checks = Cell::new(0);
    let stop = || {
        checks.set(checks.get() + 1);
        checks.get() > 4
    };
    assert_eq!(
        query_cancellable(&profile, selection, plan, Cancellation::new(&stop))
            .unwrap_err()
            .code,
        "query_cancelled"
    );
}

#[test]
fn zero_scope_has_null_percent_and_empty_tables() {
    let profile = profile("root;A 10\nroot;B 20\n");
    let selection = select(
        &profile,
        json!({"all":[{"contains_all":[{"frame_name":"A"}]},{"contains_all":[{"frame_name":"B"}]}]}),
    );
    let page = run(&profile, &selection, json!({"view":{"kind":"children"}}));
    assert_eq!(page["scope"]["selected_weight"], 0);
    assert_eq!(page["table"]["rows"], json!([]));
    assert!(page["anchor"]["scope_percent"].is_null());
    let frames = run(
        &profile,
        &selection,
        json!({"view":{"kind":"frames","stat":"self","include_zero":true}}),
    );
    assert_eq!(frames["table"]["rows"], json!([]));
}

#[test]
fn recursing_leaf_self_weight_is_independent_of_relative_occurrence() {
    let profile = profile("root;A;B;A 10\n");
    let selection = default_selection(&profile);
    let result = run(
        &profile,
        &selection,
        json!({
            "view":{"kind":"callees","anchor":{"frame_name":"A"},"occurrence":"outermost"}
        }),
    );
    assert_eq!(result["anchor"]["self_weight"], 10);
    assert_eq!(result["totals"]["matched_weight"], 10);
    assert_eq!(result["anchor"]["no_neighbor_weight"], 0);
}

#[test]
fn weighted_query_cursor_is_bound_to_the_external_scope_key() {
    use prof_mcp::query::engine::{FactWeight, MetricScope, query_weighted};
    use std::collections::BTreeMap;

    let profile = profile("root;A 1\nroot;B 1\n");
    let selection = default_selection(&profile);
    let weights = BTreeMap::from([
        (
            0,
            FactWeight {
                weight: 6,
                fact_count: 2,
            },
        ),
        (
            1,
            FactWeight {
                weight: 7,
                fact_count: 3,
            },
        ),
    ]);
    let mut plan = serde_json::from_value::<QueryPlan>(json!({
        "metric":"sample_period","view":{"kind":"paths","limit":1}
    }))
    .unwrap();
    let mut scope = MetricScope {
        profile_weight: 13,
        event_stream_id: Some("e1".into()),
        scope_key: Some("time:10,20".into()),
    };
    let page = query_weighted(
        &profile,
        selection.clone(),
        plan.clone(),
        scope.clone(),
        &weights,
    )
    .unwrap();
    assert_eq!(page.scope.selected_weight, 13);
    plan.cursor = page.projection.cursor;
    assert!(
        query_weighted(
            &profile,
            selection.clone(),
            plan.clone(),
            scope.clone(),
            &weights
        )
        .is_ok()
    );
    scope.scope_key = Some("time:20,30".into());
    assert_eq!(
        query_weighted(&profile, selection, plan, scope, &weights)
            .unwrap_err()
            .code,
        "invalid_cursor"
    );
}

#[test]
fn weighted_zero_measures_keep_observed_shapes_and_null_shares() {
    use prof_mcp::query::engine::{FactWeight, MetricScope, query_weighted};
    use std::collections::BTreeMap;

    let profile = profile("root;A 1\nroot;B 1\n");
    let selection = default_selection(&profile);
    let weights = BTreeMap::from([(
        0,
        FactWeight {
            weight: 0,
            fact_count: 3,
        },
    )]);
    let scope = MetricScope {
        profile_weight: 0,
        event_stream_id: Some("e1".into()),
        scope_key: None,
    };
    let run = |view| {
        let plan =
            serde_json::from_value::<QueryPlan>(json!({"metric":"sample_period","view":view}))
                .unwrap();
        serde_json::to_value(
            query_weighted(&profile, selection.clone(), plan, scope.clone(), &weights).unwrap(),
        )
        .unwrap()
    };
    let paths = run(json!({"kind":"paths"}));
    assert_eq!(paths["scope"]["stack_shapes"], 1);
    assert_eq!(paths["table"]["rows"][0][2], 0);
    assert!(paths["table"]["rows"][0][3].is_null());
    assert!(paths["table"]["rows"][0][4].is_null());
    let frames = run(json!({"kind":"frames","stat":"self","include_zero":true}));
    assert_eq!(frames["table"]["rows"].as_array().unwrap().len(), 2);
    let children = run(json!({"kind":"children"}));
    assert_eq!(children["table"]["rows"].as_array().unwrap().len(), 1);
    let aggregated = run(json!({"kind":"aggregate"}));
    assert_eq!(aggregated["aggregate"]["fact_count"], 3);
}

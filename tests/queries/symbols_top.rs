use crate::support;
use prof_mcp::query::{self, FrameSelector, MatchMode, TopSort};

#[test]
fn find_returns_frame_stats() {
    let profile = support::profile("root;A;B 30\nroot;A;C 20\nroot;A 5\n");
    let a = support::frame(&profile, "A");
    assert_eq!(profile.frame_stats[a as usize].self_weight, 5);
    assert_eq!(profile.frame_stats[a as usize].inclusive_weight, 55);
    let find = query::find_symbols(&profile, "A", MatchMode::Contains, 20).unwrap();
    assert_eq!(
        find["data"],
        serde_json::json!({
            "query": "A",
            "mode": "contains",
            "matches": [{
                "frame_id": a,
                "name": "A",
                "self_weight": 5,
                "inclusive_weight": 55,
                "stack_count": 3,
                "profile_percent": 100.0,
                "scope_percent": 100.0
            }]
        })
    );
}

#[test]
fn focused_top_scope() {
    let profile = support::profile("root;A;B 30\nroot;A;C 20\nroot;A 5\n");
    let a = support::frame(&profile, "A");
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
    assert_eq!(top["data"]["metric"], "self");
    assert_eq!(top["data"]["frame"], a);
    assert!(top["data"].get("sort").is_none());
    assert!(top["data"].get("focus").is_none());
    assert_eq!(top["data"]["rows"].as_array().unwrap().len(), 2);
    assert_eq!(top["data"]["rows"][0]["name"], "B");
    let self_top = query::top(&profile, TopSort::SelfWeight, 20, None, Some("^A$")).unwrap();
    assert!(self_top["data"]["frame"].is_null());
    assert_eq!(
        self_top["data"]["rows"][0]["profile_percent"],
        serde_json::json!(100.0 * 5.0 / 55.0)
    );
}

#[test]
fn full_scope_top_matches_global_with_recursion_and_filters() {
    let profile = support::profile("root;anchor;anchor;B 10\nroot;A;anchor;C 10\nroot;anchor 5\n");
    let anchor = support::frame(&profile, "anchor");
    let selector = FrameSelector {
        frame_id: Some(anchor),
        frame_name: None,
    };
    for metric in [TopSort::SelfWeight, TopSort::Inclusive] {
        for regex in [None, Some("B|C"), Some("absent")] {
            for limit in [1, 20] {
                let scoped = query::top(&profile, metric, limit, Some(&selector), regex).unwrap();
                let mut global = query::top(&profile, metric, limit, None, regex).unwrap();
                global["data"]["frame"] = serde_json::json!(anchor);
                assert_eq!(scoped, global);
            }
        }
    }
}

#[test]
fn partial_scope_top_keeps_subset_weights_and_ranking() {
    let profile = support::profile("root;A;B 10\nroot;C;B 100\nroot;A;D 20\n");
    let selector = FrameSelector {
        frame_name: Some("A".into()),
        frame_id: None,
    };
    for metric in [TopSort::SelfWeight, TopSort::Inclusive] {
        let top = query::top(&profile, metric, 1, Some(&selector), Some("B|D")).unwrap();
        assert_eq!(top["scope_weight"], 30);
        let row = &top["data"]["rows"][0];
        assert_eq!(row["name"], "D");
        assert_eq!(row["self_weight"], 20);
        assert_eq!(row["inclusive_weight"], 20);
        assert_eq!(row["stack_count"], 1);
        assert_eq!(top["truncation_reasons"][0]["available"], 2);
    }
}

#[test]
fn sparse_top_preserves_recursive_counts_and_weight_ties() {
    for padding in [0, 256] {
        let mut input = "root;A;A;B 10\nroot;A;C 10\nroot;D 100\n".to_owned();
        for i in 0..padding {
            input.push_str(&format!("root;unused{i} 1\n"));
        }
        let profile = support::profile(&input);
        let selector = FrameSelector {
            frame_name: Some("A".into()),
            frame_id: None,
        };
        for metric in [TopSort::SelfWeight, TopSort::Inclusive] {
            let result = query::top(&profile, metric, 1, Some(&selector), Some("B|C")).unwrap();
            assert_eq!(result["scope_weight"], 20);
            let row = &result["data"]["rows"][0];
            assert_eq!(row["name"], "B");
            assert_eq!(row["self_weight"], 10);
            assert_eq!(row["scope_percent"], 50.0);
            assert_eq!(result["truncation_reasons"][0]["available"], 2);
            let result = query::top(&profile, metric, 20, Some(&selector), Some("^A$")).unwrap();
            let row = &result["data"]["rows"][0];
            assert_eq!(row["self_weight"], 0);
            assert_eq!(row["inclusive_weight"], 20);
            assert_eq!(row["stack_count"], 2);
        }
    }
}

#[test]
fn summary_shape() {
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
    let warnings = summary["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .all(|warning| !warning.as_str().unwrap().contains("profile_"))
    );
    assert!(
        warnings[1]
            .as_str()
            .unwrap()
            .contains("count each stack once")
    );
    let missing = query::find_symbols(&profile, "absent", MatchMode::Contains, 20).unwrap();
    assert_eq!(
        missing["warnings"],
        serde_json::json!(["No exact frame identities matched."])
    );
}

#[test]
fn template_frames_are_distinct() {
    let profile = support::profile("root;foo<int>;foo<double> 10\nroot;foo<int>;foo<int> 5\n");
    let found = query::find_symbols(&profile, "foo", MatchMode::Contains, 10).unwrap();
    let rows = found["data"]["matches"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "foo<int>");
    assert_eq!(rows[0]["self_weight"], 5);
    assert_eq!(rows[0]["inclusive_weight"], 15);
    assert_eq!(rows[0]["stack_count"], 2);
    assert_eq!(rows[1]["name"], "foo<double>");
    assert_eq!(rows[1]["self_weight"], 10);
    assert_eq!(rows[1]["inclusive_weight"], 10);
    assert_eq!(rows[1]["stack_count"], 1);
    assert_ne!(rows[0]["frame_id"], rows[1]["frame_id"]);
    let top = query::top(&profile, TopSort::Inclusive, 10, None, Some("foo")).unwrap();
    assert_eq!(top["data"]["rows"], found["data"]["matches"]);
    assert!(top["data"].get("grouped_rows").is_none());
}

#[test]
fn frame_names_are_opaque() {
    let names = [
        "operator<<",
        "make_unique<T>",
        "make<A<B>, C>",
        "make<T>::call<U>",
        "<T>",
        "unclosed<T",
        "函数<T>",
    ];
    let input = names
        .iter()
        .map(|name| format!("root;{name} 1\n"))
        .collect::<String>();
    let profile = support::profile(&input);
    for name in names {
        let found = query::find_symbols(&profile, name, MatchMode::Contains, 10).unwrap();
        assert_eq!(found["data"]["matches"][0]["name"], name);
        assert_eq!(
            found["data"]["matches"][0]["frame_id"],
            support::frame(&profile, name)
        );
    }
    let top = query::top(&profile, TopSort::SelfWeight, 10, None, None).unwrap();
    let rows = top["data"]["rows"].as_array().unwrap();
    for name in names {
        assert!(rows.iter().any(|row| row["name"] == name));
    }
}

#[test]
fn query_validation_errors() {
    let profile = support::profile(support::RECURSION);
    let foo = support::frame(&profile, "foo");
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
fn regex_syntax_and_unicode() {
    let profile = support::profile("root;alpha 3\nroot;alps 2\nroot;函数 1\n");
    for (pattern, expected) in [
        (r"^a\p{Latin}+$", vec!["alpha", "alps"]),
        (r"\p{Han}", vec!["函数"]),
        (r"(?i)^ALPHA$", vec!["alpha"]),
        (r"^a(?:lpha|lps)$", vec!["alpha", "alps"]),
    ] {
        let result = query::find_symbols(&profile, pattern, MatchMode::Regex, 20).unwrap();
        let names: Vec<_> = result["data"]["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, expected, "{pattern}");
    }
}

#[test]
fn summary_names_are_opaque() {
    for name in [
        "ordinary",
        "[unknown]",
        "[unknown@0x7] [/lib/a.so]",
        "%5Bunknown%5D",
    ] {
        let profile = support::profile(&format!("root;{name} 7\n"));
        let summary = query::summary(&profile);
        let data = &summary["data"];
        assert!(data.get("unknown_frame_weight").is_none());
        assert_eq!(data["total_weight"], 7);
        assert_eq!(data["frame_count"], 2);
        assert_eq!(data["top_self"][0]["name"], name);
        assert_eq!(data["top_self"][0]["self_weight"], 7);
        assert_eq!(summary["warnings"].as_array().unwrap().len(), 1);
    }
}

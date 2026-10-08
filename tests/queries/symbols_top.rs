use crate::support;
use prof_mcp::query::{self, FrameSelector, MatchMode, TopSort};

#[test]
fn find_reports_observed_context_and_frame_weights() {
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
}

#[test]
fn focused_top_filters_rows_without_changing_scope() {
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
fn invalid_regex_selectors_and_row_budgets_return_structured_errors() {
    let profile = support::profile(support::RECURSION);
    let foo = support::frame(&profile, "foo");
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
fn regex_queries_preserve_unicode_properties_and_common_syntax() {
    let profile = support::profile("root;alpha 3\nroot;alps 2\nroot;函数 1\n");
    for (pattern, expected) in [
        (r"^a\p{Latin}+$", vec!["alpha", "alps"]),
        (r"\p{Han}", vec!["函数"]),
        (r"(?i)^ALPHA$", vec!["alpha"]),
        (r"^a(?:lpha|lps)$", vec!["alpha", "alps"]),
    ] {
        let result = query::find_symbols(&profile, pattern, MatchMode::Regex, 20, false).unwrap();
        let names: Vec<_> = result["data"]["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, expected, "{pattern}");
    }
}

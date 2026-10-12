use crate::support;
use prof_mcp::query;

#[test]
fn tree_pruning_and_continuations() {
    let profile = support::profile("A 5\nB 4\nC 1\n");
    let one = support::json(query::tree(&profile, [0, 0], None, 0, 1, 0.0).unwrap());
    assert!(one["truncated"].as_bool().unwrap());
    assert_eq!(one["data"]["root"]["omitted_children"], 3);
    assert_eq!(one["truncation_reasons"][0]["kind"], "depth_limit");
    assert_eq!(one["truncation_reasons"][0]["omitted_children"], 3);
    assert_eq!(one["truncation_reasons"][0]["omitted_weight"], 10);
    assert_eq!(one["data"]["continuations"].as_array().unwrap().len(), 3);
    assert_eq!(one["data"]["continuations_available"], 3);
    assert_eq!(one["data"]["continuations_omitted"], 0);
    assert_eq!(
        one["data"]["continuations"][0]["continuation"]["fingerprint"],
        profile.fingerprint()
    );
    let continuation: [u32; 2] =
        serde_json::from_value(one["data"]["continuations"][0]["node_id"].clone()).unwrap();
    let continuation_page = support::json(
        query::tree(
            &profile,
            continuation,
            Some(profile.fingerprint()),
            4,
            64,
            0.0,
        )
        .unwrap(),
    );
    assert_eq!(
        continuation_page["data"]["root"]["node_id"],
        serde_json::json!(continuation)
    );
    let threshold = support::json(query::tree(&profile, [0, 0], None, 4, 64, 20.0).unwrap());
    assert_eq!(threshold["data"]["root"]["omitted_children"], 1);
    assert_eq!(
        threshold["truncation_reasons"][0]["kind"],
        "min_scope_percent"
    );
    let tree = support::json(query::tree(&profile, [0, 0], None, 4, 2, 0.0).unwrap());
    assert_eq!(tree["truncation_reasons"][0]["kind"], "node_budget");
    let child_id: [u32; 2] =
        serde_json::from_value(tree["data"]["root"]["children"][0]["node_id"].clone()).unwrap();
    let error = query::tree(&profile, child_id, None, 4, 64, 0.0).unwrap_err();
    assert_eq!(error.code, "profile_changed");
    let page = support::json(
        query::tree(&profile, child_id, Some(profile.fingerprint()), 4, 64, 0.0).unwrap(),
    );
    assert_eq!(page["scope_weight"], 5);
    let repeat = support::json(query::tree(&profile, [0, 0], None, 4, 2, 0.0).unwrap());
    assert_eq!(tree, repeat);
}

#[test]
fn tree_construction_bounds_long_names_in_nodes_and_continuations() {
    let profile = support::profile(&format!("{} 1\n", "多".repeat(24_000)));
    for depth in [0, 1] {
        let error = query::tree(&profile, [0, 0], None, depth, 2, 0.0).unwrap_err();
        assert_eq!(error.code, "query_too_large");
        assert_eq!(error.details["resource"], "response_bytes");
    }
}

#[test]
fn mixed_tree_truncations_keep_fields_and_reason_order() {
    let profile = support::profile("A;leafA 10\nB;leafB 3\nC 1\n");
    let tree = support::json(query::tree(&profile, [0, 0], None, 1, 2, 10.0).unwrap());
    assert_eq!(
        tree["truncation_reasons"],
        serde_json::json!([
            {"kind":"depth_limit","max_depth":1,"omitted_children":1,"omitted_weight":10},
            {"kind":"min_scope_percent","threshold":10.0,"omitted_children":1,"omitted_weight":1},
            {"kind":"node_budget","max_nodes":2,"omitted_children":1,"omitted_weight":3}
        ])
    );
    assert_eq!(tree["data"]["continuations"][0]["reason"], "depth_limit");
}

#[test]
fn compressed_labels_preserve_logical_budgets_and_continuations() {
    let profile = support::profile("a;b;c 5\na;b;d 7\n");
    assert_eq!(profile.cct().nodes().len(), 4);
    let page = support::json(query::tree(&profile, [0, 0], None, 1, 2, 0.0).unwrap());
    let a = &page["data"]["root"]["children"][0];
    assert_eq!(a["name"], "a");
    assert_eq!(a["self_weight"], 0);
    assert_eq!(a["total_weight"], 12);
    assert_eq!(a["omitted_children"], 1);
    assert_eq!(a["omitted_weight"], 12);
    let continuation = &page["data"]["continuations"][0];
    let id: [u32; 2] = serde_json::from_value(continuation["node_id"].clone()).unwrap();
    assert_eq!(id[1], 1);
    assert_eq!(continuation["name"], "b");
    let resumed = query::tree(&profile, id, Some(profile.fingerprint()), 1, 3, 0.0).unwrap();
    assert_eq!(resumed.data.root.name, Some("b"));
    assert_eq!(resumed.data.root.children[0].name, Some("d"));
    assert_eq!(resumed.data.root.children[1].name, Some("c"));
    for invalid in [[0, 1], [id[0], 2], [u32::MAX, 0]] {
        assert_eq!(
            query::tree(&profile, invalid, Some(profile.fingerprint()), 1, 3, 0.0)
                .unwrap_err()
                .code,
            "invalid_node_id"
        );
    }
    assert_eq!(
        query::tree(&profile, id, Some("wrong"), 1, 3, 0.0)
            .unwrap_err()
            .code,
        "profile_changed"
    );
}

#[test]
fn compressed_tree_keeps_terminal_prefixes_and_first_frame_ties() {
    let profile = support::profile("a;b;c 5\na 3\nd;e;z 5\nx;y;a 5\n");
    let full = query::tree(&profile, [0, 0], None, 8, 32, 0.0).unwrap();
    let root = &full.data.root;
    assert_eq!(
        root.children
            .iter()
            .map(|n| n.name.unwrap())
            .collect::<Vec<_>>(),
        ["a", "d", "x"]
    );
    assert_eq!(root.children[0].self_weight, 3);
    assert_eq!(root.children[0].children[0].name, Some("b"));
    assert_eq!(root.children[0].children[0].self_weight, 0);
}

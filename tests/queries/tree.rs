use crate::support;
use prof_mcp::query;

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

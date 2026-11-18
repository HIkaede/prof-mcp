mod support;
use prof_mcp::query::summary;

#[test]
fn distinguish_unknown_leaf_from_unknown_stack_coverage() {
    let profile = support::profile(
        "r;[unknown] 20\nr;[unknown];[unknown];x 30\nr;[unknown@0xAf] [m];[unknown] 5\nr;[unknown@interface] 45\n",
    );
    let result = support::json(summary(&profile).unwrap());
    let symbols = &result["data"]["symbol_coverage"];
    assert_eq!(symbols["unresolved_frames"], 2);
    assert_eq!(symbols["unresolved_self_weight"], 25);
    assert_eq!(symbols["unresolved_self_percent"], 25.0);
    assert_eq!(symbols["unresolved_stack_weight"], 55);
    assert_eq!(symbols["unresolved_stack_percent"], 55.0);
    assert_eq!(
        result["data"]["capabilities"]["storage"],
        "folded_aggregate"
    );
    assert_eq!(
        result["data"]["quality"]["lost_samples"],
        serde_json::Value::Null
    );
}

#[test]
fn empty_folded_profile_has_no_measured_share() {
    let profile = support::profile("");
    let result = support::json(summary(&profile).unwrap());
    assert_eq!(
        result["data"]["symbol_coverage"]["unresolved_stack_weight"],
        0
    );
    assert!(result["data"]["symbol_coverage"]["unresolved_stack_percent"].is_null());
}

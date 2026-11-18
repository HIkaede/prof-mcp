mod support;

use prof_mcp::query::comparison::{CompareRequest, compare};
use serde_json::json;

fn run(a: &str, b: &str, extra: serde_json::Value) -> serde_json::Value {
    let mut request = json!({"baseline":{"profile":"a"}, "candidate":{"profile":"b"}});
    for (key, value) in extra.as_object().unwrap() {
        request[key] = value.clone();
    }
    let request: CompareRequest = serde_json::from_value(request).unwrap();
    let a = support::profile(a);
    let b = support::profile(b);
    serde_json::to_value(compare(&a, &b, &request).unwrap()).unwrap()
}

#[test]
fn compare_names_presence_and_denominators() {
    let a = "root;old 10\nroot;common 30\n";
    let b = "root;new 40\nroot;common 40\n";
    let forward = run(a, b, json!({}));
    let reverse = run(b, a, json!({}));
    assert_eq!(forward["baseline"]["scope_weight"], 40);
    assert_eq!(forward["candidate"]["scope_weight"], 80);
    assert_eq!(forward["matched_rows"], 4);
    let find = |result: &serde_json::Value, name: &str| {
        result["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == name)
            .unwrap()
            .clone()
    };
    let new = find(&forward, "new");
    assert_eq!(new["baseline_present"], false);
    assert_eq!(new["candidate_present"], true);
    assert!(new["relative_weight_change_percent"].is_null());
    let root = find(&forward, "root");
    assert_eq!(root["baseline_present"], true);
    assert_eq!(root["baseline_weight"], 0);
    assert_eq!(find(&reverse, "new")["delta_pp"], -50.0);
    assert_eq!(find(&forward, "common")["delta_pp"], -25.0);
    assert_eq!(run(a, a, json!({"limit":1}))["rows"][0]["delta_pp"], 0.0);
    assert!(run("", b, json!({}))["rows"][0]["delta_pp"].is_null());
}

#[test]
fn unsupported_metric_and_alignment_are_rejected() {
    let p = support::profile("root 1\n");
    let mut req =
        json!({"baseline":{"profile":"a"},"candidate":{"profile":"b"},"metric":"sample_period"});
    let typed = serde_json::from_value(req.clone()).unwrap();
    assert_eq!(
        compare(&p, &p, &typed).unwrap_err().code,
        "capability_unavailable"
    );
    req["identity_policy"] = json!("numeric_id");
    assert!(serde_json::from_value::<CompareRequest>(req).is_err());
}

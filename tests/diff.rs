mod support;

use rmdb_prof_mcp::query::{self, DiffSort, TopSort};

#[test]
fn diff_joins_exact_names_across_different_frame_ids_and_sorts() {
    let baseline = support::profile("root;only_base 10\nroot;shared 90\n");
    let candidate = support::profile("root;candidate_first 20\nroot;shared 80\n");
    let regression = query::diff(
        &baseline,
        &candidate,
        TopSort::SelfWeight,
        DiffSort::Regression,
        30,
        None,
    )
    .unwrap();
    assert_eq!(regression["data"]["rows"][0]["name"], "candidate_first");
    assert_eq!(regression["data"]["rows"][0]["delta_pp"], 20.0);
    let improvement = query::diff(
        &baseline,
        &candidate,
        TopSort::SelfWeight,
        DiffSort::Improvement,
        30,
        None,
    )
    .unwrap();
    assert_eq!(improvement["data"]["rows"][0]["name"], "only_base");
    let absolute = query::diff(
        &baseline,
        &candidate,
        TopSort::SelfWeight,
        DiffSort::Absolute,
        30,
        None,
    )
    .unwrap();
    assert_eq!(absolute["scope_weight"]["baseline"], 100);
    assert_eq!(absolute["scope_weight"]["candidate"], 100);
    let shared = absolute["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "shared")
        .unwrap();
    assert_eq!(shared["baseline_weight"], 90);
    assert_eq!(shared["candidate_weight"], 80);
}

#[test]
fn equal_percentages_with_different_raw_totals_keep_raw_weights() {
    let baseline = support::profile("root;hot 50\nroot;cold 50\n");
    let candidate = support::profile("root;hot 100\nroot;cold 100\n");
    let result = query::diff(
        &baseline,
        &candidate,
        TopSort::SelfWeight,
        DiffSort::Absolute,
        30,
        None,
    )
    .unwrap();
    let hot = result["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "hot")
        .unwrap();
    assert_eq!(hot["baseline_weight"], 50);
    assert_eq!(hot["candidate_weight"], 100);
    assert_eq!(hot["delta_pp"], 0.0);
}

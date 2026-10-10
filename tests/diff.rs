mod support;

use prof_mcp::query::{self, DiffSort, TopSort};

#[test]
fn diff_by_exact_name() {
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
    let limited = query::diff(
        &baseline,
        &candidate,
        TopSort::SelfWeight,
        DiffSort::Regression,
        1,
        None,
    )
    .unwrap();
    assert_eq!(limited["truncation_reasons"][0]["kind"], "row_limit");
    assert_eq!(limited["truncation_reasons"][0]["available"], 4);
}

#[test]
fn diff_preserves_raw_weights() {
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

#[test]
fn diff_totals_are_descriptive() {
    let baseline = support::profile("root;hot 100\n");
    for weight in [1, 80, 100, 125, 250, 10000] {
        let candidate = support::profile(&format!("root;hot {weight}\n"));
        let result = query::diff(
            &baseline,
            &candidate,
            TopSort::SelfWeight,
            DiffSort::Absolute,
            10,
            None,
        )
        .unwrap();
        assert!(result["data"].get("total_weight_ratio").is_none());
        assert_eq!(result["warnings"].as_array().unwrap().len(), 1);
        assert_eq!(result["data"]["rows"][0]["delta_pp"], 0.0);
        assert_eq!(result["scope_weight"]["candidate"], weight);
    }
}

#[test]
fn bounded_diff_keeps_order() {
    let baseline = support::profile("root;alpha 10\nroot;beta 20\nroot;gamma 70\n");
    let candidate = support::profile("root;alpha 20\nroot;beta 10\nroot;delta 70\n");
    for metric in [TopSort::SelfWeight, TopSort::Inclusive] {
        for (sort, names) in [
            (
                DiffSort::Regression,
                ["delta", "alpha", "root", "beta", "gamma"],
            ),
            (
                DiffSort::Improvement,
                ["gamma", "beta", "root", "alpha", "delta"],
            ),
            (
                DiffSort::Absolute,
                ["delta", "gamma", "alpha", "beta", "root"],
            ),
        ] {
            for regex in [None, Some("^(alpha|beta|delta|gamma)$")] {
                let expected: Vec<_> = names
                    .into_iter()
                    .filter(|name| regex.is_none() || *name != "root")
                    .collect();
                for limit in 1..=5 {
                    let result =
                        query::diff(&baseline, &candidate, metric, sort, limit, regex).unwrap();
                    let rows = result["data"]["rows"].as_array().unwrap();
                    let actual: Vec<_> = rows
                        .iter()
                        .map(|row| row["name"].as_str().unwrap())
                        .collect();
                    assert_eq!(actual, expected[..limit.min(expected.len())]);
                    assert_eq!(result["truncated"], expected.len() > limit);
                    if expected.len() > limit {
                        assert_eq!(result["truncation_reasons"][0]["available"], expected.len());
                    }
                    for row in rows {
                        let (bw, cw) = match row["name"].as_str().unwrap() {
                            "alpha" => (10, 20),
                            "beta" => (20, 10),
                            "gamma" => (70, 0),
                            "delta" => (0, 70),
                            "root" => match metric {
                                TopSort::SelfWeight => (0, 0),
                                TopSort::Inclusive => (100, 100),
                            },
                            _ => unreachable!(),
                        };
                        assert_eq!(
                            *row,
                            serde_json::json!({
                                "name": row["name"], "baseline_weight": bw, "candidate_weight": cw,
                                "baseline_percent": bw as f64, "candidate_percent": cw as f64,
                                "delta_pp": (cw - bw) as f64
                            })
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn bounded_diff_matches_unbounded_reference_for_unicode_ties() {
    let names = [
        "alpha", "βeta", "中", "delta", "éclair", "gamma", "λ", "omega", "zeta", "尾", "a", "a2",
        "foo",
    ];
    let baseline_input: String = names
        .iter()
        .enumerate()
        .map(|(index, name)| format!("root;{name} {}\n", 10 + (index % 4) * 10))
        .collect();
    let candidate_input: String = names
        .iter()
        .enumerate()
        .map(|(index, name)| format!("root;{name} {}\n", 40 - (index % 4) * 10))
        .collect();
    let baseline = support::profile(&baseline_input);
    let candidate = support::profile(&candidate_input);
    for metric in [TopSort::SelfWeight, TopSort::Inclusive] {
        for sort in [
            DiffSort::Regression,
            DiffSort::Improvement,
            DiffSort::Absolute,
        ] {
            for regex in [None, Some("^(alpha|βeta|中|éclair|尾)$")] {
                let reference =
                    query::diff(&baseline, &candidate, metric, sort, 200, regex).unwrap();
                let available = reference["data"]["rows"].as_array().unwrap().len();
                for limit in [1, 3, 7, 20] {
                    let bounded =
                        query::diff(&baseline, &candidate, metric, sort, limit, regex).unwrap();
                    assert_eq!(
                        bounded["data"]["rows"],
                        serde_json::Value::Array(
                            reference["data"]["rows"].as_array().unwrap()[..limit.min(available)]
                                .to_vec()
                        )
                    );
                    assert_eq!(bounded["truncated"], available > limit);
                    if available > limit {
                        assert_eq!(
                            bounded["truncation_reasons"][0]["available"],
                            serde_json::json!(available)
                        );
                    }
                }
            }
        }
    }
}

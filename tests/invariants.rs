mod support;

use prof_mcp::profile::Profile;

fn assert_invariants(profile: &Profile) {
    assert_eq!(profile.root().total_weight, profile.total_weight);
    for node in &profile.cct.nodes {
        assert_eq!(
            node.total_weight,
            node.self_weight
                + node
                    .children
                    .values()
                    .map(|id| profile.cct.nodes[*id as usize].total_weight)
                    .sum::<u64>()
        );
    }
    assert_eq!(
        profile
            .frame_stats
            .iter()
            .map(|stats| stats.self_weight)
            .sum::<u64>(),
        profile.total_weight
    );
    for stats in &profile.frame_stats {
        assert!(
            stats.self_weight <= stats.inclusive_weight
                && stats.inclusive_weight <= profile.total_weight
        );
    }
    for (frame, ids) in profile.frame_to_stacks.iter().enumerate() {
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), ids.len());
        for id in ids {
            assert!(
                profile.stacks[*id as usize]
                    .frames
                    .contains(&(frame as u32))
            );
        }
    }
}

#[test]
fn cct_and_frame_index_invariants_hold() {
    assert_invariants(&support::profile(
        "root;A;B 30\nroot;A;C 20\nroot;A 5\nroot;foo;foo;bar 10\n",
    ));
}

// IDs and fingerprints describe each input's representation. Compare observed
// statistics by symbol name when byte order or duplicate-line layout changes.
fn stats_by_name(profile: &Profile) -> std::collections::BTreeMap<&str, (u64, u64, u32)> {
    profile
        .frames
        .iter()
        .zip(&profile.frame_stats)
        .map(|(frame, stats)| {
            (
                frame.name.as_ref(),
                (stats.self_weight, stats.inclusive_weight, stats.stack_count),
            )
        })
        .collect()
}

#[test]
fn permutations_and_split_weights_preserve_statistics_rankings_and_zero_diff() {
    use prof_mcp::query::{self, DiffSort, FrameSelector, FrameWindow, TopSort};
    let paths = ["root;A;leaf", "root;B;leaf", "root;A;A"];
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    for a in 1..=2 {
        for b in 1..=2 {
            for c in 1..=2 {
                let weights = [a, b, c];
                let merged = paths
                    .iter()
                    .zip(weights)
                    .map(|(p, w)| format!("{p} {}\n", 2 * w))
                    .collect::<String>();
                let baseline = support::profile(&merged);
                for order in orders {
                    let split = order
                        .iter()
                        .map(|&i| {
                            format!("{} {}\n{} {}\n", paths[i], weights[i], paths[i], weights[i])
                        })
                        .collect::<String>();
                    let candidate = support::profile(&split);
                    assert_invariants(&candidate);
                    assert_eq!(baseline.total_weight, candidate.total_weight);
                    assert_eq!(stats_by_name(&baseline), stats_by_name(&candidate));
                    for metric in [TopSort::SelfWeight, TopSort::Inclusive] {
                        let top = query::top(&candidate, metric, 20, None, None, false).unwrap();
                        assert_eq!(
                            top,
                            query::top(&candidate, metric, 20, None, None, false).unwrap()
                        );
                        let ranked_names = |p: &Profile| {
                            query::top(p, metric, 20, None, None, false).unwrap()["data"]["rows"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|row| row["name"].clone())
                                .collect::<Vec<_>>()
                        };
                        assert_eq!(ranked_names(&baseline), ranked_names(&candidate));
                        for other in [&baseline, &candidate] {
                            let diff = query::diff(
                                &candidate,
                                other,
                                metric,
                                DiffSort::Absolute,
                                20,
                                None,
                            )
                            .unwrap();
                            let rows = diff["data"]["rows"].as_array().unwrap();
                            assert_eq!(rows.len(), candidate.frames.len());
                            for row in rows {
                                assert_eq!(row["delta_pp"], 0.0);
                                assert_eq!(row["baseline_weight"], row["candidate_weight"]);
                            }
                        }
                    }
                    let selector = FrameSelector {
                        frame_name: Some("A".into()),
                        frame_id: None,
                    };
                    let full = query::paths(&candidate, &selector, 20).unwrap();
                    for window in [
                        FrameWindow::Head { lines: 1 },
                        FrameWindow::Tail { lines: 1 },
                        FrameWindow::AroundTarget {
                            before: 0,
                            after: 1,
                        },
                    ] {
                        let windowed =
                            query::paths_with_window(&candidate, &selector, 20, Some(window))
                                .unwrap();
                        assert_eq!(full["scope_weight"], windowed["scope_weight"]);
                        let identity = |v: &serde_json::Value| {
                            v["data"]["paths"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|row| {
                                    (
                                        row["weight"].clone(),
                                        row["target_positions"].clone(),
                                        row["scope_percent"].clone(),
                                    )
                                })
                                .collect::<Vec<_>>()
                        };
                        assert_eq!(identity(&full), identity(&windowed));
                    }
                }
            }
        }
    }
}

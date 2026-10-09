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
fn equivalent_folded_inputs() {
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
                        let top = query::top(&candidate, metric, 20, None, None).unwrap();
                        assert_eq!(top, query::top(&candidate, metric, 20, None, None).unwrap());
                        let ranked_names = |p: &Profile| {
                            query::top(p, metric, 20, None, None).unwrap()["data"]["rows"]
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

fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
    *state >> 32
}

fn strip_scope(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            object.remove("scope_percent");
            for child in object.values_mut() {
                strip_scope(child);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                strip_scope(child);
            }
        }
        _ => {}
    }
}

fn find_node(value: &serde_json::Value, id: u64) -> Option<&serde_json::Value> {
    if value["node_id"] == id {
        return Some(value);
    }
    value["children"]
        .as_array()?
        .iter()
        .find_map(|child| find_node(child, id))
}

#[test]
fn generated_stack_algebra() {
    use prof_mcp::query::{self, DiffSort, FrameSelector, MatchMode, TopSort};
    for seed in 0..128 {
        let mut state = seed;
        let names = ["A", "B", "foo<int>", "foo<double>", "函数", "operator<<"];
        let mut stacks = vec![("root;A;A".to_owned(), 10)];
        for _ in 0..24 {
            let mut frames = vec!["root"];
            for _ in 0..1 + next(&mut state) % 6 {
                frames.push(names[next(&mut state) as usize % names.len()]);
            }
            stacks.push((frames.join(";"), 2 + next(&mut state) % 100));
        }
        let input = stacks
            .iter()
            .map(|(s, w)| format!("{s} {w}\n"))
            .collect::<String>();
        for i in (1..stacks.len()).rev() {
            let j = next(&mut state) as usize % (i + 1);
            stacks.swap(i, j);
        }
        let split = stacks
            .iter()
            .map(|(s, w)| format!("{s} 1\n{s} {}\n", w - 1))
            .collect::<String>();
        let baseline = support::profile(&input);
        let candidate = support::profile(&split);
        assert_invariants(&baseline);
        assert_invariants(&candidate);
        assert_eq!(
            stats_by_name(&baseline),
            stats_by_name(&candidate),
            "seed={seed}"
        );
        for metric in [TopSort::SelfWeight, TopSort::Inclusive] {
            let diff =
                query::diff(&baseline, &candidate, metric, DiffSort::Absolute, 200, None).unwrap();
            assert!(
                diff["data"]["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|row| row["delta_pp"] == 0.0)
            );
            let ranked = |p: &Profile| {
                let top = query::top(p, metric, 200, None, None).unwrap();
                assert_eq!(top, query::top(p, metric, 200, None, None).unwrap());
                top["data"]["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| {
                        (
                            row["name"].clone(),
                            row["self_weight"].clone(),
                            row["inclusive_weight"].clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(ranked(&baseline), ranked(&candidate));
        }
        for name in names {
            if baseline.frame_id(name).is_none() {
                continue;
            }
            let selector = FrameSelector {
                frame_name: Some(name.into()),
                frame_id: None,
            };
            let found = query::find_symbols(&baseline, name, MatchMode::Contains, 100).unwrap();
            assert_eq!(
                found,
                query::find_symbols(&baseline, name, MatchMode::Contains, 100).unwrap()
            );
            let paths = query::paths(&baseline, &selector, 50).unwrap();
            assert_eq!(paths, query::paths(&baseline, &selector, 50).unwrap());
            assert_eq!(
                paths["data"]["paths"],
                query::paths(&candidate, &selector, 50).unwrap()["data"]["paths"]
            );
            for row in paths["data"]["paths"].as_array().unwrap() {
                assert!(
                    row["frames"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|frame| frame == name)
                );
            }
            for callers in [true, false] {
                let direction = if callers {
                    query::callers
                } else {
                    query::callees
                };
                let full = direction(&baseline, &selector, 16, 512, 0.0, None).unwrap();
                let page = direction(&baseline, &selector, 1, 4, 0.0, None).unwrap();
                assert_eq!(
                    page,
                    direction(&baseline, &selector, 1, 4, 0.0, None).unwrap()
                );
                for continuation in page["data"]["continuations"].as_array().unwrap() {
                    let path = continuation["node_path"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|id| id.as_u64().unwrap() as u32)
                        .collect::<Vec<_>>();
                    let resumed = direction(
                        &baseline,
                        &selector,
                        16,
                        512,
                        0.0,
                        Some((&path, &baseline.source.fingerprint)),
                    )
                    .unwrap();
                    let mut expected = &full["data"]["root"];
                    for id in &path {
                        expected = expected["children"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|node| node["frame_id"] == *id)
                            .unwrap();
                    }
                    let mut expected = expected.clone();
                    let mut actual = resumed["data"]["root"].clone();
                    strip_scope(&mut expected);
                    strip_scope(&mut actual);
                    assert_eq!(expected, actual, "seed={seed}, callers={callers}");
                }
            }
        }
        let full = query::tree(&baseline, 0, None, 16, 512, 0.0).unwrap();
        let page = query::tree(&baseline, 0, None, 2, 8, 0.0).unwrap();
        assert_eq!(page, query::tree(&baseline, 0, None, 2, 8, 0.0).unwrap());
        for continuation in page["data"]["continuations"].as_array().unwrap() {
            let id = continuation["node_id"].as_u64().unwrap();
            let resumed = query::tree(
                &baseline,
                id as u32,
                Some(&baseline.source.fingerprint),
                16,
                512,
                0.0,
            )
            .unwrap();
            let mut expected = find_node(&full["data"]["root"], id).unwrap().clone();
            let mut actual = resumed["data"]["root"].clone();
            strip_scope(&mut expected);
            strip_scope(&mut actual);
            assert_eq!(expected, actual, "seed={seed}");
        }
    }
}

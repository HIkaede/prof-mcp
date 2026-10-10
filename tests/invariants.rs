mod support;

use prof_mcp::profile::Profile;

fn assert_invariants(profile: &Profile) {
    assert_eq!(profile.frames.name_offsets.len(), profile.frames.len() + 1);
    assert_eq!(profile.frames.name_offsets[0], 0);
    assert_eq!(
        *profile.frames.name_offsets.last().unwrap() as usize,
        profile.frames.name_text.len()
    );
    let mut ordered_names = profile.frames.iter().collect::<Vec<_>>();
    ordered_names.sort_unstable();
    assert_eq!(
        profile
            .frames
            .name_order
            .iter()
            .map(|id| profile.frame_name(*id))
            .collect::<Vec<_>>(),
        ordered_names
    );
    for (id, name) in profile.frames.iter().enumerate() {
        assert_eq!(profile.frame_id(name), Some(id as u32));
    }
    assert_eq!(profile.frame_id("missing-frame"), None);
    for (order, inclusive) in [(&profile.top_self, false), (&profile.top_inclusive, true)] {
        let mut expected: Vec<_> = (0..profile.frames.len() as u32).collect();
        expected.sort_unstable_by(|a, b| {
            let an = &profile.frame_stats[*a as usize];
            let bn = &profile.frame_stats[*b as usize];
            let metric = |stats: &prof_mcp::profile::FrameStats| {
                if inclusive {
                    stats.inclusive_weight
                } else {
                    stats.self_weight
                }
            };
            metric(bn)
                .cmp(&metric(an))
                .then_with(|| bn.self_weight.cmp(&an.self_weight))
                .then_with(|| profile.frame_name(*a).cmp(profile.frame_name(*b)))
                .then(a.cmp(b))
        });
        assert_eq!(order.as_ref(), expected);
    }
    assert_eq!(profile.stacks.offsets.len(), profile.stacks.len() + 1);
    assert_eq!(profile.stacks.offsets[0], 0);
    assert_eq!(
        *profile.stacks.offsets.last().unwrap() as usize,
        profile.stacks.frames.len()
    );
    assert!(
        profile
            .stacks
            .offsets
            .windows(2)
            .all(|range| range[0] < range[1])
    );
    assert_eq!(
        profile.stacks.weights.iter().sum::<u64>(),
        profile.total_weight
    );
    assert_eq!(
        profile.frame_to_stacks.offsets.len(),
        profile.frames.len() + 1
    );
    assert_eq!(profile.frame_to_stacks.offsets[0], 0);
    assert_eq!(
        *profile.frame_to_stacks.offsets.last().unwrap() as usize,
        profile.frame_to_stacks.stack_ids.len()
    );
    assert_eq!(profile.cct.child_offsets.len(), profile.cct.nodes.len() + 1);
    assert_eq!(profile.cct.child_offsets[0], 0);
    assert_eq!(profile.cct.children.len(), profile.cct.nodes.len() - 1);
    assert_eq!(
        *profile.cct.child_offsets.last().unwrap() as usize,
        profile.cct.children.len()
    );
    assert_eq!(profile.root().total_weight, profile.total_weight);
    for (id, node) in profile.cct.nodes.iter().enumerate() {
        assert_eq!(
            node.total_weight,
            node.self_weight
                + profile
                    .cct
                    .children(id as u32)
                    .iter()
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
        let mut unique = ids.to_vec();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), ids.len());
        let expected: Vec<_> = profile
            .stacks
            .iter()
            .enumerate()
            .filter(|(_, stack)| stack.frames.contains(&(frame as u32)))
            .map(|(id, _)| id as u32)
            .collect();
        assert_eq!(ids, expected);
        let stats = &profile.frame_stats[frame];
        assert_eq!(stats.stack_count as usize, ids.len());
        assert_eq!(
            stats.inclusive_weight,
            ids.iter()
                .map(|id| profile.stacks.stack(*id).weight)
                .sum::<u64>()
        );
        for id in ids {
            assert!(profile.stacks.stack(*id).frames.contains(&(frame as u32)));
        }
    }
    assert_reference_cct(profile);
}

fn assert_reference_cct(profile: &Profile) {
    #[derive(Default)]
    struct Node {
        frame: Option<u32>,
        self_weight: u64,
        total_weight: u64,
        children: std::collections::BTreeMap<u32, u32>,
    }
    let mut nodes = vec![Node::default()];
    for stack in profile.stacks.iter() {
        let mut node = 0;
        nodes[node].total_weight += stack.weight;
        for &frame in stack.frames {
            let child = match nodes[node].children.get(&frame) {
                Some(&id) => id,
                None => {
                    let id = nodes.len() as u32;
                    nodes.push(Node {
                        frame: Some(frame),
                        ..Node::default()
                    });
                    nodes[node].children.insert(frame, id);
                    id
                }
            };
            node = child as usize;
            nodes[node].total_weight += stack.weight;
        }
        nodes[node].self_weight += stack.weight;
    }
    assert_eq!(nodes.len(), profile.cct.nodes.len());
    for (id, expected) in nodes.iter().enumerate() {
        let actual = &profile.cct.nodes[id];
        assert_eq!(
            (actual.frame, actual.self_weight, actual.total_weight),
            (expected.frame, expected.self_weight, expected.total_weight)
        );
        let mut children: Vec<_> = expected.children.values().copied().collect();
        children.sort_by(|a, b| {
            let an = &nodes[*a as usize];
            let bn = &nodes[*b as usize];
            bn.total_weight
                .cmp(&an.total_weight)
                .then_with(|| {
                    profile
                        .frame_name(an.frame.unwrap())
                        .cmp(profile.frame_name(bn.frame.unwrap()))
                })
                .then(a.cmp(b))
        });
        assert_eq!(profile.cct.children(id as u32), children);
    }
}

#[test]
fn compact_layout_handles_empty_deep_and_prefix_stacks() {
    assert_invariants(&support::profile(""));
    let deep = vec!["recursive"; 4096].join(";");
    let profile = support::profile(&format!("{deep} 7\nrecursive 3\nrecursive;branch 7\n"));
    assert_invariants(&profile);
    assert_eq!(profile.max_depth, 4096);
    assert_eq!(profile.stacks.stack(0).frames.len(), 1);
    assert_eq!(profile.recursive_frames[0].max_occurrences, 4096);
    assert_eq!(profile.recursive_frames[0].affected_weight, 7);
}

#[test]
fn cct_and_frame_index_invariants_hold() {
    assert_invariants(&support::profile(
        "root;A;B 30\nroot;A;C 20\nroot;A 5\nroot;foo;foo;bar 10\n",
    ));
}

#[test]
fn lexical_names_preserve_unicode_and_weight_ties() {
    assert_invariants(&support::profile(
        "根;é;z 7\n根;a;中 7\n根;é;é 7\n根;a;a 7\n根;z 14\n",
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
                frame,
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

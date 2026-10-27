use super::*;
use crate::profile::{BuildLimits, ProfileBuilder};
use std::{io::Cursor, path::PathBuf};
fn profile(input: &str) -> Profile {
    ProfileBuilder::new(BuildLimits::default())
        .from_reader(Cursor::new(input), PathBuf::from("direction.folded"))
        .unwrap()
}
fn next(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    *seed >> 32
}
#[test]
fn demand_direction_matches_temp_tree_oracle() {
    for original_seed in 0..64 {
        let mut seed = original_seed;
        let mut input = "root;a;a;b 3\nroot;a 2\nroot;b;a;b;a;a 5\n".to_owned();
        let names = ["a", "b", "c", "函数", "foo<int>"];
        for _ in 0..32 {
            let mut frames = vec!["root"];
            for _ in 0..1 + next(&mut seed) % 8 {
                frames.push(names[next(&mut seed) as usize % names.len()]);
            }
            input.push_str(&format!(
                "{} {}\n",
                frames.join(";"),
                1 + next(&mut seed) % 7
            ));
        }
        let p = profile(&input);
        for name in ["root", "a", "b"] {
            let selector = FrameSelector {
                frame_name: Some(name.into()),
                frame_id: None,
            };
            for callers in [true, false] {
                let query = if callers {
                    super::callers
                } else {
                    super::callees
                };
                let reference = if callers {
                    oracle::callers
                } else {
                    oracle::callees
                };
                for (depth, nodes, threshold) in [
                    (0, 1, 0.0),
                    (1, 2, 0.0),
                    (3, 4, 5.0),
                    (8, 200, 0.0),
                    (4, 12, 40.0),
                ] {
                    let expected = serde_json::to_value(
                        reference(&p, &selector, depth, nodes, threshold, None).unwrap(),
                    )
                    .unwrap();
                    let actual = query(&p, &selector, depth, nodes, threshold, None).unwrap();
                    assert_eq!(
                        serde_json::to_value(&actual).unwrap(),
                        expected,
                        "seed={original_seed}, name={name},callers={callers}"
                    );
                    for continuation in actual.data.continuations.iter().take(4) {
                        let path = &continuation.continuation.cursor[1..];
                        let cursor = Some((path, p.fingerprint()));
                        assert_eq!(
                            serde_json::to_value(query(&p, &selector, 2, 5, 5.0, cursor).unwrap())
                                .unwrap(),
                            serde_json::to_value(
                                reference(&p, &selector, 2, 5, 5.0, cursor).unwrap()
                            )
                            .unwrap()
                        );
                    }
                }
            }
        }
    }
}
#[test]
fn demand_direction_keeps_wide_first_level_guard() {
    let mut input = String::new();
    for child in 0..MAX_DIRECTION_NODES {
        input.push_str(&format!("root;child{child} 1\n"));
    }
    let p = profile(&input);
    let selector = FrameSelector {
        frame_name: Some("root".into()),
        frame_id: None,
    };
    let error = callees(&p, &selector, 0, 1, 0.0, None).unwrap_err();
    assert_eq!(error.code, "query_too_large");
    assert_eq!(error.details["resource"], "direction_nodes");
    assert_eq!(error.details["limit"], MAX_DIRECTION_NODES);
}
#[test]
fn demand_direction_preserves_shared_terminal_prefix_and_dfs_order() {
    let p = profile("root;a;one 4\nroot;a;two 3\nroot;b;x 6\nroot;c 1\n");
    let selector = FrameSelector {
        frame_name: Some("root".into()),
        frame_id: None,
    };
    let output = callees(&p, &selector, 4, 3, 0.0, None).unwrap();
    assert_eq!(output.data.root.children[0].name, Some("a"));
    assert_eq!(output.data.root.children[0].children[0].name, Some("one"));
    assert_eq!(output.data.root.omitted_children, 2);
    assert_eq!(output.data.root.omitted_weight, 7);
    assert_eq!(
        output
            .data
            .continuations
            .iter()
            .map(|c| c.name)
            .collect::<Vec<_>>(),
        ["two", "b", "c"]
    );
    for input in [
        "root;a 2\nroot;a;b;c 3\nroot;a;b;d 3\n",
        "root;a;b;c;d 1\nroot;a;b;c;e 1\n",
    ] {
        let p = profile(input);
        for (depth, nodes) in [(0, 1), (1, 2), (3, 4), (8, 64)] {
            assert_eq!(
                serde_json::to_value(callees(&p, &selector, depth, nodes, 0.0, None).unwrap())
                    .unwrap(),
                serde_json::to_value(
                    oracle::callees(&p, &selector, depth, nodes, 0.0, None).unwrap()
                )
                .unwrap()
            );
        }
    }
}

#[test]
fn shared_chain_full_match_continuations_keep_exact_scopes() {
    for input in [
        "root;a;b;focus;d 7\nroot;a;b;focus;e 5\n",
        "root;a;b;focus;d 7\nroot;z;focus;e 5\n",
    ] {
        let p = profile(input);
        let selector = FrameSelector {
            frame_name: Some("focus".into()),
            frame_id: None,
        };
        assert_eq!(
            p.postings()
                .constant_positions_pair(p.frame_id("root").unwrap() as usize),
            Some((0, 0))
        );
        for callers in [true, false] {
            let query = if callers {
                super::callers
            } else {
                super::callees
            };
            let reference = if callers {
                oracle::callers
            } else {
                oracle::callees
            };
            let page = query(&p, &selector, 0, 1, 0.0, None).unwrap();
            for continuation in &page.data.continuations {
                let cursor = Some((&continuation.continuation.cursor[1..], p.fingerprint()));
                assert_eq!(
                    serde_json::to_value(query(&p, &selector, 2, 3, 0.0, cursor).unwrap()).unwrap(),
                    serde_json::to_value(reference(&p, &selector, 2, 3, 0.0, cursor).unwrap())
                        .unwrap()
                );
            }
        }
    }
}

#[test]
fn deep_cursor_path_never_emits_unresumable_cursor() {
    // A 4096-id path is accepted on resume, but growing it into a cursor would
    // produce 4098 ids, which the server rejects, so no cursor may be emitted.
    let names = vec!["r"; 4096].join(";");
    for (input, callers) in [
        (format!("up2;up;{names};focus 7\n"), true),
        (format!("focus;{names};down;down2 7\n"), false),
    ] {
        let p = ProfileBuilder::new(BuildLimits {
            max_depth: 4099,
            ..BuildLimits::default()
        })
        .from_reader(Cursor::new(input), PathBuf::from("deep.folded"))
        .unwrap();
        let selector = FrameSelector {
            frame_name: Some("focus".into()),
            frame_id: None,
        };
        let path = vec![p.frame_id("r").unwrap(); 4096];
        let cursor = Some((path.as_slice(), p.fingerprint()));
        let output = if callers {
            super::callers(&p, &selector, 1, 1, 0.0, cursor)
        } else {
            super::callees(&p, &selector, 1, 1, 0.0, cursor)
        }
        .unwrap();
        assert!(output.data.continuations.is_empty());
        assert!(output.data.continuations_truncated);
        let reference = if callers {
            oracle::callers(&p, &selector, 1, 1, 0.0, cursor)
        } else {
            oracle::callees(&p, &selector, 1, 1, 0.0, cursor)
        }
        .unwrap();
        assert_eq!(
            serde_json::to_value(output).unwrap(),
            serde_json::to_value(reference).unwrap()
        );
    }
}

#[test]
fn direction_accepts_maximum_cursor_and_rejects_terminal_unknown_path() {
    let names = vec!["r"; 4096].join(";");
    for (input, callers) in [
        (format!("{names};focus 7\n"), true),
        (format!("focus;{names} 7\n"), false),
    ] {
        let p = ProfileBuilder::new(BuildLimits {
            max_depth: 4099,
            ..BuildLimits::default()
        })
        .from_reader(Cursor::new(input), PathBuf::from("deep.folded"))
        .unwrap();
        let selector = FrameSelector {
            frame_name: Some("focus".into()),
            frame_id: None,
        };
        let path = vec![p.frame_id("r").unwrap(); 4096];
        let query = if callers {
            super::callers
        } else {
            super::callees
        };
        let reference = if callers {
            oracle::callers
        } else {
            oracle::callees
        };
        let cursor = Some((path.as_slice(), p.fingerprint()));
        let output = query(&p, &selector, 1, 2, 0.0, cursor).unwrap();
        assert_eq!(output.data.root.self_weight, 7);
        assert_eq!(
            serde_json::to_value(output).unwrap(),
            serde_json::to_value(reference(&p, &selector, 1, 2, 0.0, cursor).unwrap()).unwrap()
        );
        let too_long = vec![p.frame_id("r").unwrap(); 4097];
        assert_eq!(
            query(&p, &selector, 1, 2, 0.0, Some((&too_long, p.fingerprint())))
                .unwrap_err()
                .code,
            "invalid_node_id"
        );
    }
    let p = profile("focus 7\n");
    let selector = FrameSelector {
        frame_name: Some("focus".into()),
        frame_id: None,
    };
    let unknown = [p.frame_id("focus").unwrap()];
    for query in [super::callers, super::callees] {
        assert_eq!(
            query(&p, &selector, 1, 2, 0.0, Some((&unknown, p.fingerprint())))
                .unwrap_err()
                .code,
            "invalid_node_id"
        );
    }
}

#[test]
fn partition_keeps_variable_anchor_rank_ordinals() {
    let p =
        profile("root;a;b;a;c 11\nroot;x;y;a;d 7\nroot;a;b;e 5\nroot;x;a;c 3\nroot;z;a;b;a;c 2\n");
    let selector = FrameSelector {
        frame_name: Some("a".into()),
        frame_id: None,
    };
    assert!(
        p.postings()
            .constant_positions_pair(p.frame_id("a").unwrap() as usize)
            .is_none()
    );
    for callers in [true, false] {
        let query = if callers {
            super::callers
        } else {
            super::callees
        };
        let reference = if callers {
            oracle::callers
        } else {
            oracle::callees
        };
        for (depth, nodes) in [(0, 1), (2, 4), (4, 8), (16, 64)] {
            assert_eq!(
                serde_json::to_value(query(&p, &selector, depth, nodes, 0.0, None).unwrap())
                    .unwrap(),
                serde_json::to_value(reference(&p, &selector, depth, nodes, 0.0, None).unwrap())
                    .unwrap()
            );
        }
    }
}

#[test]
fn terminal_callee_metadata_preserves_recursive_first_positions() {
    assert_eq!(std::mem::size_of::<Node>(), 16);
    assert_eq!(std::mem::size_of::<Child>(), 32);
    let selector = FrameSelector {
        frame_name: Some("focus".into()),
        frame_id: None,
    };
    for input in [
        "root;focus 7\nroot;x;focus 5\n",
        "root;focus;focus 7\nroot;x;focus 5\n",
    ] {
        let p = profile(input);
        let output = callees(&p, &selector, 4, 64, 0.0, None).unwrap();
        assert_eq!(
            serde_json::to_value(output).unwrap(),
            serde_json::to_value(oracle::callees(&p, &selector, 4, 64, 0.0, None).unwrap())
                .unwrap()
        );
    }
}

#[test]
fn demand_direction_verifies_large_tree_beyond_eager_work_guard() {
    let count = 40_000;
    let mut input = String::new();
    for id in 0..count {
        input.push_str(&format!("root;a{id:05};b{id:05};c{id:05} 1\n"));
    }
    let p = profile(&input);
    let selector = FrameSelector {
        frame_name: Some("root".into()),
        frame_id: None,
    };
    assert_eq!(
        oracle::callees(&p, &selector, 3, 3, 0.0, None)
            .unwrap_err()
            .details["resource"],
        "direction_nodes"
    );
    let output = callees(&p, &selector, 3, 3, 0.0, None).unwrap();
    assert_eq!(output.scope_weight, count);
    let root = &output.data.root;
    assert_eq!(root.self_weight, 0);
    assert_eq!(root.total_weight, count);
    assert_eq!(root.children.len(), 1);
    assert_eq!(
        (root.omitted_children, root.omitted_weight),
        (count as usize - 1, count - 1)
    );
    let a = &root.children[0];
    assert_eq!(
        (a.name, a.self_weight, a.total_weight),
        (Some("a00000"), 0, 1)
    );
    assert_eq!(a.children.len(), 1);
    let b = &a.children[0];
    assert_eq!(
        (b.name, b.self_weight, b.total_weight),
        (Some("b00000"), 0, 1)
    );
    assert!(b.children.is_empty());
    assert_eq!((b.omitted_children, b.omitted_weight), (1, 1));
    let data = &output.data;
    assert_eq!(data.continuations_available, count as usize);
    assert_eq!(data.continuations.len(), 128);
    assert_eq!(data.continuations_omitted, count as usize - 128);
    assert!(data.continuations_truncated);
    assert_eq!(data.continuations[0].name, "c00000");
    assert_eq!(
        data.continuations[0].continuation.cursor,
        ["root", "a00000", "b00000", "c00000"].map(|name| p.frame_id(name).unwrap())
    );
    for (index, row) in data.continuations[1..].iter().enumerate() {
        assert_eq!(row.name, format!("a{:05}", index + 1));
        assert_eq!(row.total_weight, 1);
        assert_eq!(
            row.continuation.cursor,
            [p.frame_id("root").unwrap(), p.frame_id(row.name).unwrap()]
        );
    }
    let reasons = serde_json::to_value(output.truncation_reasons).unwrap();
    assert_eq!(
        reasons,
        serde_json::json!([{"kind":"node_budget","max_nodes":3,"omitted_children":count,"omitted_weight":count}])
    );
}

#[test]
fn weighted_callers_match_oracle_with_caps_and_resumes() {
    let mut input = String::new();
    for id in 0..256 {
        let prefix = match id % 4 {
            0 => "root;a;b;a",
            1 => "root;a;a",
            2 => "root;x;a;y;a;a",
            _ => "root;b",
        };
        input.push_str(&format!("{prefix};work{id:04} {}\n", id % 7 + 1));
    }
    let p = profile(&input);
    let selector = FrameSelector {
        frame_name: Some("a".into()),
        frame_id: None,
    };
    assert_eq!(
        p.postings().ranked(p.frame_id("a").unwrap() as usize).len(),
        192
    );
    assert!(
        p.postings()
            .constant_positions_pair(p.frame_id("a").unwrap() as usize)
            .is_none()
    );
    for depth in [0, 1, 4, 16] {
        for nodes in [1, 2, 64] {
            for threshold in [0.0, 50.0] {
                let output = callers(&p, &selector, depth, nodes, threshold, None).unwrap();
                assert_eq!(
                    serde_json::to_value(&output).unwrap(),
                    serde_json::to_value(
                        oracle::callers(&p, &selector, depth, nodes, threshold, None).unwrap()
                    )
                    .unwrap()
                );
                for row in &output.data.continuations {
                    let cursor = Some((&row.continuation.cursor[1..], p.fingerprint()));
                    assert_eq!(
                        serde_json::to_value(
                            callers(&p, &selector, depth, nodes, threshold, cursor).unwrap()
                        )
                        .unwrap(),
                        serde_json::to_value(
                            oracle::callers(&p, &selector, depth, nodes, threshold, cursor)
                                .unwrap()
                        )
                        .unwrap()
                    );
                }
            }
        }
    }
}

#[test]
fn diverse_variable_callers_abort_representative_dictionary() {
    let mut input = String::new();
    for id in 0..192 {
        let prefix = if id % 2 == 0 {
            format!("root;p{id:04};a")
        } else {
            format!("root;pad;p{id:04};a")
        };
        input.push_str(&format!("{prefix};work{id:04} {}\n", id % 11 + 1));
    }
    let p = profile(&input);
    let selector = FrameSelector {
        frame_name: Some("a".into()),
        frame_id: None,
    };
    assert!(
        p.postings()
            .constant_positions_pair(p.frame_id("a").unwrap() as usize)
            .is_none()
    );
    for (depth, nodes) in [(0, 1), (1, 2), (4, 64)] {
        let output = callers(&p, &selector, depth, nodes, 0.0, None).unwrap();
        assert_eq!(
            serde_json::to_value(&output).unwrap(),
            serde_json::to_value(oracle::callers(&p, &selector, depth, nodes, 0.0, None).unwrap())
                .unwrap()
        );
        for row in &output.data.continuations {
            let cursor = Some((&row.continuation.cursor[1..], p.fingerprint()));
            assert_eq!(
                serde_json::to_value(callers(&p, &selector, 4, 64, 0.0, cursor).unwrap()).unwrap(),
                serde_json::to_value(oracle::callers(&p, &selector, 4, 64, 0.0, cursor).unwrap())
                    .unwrap()
            );
        }
    }
}

#[test]
fn capped_representatives_keep_lookahead_and_terminal_differences() {
    let mut input = String::new();
    for id in 0..160 {
        let prefix = match id % 4 {
            0 => "r;a;b;a",
            1 => "long;pad;r;a;b;a",
            2 => "r;a",
            _ => "pad;r;a",
        };
        input.push_str(&format!("{prefix};work{id:04} {}\n", id % 5 + 1));
    }
    let p = profile(&input);
    let selector = FrameSelector {
        frame_name: Some("a".into()),
        frame_id: None,
    };
    for depth in [0, 1, 2, 4] {
        for nodes in [1, 2, 64] {
            let output = callers(&p, &selector, depth, nodes, 0.0, None).unwrap();
            assert_eq!(
                serde_json::to_value(&output).unwrap(),
                serde_json::to_value(
                    oracle::callers(&p, &selector, depth, nodes, 0.0, None).unwrap()
                )
                .unwrap()
            );
            for row in &output.data.continuations {
                let cursor = Some((&row.continuation.cursor[1..], p.fingerprint()));
                assert_eq!(
                    serde_json::to_value(callers(&p, &selector, 4, 64, 0.0, cursor).unwrap())
                        .unwrap(),
                    serde_json::to_value(
                        oracle::callers(&p, &selector, 4, 64, 0.0, cursor).unwrap()
                    )
                    .unwrap()
                );
            }
        }
    }
}

#[test]
fn terminal_only_children_render_without_member_partition() {
    let p = profile("root;x 7\nroot;y 5\nroot;z 3\n");
    let selector = FrameSelector {
        frame_name: Some("root".into()),
        frame_id: None,
    };
    for nodes in [1, 2, 64] {
        let output = callees(&p, &selector, 4, nodes, 0.0, None).unwrap();
        assert_eq!(
            serde_json::to_value(output).unwrap(),
            serde_json::to_value(oracle::callees(&p, &selector, 4, nodes, 0.0, None).unwrap())
                .unwrap()
        );
    }
}

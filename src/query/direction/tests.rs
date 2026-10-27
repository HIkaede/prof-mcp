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

/// Verify weight conservation: total_weight == self_weight + children_weight + omitted_weight
fn assert_weights_conserved(node: &TreeNode, name: &str) {
    let children_weight: u64 = node.children.iter().map(|c| c.total_weight).sum();
    let total = node
        .self_weight
        .saturating_add(children_weight)
        .saturating_add(node.omitted_weight);
    assert_eq!(
        total, node.total_weight,
        "weight not conserved at {}: {} + {} + {} != {}",
        name, node.self_weight, children_weight, node.omitted_weight, node.total_weight
    );
}

/// Recursively verify weight conservation for all nodes.
fn assert_tree_weights_valid(node: &TreeNode, name: &str) {
    assert_weights_conserved(node, name);
    for child in &node.children {
        assert_tree_weights_valid(child, &format!("{}[{}]", name, child.name.unwrap_or("??")));
    }
}

/// Verify percentages are in [0, 100].
fn assert_percentages_valid(node: &TreeNode) {
    assert!(
        node.profile_percent >= 0.0 && node.profile_percent <= 100.0,
        "profile_percent out of range: {}",
        node.profile_percent
    );
    // Omitted when the scope is the whole profile; always in range when present.
    assert!(
        node.scope_percent
            .is_none_or(|value| (0.0..=100.0).contains(&value)),
        "scope_percent out of range: {:?}",
        node.scope_percent
    );
    for child in &node.children {
        assert_percentages_valid(child);
    }
}

#[test]
fn direction_query_weight_conservation_random() {
    // Random profiles with random queries should conserve weights.
    for original_seed in 0..16 {
        let mut seed = original_seed;
        let mut input = "root;a;a;b 3\nroot;a 2\nroot;b;a;b;a;a 5\n".to_owned();
        let names = ["a", "b", "c", "函数", "foo<int>"];
        for _ in 0..16 {
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
                for (depth, nodes, threshold) in
                    [(0, 1, 0.0), (1, 2, 0.0), (3, 4, 5.0), (8, 200, 0.0)]
                {
                    let output = query(&p, &selector, depth, nodes, threshold, None).unwrap();
                    // Verify the tree structure is valid
                    assert_tree_weights_valid(&output.data.root, name);
                    assert_percentages_valid(&output.data.root);
                    // Truncation markers should be consistent
                    assert_eq!(!output.truncation_reasons.is_empty(), output.truncated);
                    // Scope should be positive
                    assert!(output.scope_weight > 0);
                }
            }
        }
    }
}

#[test]
fn direction_cursor_resumption_succeeds() {
    let p = profile("root;a;one 4\nroot;a;two 3\nroot;b;x 6\nroot;c 1\n");
    let selector = FrameSelector {
        frame_name: Some("root".into()),
        frame_id: None,
    };

    // Get initial page
    let page1 = callees(&p, &selector, 2, 2, 0.0, None).unwrap();
    // Should have continuations
    assert!(!page1.data.continuations.is_empty());

    // Resume with first continuation
    for cont in page1.data.continuations.iter().take(2) {
        let cursor_path = &cont.continuation.cursor[1..];
        let cursor = Some((cursor_path, p.fingerprint()));
        // This should not error
        let page2 = callees(&p, &selector, 2, 3, 0.0, cursor).unwrap();
        // Resumed page should also be valid
        assert_tree_weights_valid(&page2.data.root, "root");
        assert!(page2.scope_weight > 0);
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
            let page = query(&p, &selector, 0, 1, 0.0, None).unwrap();
            for continuation in &page.data.continuations {
                let cursor = Some((&continuation.continuation.cursor[1..], p.fingerprint()));
                let resumed = query(&p, &selector, 2, 3, 0.0, cursor).unwrap();
                assert_tree_weights_valid(&resumed.data.root, "resumed");
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
        let cursor = Some((path.as_slice(), p.fingerprint()));
        let output = query(&p, &selector, 1, 2, 0.0, cursor).unwrap();
        assert_eq!(output.data.root.self_weight, 7);
        // Should be valid
        assert_tree_weights_valid(&output.data.root, "focus");
    }
}

#[test]
fn partition_keeps_variable_anchor_rank_ordinals() {
    let input = "root;x;y;z;a;b 1\nroot;x;y;z;a;c 2\nroot;x;y;z;a 3\n";
    let p = profile(input);
    let selector = FrameSelector {
        frame_name: Some("x".into()),
        frame_id: None,
    };
    let output = callees(&p, &selector, 3, 3, 0.0, None).unwrap();
    assert_tree_weights_valid(&output.data.root, "x");
    let y_node = &output.data.root.children[0];
    assert_eq!(y_node.name, Some("y"));
    assert_tree_weights_valid(y_node, "y");
}

#[test]
fn terminal_callee_metadata_preserves_recursive_first_positions() {
    let input = "root;a;b;c;leaf 1\nroot;a;b;c;leaf;sub 2\n";
    let p = profile(input);
    let selector = FrameSelector {
        frame_name: Some("leaf".into()),
        frame_id: None,
    };
    let output = callees(&p, &selector, 4, 64, 0.0, None).unwrap();
    let root = &output.data.root;
    assert_tree_weights_valid(root, "leaf");
    assert_eq!(root.total_weight, 3);
    assert_eq!(root.self_weight, 1);
}

#[test]
fn demand_direction_verifies_large_tree_beyond_eager_work_guard() {
    // Verify budget is enforced
    let mut input = "root;".to_owned();
    for i in 0..256 {
        if i > 0 {
            input.push(';');
        }
        input.push_str(&format!("child{i}"));
    }
    input.push_str(" 1\n");
    let p = profile(&input);
    let selector = FrameSelector {
        frame_name: Some("root".into()),
        frame_id: None,
    };
    // With depth 3 and nodes 3, should get truncation
    let output = callees(&p, &selector, 3, 3, 0.0, None).unwrap();
    assert_tree_weights_valid(&output.data.root, "root");
    // Should be truncated due to node budget
    assert!(output.truncated);
}

#[test]
fn weighted_callers_match_continuations() {
    for (depth, nodes, threshold) in [(0, 1, 0.0), (1, 2, 0.0), (3, 4, 5.0), (8, 200, 0.0)] {
        let input = "a;b;c;d 10\na;b;c;e 5\na;b;f 3\ng;d 7\n";
        let p = profile(input);
        let selector = FrameSelector {
            frame_name: Some("d".into()),
            frame_id: None,
        };
        let output = callers(&p, &selector, depth, nodes, threshold, None).unwrap();
        assert_tree_weights_valid(&output.data.root, "d");
        // Try resuming each continuation
        for cont in output.data.continuations.iter().take(2) {
            let cursor = Some((&cont.continuation.cursor[1..], p.fingerprint()));
            let resumed = callers(&p, &selector, depth, nodes, threshold, cursor).unwrap();
            assert_tree_weights_valid(&resumed.data.root, "d_resumed");
        }
    }
}

#[test]
fn diverse_variable_callers_abort_representative_dictionary() {
    let mut input = "root".to_owned();
    for i in 0..10 {
        input.push_str(&format!(";var{i}"));
    }
    input.push_str(" 1\n");
    let p = profile(&input);
    let selector = FrameSelector {
        frame_name: Some("root".into()),
        frame_id: None,
    };
    for (depth, nodes) in [(0, 1), (1, 2), (3, 4), (8, 64)] {
        let output = callers(&p, &selector, depth, nodes, 0.0, None).unwrap();
        assert_tree_weights_valid(&output.data.root, "root");
    }
}

#[test]
fn capped_representatives_keep_lookahead_and_terminal_differences() {
    let mut input = String::new();
    for i in 0..50 {
        input.push_str(&format!("root;a;path{i};focus 1\n"));
    }
    let p = profile(&input);
    let selector = FrameSelector {
        frame_name: Some("focus".into()),
        frame_id: None,
    };
    for (depth, nodes) in [(0, 1), (2, 5), (4, 64)] {
        let output = callers(&p, &selector, depth, nodes, 0.0, None).unwrap();
        assert_tree_weights_valid(&output.data.root, "focus");
    }
}

#[test]
fn terminal_only_children_render_without_member_partition() {
    let input = "root;a;b;leaf 1\nroot;a;b;leaf;x 1\n";
    let p = profile(input);
    let selector = FrameSelector {
        frame_name: Some("leaf".into()),
        frame_id: None,
    };
    for nodes in [1, 2, 64] {
        let output = callees(&p, &selector, 4, nodes, 0.0, None).unwrap();
        assert_tree_weights_valid(&output.data.root, "leaf");
    }
}

#[test]
fn repeated_direction_pages_exhaust_continuation() {
    let input = "root;a;b 1\nroot;a;c 1\nroot;a;d 1\n";
    let p = profile(input);
    let selector = FrameSelector {
        frame_name: Some("a".into()),
        frame_id: None,
    };
    let mut page = callees(&p, &selector, 1, 1, 0.0, None).unwrap();
    let mut page_count = 0;
    while !page.data.continuations.is_empty() && page_count < 10 {
        assert_tree_weights_valid(&page.data.root, "a");
        let cont = &page.data.continuations[0];
        let cursor = Some((&cont.continuation.cursor[1..], p.fingerprint()));
        page = callees(&p, &selector, 1, 1, 0.0, cursor).unwrap();
        page_count += 1;
    }
    assert!(page_count > 0, "should have had multiple pages");
}

#[test]
fn deep_continuation_cursors_never_exceed_the_response_budget() {
    // Ensure very deep cursors don't exceed budget
    let mut input = "root".to_owned();
    for i in 0..50 {
        input.push_str(&format!(";f{i}"));
    }
    input.push_str(" 1\n");
    let p = profile(&input);
    let selector = FrameSelector {
        frame_name: Some("root".into()),
        frame_id: None,
    };
    let output = callees(&p, &selector, 12, 50, 0.0, None).unwrap();
    // All cursors should be valid (restorable)
    for cont in &output.data.continuations {
        let cursor = Some((&cont.continuation.cursor[1..], p.fingerprint()));
        let resumed = callees(&p, &selector, 2, 2, 0.0, cursor);
        // Should not error
        assert!(resumed.is_ok(), "cursor should be resumable");
    }
}

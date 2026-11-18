use std::{cell::Cell, fmt::Write as _, io::Cursor};

use prof_mcp::{
    profile::{BuildLimits, ContextRef, Profile, ProfileBuilder},
    query::{
        Cancellation,
        selection::{FrameRef, Metric, Predicate, Selection, SelectionBudget},
    },
};

fn profile(folded: &str) -> Profile {
    ProfileBuilder::new(BuildLimits::default())
        .from_reader(Cursor::new(folded), "/selection.folded".into())
        .unwrap()
}

fn frame(name: &str) -> FrameRef {
    FrameRef::FrameName(name.to_owned())
}

fn path_prefix(names: &[&str]) -> Predicate {
    Predicate::PathPrefix(names.iter().map(|name| (*name).to_owned()).collect())
}

fn selection(profile: &Profile, predicate: Predicate) -> Selection {
    Selection::new(profile, predicate).unwrap()
}

fn context_for(profile: &Profile, names: &[&str]) -> ContextRef {
    let prefix: Vec<_> = names
        .iter()
        .map(|name| profile.frame_id(name).unwrap())
        .collect();
    profile.context_for_prefix(&prefix).unwrap()
}

#[test]
fn path_association_is_preserved() {
    let p = profile("main;A;C;X 50\nmain;B;C;Y 50\n");
    let q = profile("main;A;C;Y 50\nmain;B;C;X 50\n");
    let predicate = Predicate::ContainsAll(vec![frame("A"), frame("X")]);
    for (profile, expected) in [(&p, 50), (&q, 0)] {
        assert_eq!(
            selection(profile, predicate.clone())
                .evaluate(profile)
                .unwrap()
                .scope_weight(),
            expected
        );
    }
}

#[test]
fn range_covers_each_prefix_including_compressed_edges_and_self() {
    let profile = profile("entry;A;B 10\nentry;A;B;C 30\nentry;A;B;D;E 50\nz;Q;A 20\n");
    let root = ContextRef::ROOT;
    let entry = context_for(&profile, &["entry"]);
    let a = context_for(&profile, &["entry", "A"]);
    let b = context_for(&profile, &["entry", "A", "B"]);
    assert_eq!(entry.node_id, a.node_id);
    assert_eq!(a.node_id, b.node_id);
    assert_eq!([entry.offset, a.offset, b.offset], [0, 1, 2]);
    assert_eq!(profile.context_frame(root), None);
    assert_eq!(profile.context_depth(root), Some(0));
    assert_eq!(
        profile.context_children(entry).unwrap().collect::<Vec<_>>(),
        [a]
    );
    assert_eq!(profile.context_range(root), Some(0..4));
    assert_eq!(profile.context_range(b), Some(0..3));

    let root_selection = selection(&profile, Predicate::default());
    let facts = root_selection
        .refine(&profile, Predicate::UnderContext(b))
        .unwrap()
        .evaluate(&profile)
        .unwrap();
    assert_eq!(facts.scope_weight(), 90);
    assert_eq!(facts.iter().collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(facts.iter_range(1..4).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(facts.iter_range(3..4).next(), None);
    assert_eq!(
        root_selection
            .evaluate(&profile)
            .unwrap()
            .iter_range(1..2)
            .collect::<Vec<_>>(),
        [1]
    );

    let mut contexts = vec![root];
    while let Some(context) = contexts.pop() {
        contexts.extend(profile.context_children(context).unwrap());
        let prefix = profile.context_prefix(context).unwrap();
        let expected: Vec<_> = profile
            .stacks()
            .iter()
            .enumerate()
            .filter_map(|(id, stack)| stack.frames.starts_with(prefix).then_some(id as u32))
            .collect();
        assert_eq!(
            profile.context_range(context).unwrap().collect::<Vec<_>>(),
            expected
        );
        assert_eq!(profile.context_for_prefix(prefix), Some(context));
    }
}

#[test]
fn wide_context_lookup_matches_weight_ranked_children_and_compressed_edges() {
    for (parent_path, names, tied_weights) in [
        ("", &[][..], true),
        ("root;fan;", &["root", "fan"][..], false),
    ] {
        let mut folded = String::from("other;aaa_missing;zzz_missing 5\n");
        if !names.is_empty() {
            folded.push_str("root;fan 7\n");
        }
        for index in 0..128 {
            let weight = if tied_weights { 2 } else { 2 + index * 13 % 17 };
            writeln!(folded, "{parent_path}child-{index:03};inside;leaf {weight}").unwrap();
        }
        if !tied_weights {
            folded.push_str("root;fan;child-100;inside;second 2\n");
        }
        let p = profile(&folded);
        let parent = context_for(&p, names);
        let children: Vec<_> = p.context_children(parent).unwrap().collect();
        assert_eq!(children.len(), 128 + usize::from(names.is_empty()));
        for child in children {
            let node = &p.cct().nodes()[child.node_id as usize];
            let expected =
                1 + usize::from(!tied_weights && p.context_frame(child) == p.frame_id("child-100"));
            for offset in 0..node.label_len() {
                let context = ContextRef { offset, ..child };
                let prefix = p.context_prefix(context).unwrap();
                assert_eq!(p.context_for_prefix(prefix), Some(context));
                assert_eq!(p.context_range(context).unwrap().len(), expected);
            }
        }
        for missing in ["aaa_missing", "zzz_missing"] {
            let mut prefix = p.context_prefix(parent).unwrap().to_vec();
            prefix.push(p.frame_id(missing).unwrap());
            assert_eq!(p.context_for_prefix(&prefix), None);
        }
        let mut prefix = p.context_prefix(parent).unwrap().to_vec();
        prefix.extend([
            p.frame_id("child-100").unwrap(),
            p.frame_id("leaf").unwrap(),
        ]);
        assert_eq!(p.context_for_prefix(&prefix), None);
    }
}

#[test]
fn wide_context_lookup_falls_back_for_a_large_child_subtree() {
    let mut folded = String::from("root 3\nroot;wide;deep 17\n");
    for index in 0..128 {
        writeln!(folded, "root;single-{index:03} {}", 500 + index).unwrap();
    }
    for index in 0..160 {
        writeln!(folded, "root;wide;deep;leaf-{index:03} 1").unwrap();
    }
    let p = profile(&folded);
    let root = context_for(&p, &["root"]);
    let wide = p.context_children(root).unwrap().last().unwrap();
    assert_eq!(p.context_frame(wide), p.frame_id("wide"));
    assert_eq!(
        p.context_for_prefix(p.context_prefix(wide).unwrap()),
        Some(wide)
    );

    let deep = context_for(&p, &["root", "wide", "deep"]);
    assert_eq!(deep, ContextRef { offset: 1, ..wide });
    let facts = selection(&p, path_prefix(&["root", "wide", "deep"]))
        .evaluate(&p)
        .unwrap();
    assert_eq!((facts.len(), facts.scope_weight()), (161, 177));

    let leaf = context_for(&p, &["root", "wide", "deep", "leaf-159"]);
    let stack = p.stacks().stack(p.context_range(leaf).unwrap().start);
    assert_eq!(p.frame_name(*stack.frames.last().unwrap()), "leaf-159");
    assert_eq!(stack.weight, 1);
}

#[test]
fn named_path_prefix_preserves_raw_frames_and_stack_root() {
    let p = profile(concat!(
        "root;函数 [liba.so];函数 [liba.so] 11\n",
        "root;函数 [liba.so];函数 [liba.so];retry 13\n",
        "root;函数 [liba.so];函数 [liba.so];done 17\n",
        "root;函数 [liba.so];skip;函数 [liba.so] 19\n",
        "root;函数 [libb.so];函数 [liba.so] 23\n",
        "other;root;函数 [liba.so];函数 [liba.so] 29\n",
    ));
    let prefix = ["root", "函数 [liba.so]", "函数 [liba.so]"];
    let branch = selection(&p, path_prefix(&prefix));
    assert_eq!(
        branch.predicate,
        Predicate::UnderContext(context_for(&p, &prefix))
    );
    let facts = branch
        .evaluate_with_budget(&p, SelectionBudget { max_stack_ids: 0 })
        .unwrap();
    assert_eq!((facts.len(), facts.scope_weight()), (3, 41));

    let copied =
        serde_json::from_str::<Selection>(&serde_json::to_string(&branch).unwrap()).unwrap();
    let selected = copied
        .refine(&p, Predicate::ExcludeAny(vec![frame("retry")]))
        .unwrap()
        .evaluate(&p)
        .unwrap();
    assert_eq!((selected.len(), selected.scope_weight()), (2, 28));
    let changed = profile("root;函数 [liba.so];函数 [liba.so] 43\n");
    assert_eq!(
        copied.evaluate(&changed).unwrap_err().code,
        "snapshot_unavailable"
    );

    let root_span = ["root", "函数 [liba.so]"];
    let inside_edge = ["other", "root"];
    for (names, total) in [(&root_span[..], 60), (&inside_edge[..], 29)] {
        let branch = selection(&p, path_prefix(names));
        assert_eq!(
            branch.predicate,
            Predicate::UnderContext(context_for(&p, names))
        );
        assert_eq!(branch.evaluate(&p).unwrap().scope_weight(), total);
    }
    assert!(context_for(&p, &inside_edge).offset > 0);
}

#[test]
fn empty_and_missing_path_prefixes_are_boolean_identities() {
    let p = profile("root;A 10\nroot;B;A 30\nother;B 40\n");
    let full = path_prefix(&[]);
    let present = path_prefix(&["root", "A"]);
    let checks = Cell::new(0);
    let no_scan = || {
        checks.set(checks.get() + 1);
        checks.get() > 2
    };
    assert_eq!(
        selection(&p, full.clone()),
        selection(&p, Predicate::default())
    );
    for absent in [
        path_prefix(&["root", "missing"]),
        path_prefix(&["A"]),
        path_prefix(&["root", "A", "B"]),
    ] {
        let bound = selection(&p, absent.clone());
        assert_eq!(bound.predicate, Predicate::Any(Vec::new()));
        checks.set(0);
        assert!(
            bound
                .evaluate_cancellable(&p, Cancellation::new(&no_scan))
                .unwrap()
                .is_empty()
        );
        for (predicate, weight) in [
            (Predicate::All(vec![present.clone(), absent.clone()]), 0),
            (Predicate::Any(vec![present.clone(), absent]), 10),
            (Predicate::All(vec![present.clone(), full.clone()]), 10),
            (Predicate::Any(vec![present.clone(), full.clone()]), 80),
        ] {
            assert_eq!(
                selection(&p, predicate)
                    .evaluate(&p)
                    .unwrap()
                    .scope_weight(),
                weight
            );
        }
    }
}

#[test]
fn snapshots_bind_references_and_ignore_folded_row_order() {
    let before = profile("root;A;C 5\nroot;B 20\nroot;A;C 7\n");
    let reordered = profile("root;B 20\nroot;A;C 12\n");
    let rebound = profile("root;A;Y 12\nroot;B 20\n");
    assert_ne!(before.fingerprint(), reordered.fingerprint());
    assert_eq!(before.snapshot_id(), reordered.snapshot_id());
    assert_ne!(before.snapshot_id(), rebound.snapshot_id());

    let context = context_for(&before, &["root", "A", "C"]);
    let a = before.frame_id("A").unwrap();
    let predicate = Predicate::All(vec![
        Predicate::UnderContext(context),
        Predicate::ContainsAll(vec![FrameRef::FrameId(a)]),
    ]);
    let pinned = selection(&before, Predicate::default())
        .refine(&before, predicate)
        .unwrap();
    let copied: Selection = serde_json::from_str(&serde_json::to_string(&pinned).unwrap()).unwrap();
    assert_eq!(copied.evaluate(&before).unwrap().scope_weight(), 12);
    assert_eq!(copied.evaluate(&reordered).unwrap().scope_weight(), 12);
    assert_eq!(
        copied.evaluate(&rebound).unwrap_err().code,
        "snapshot_unavailable"
    );
}

#[test]
fn references_limits_and_capabilities_are_checked() {
    let p = profile("root;A 10\n");
    let start = selection(&p, Predicate::default());
    let id = p.frame_id("A").unwrap();
    for predicate in [
        Predicate::ContainsAny(vec![FrameRef::FrameId(id)]),
        Predicate::UnderContext(context_for(&p, &["root"])),
    ] {
        assert_eq!(
            Selection::new(&p, predicate).unwrap_err().code,
            "invalid_selection"
        );
    }
    assert_eq!(
        Selection::new(&p, Predicate::ContainsAll(vec![frame("missing")]))
            .unwrap_err()
            .code,
        "frame_not_found"
    );
    assert_eq!(
        start
            .refine(
                &p,
                Predicate::ContainsAll(vec![FrameRef::FrameId(u32::MAX)])
            )
            .unwrap_err()
            .code,
        "frame_not_found"
    );
    assert_eq!(
        start
            .refine(
                &p,
                Predicate::UnderContext(ContextRef {
                    node_id: 1,
                    offset: 2,
                }),
            )
            .unwrap_err()
            .code,
        "invalid_context_ref"
    );
    let mut nested = Predicate::ContainsAll(vec![frame("root")]);
    for _ in 0..4 {
        nested = Predicate::All(vec![nested]);
    }
    let too_many = vec![Predicate::ContainsAny(vec![frame("root")]); 33];
    for invalid in [
        nested,
        Predicate::Any(too_many),
        Predicate::OrderedPath(vec![frame("A"); 33]),
        path_prefix(&["A"; 33]),
        Predicate::All(vec![
            path_prefix(&["A"; 32]),
            Predicate::ExcludeAny(vec![frame("root")]),
        ]),
        Predicate::PathPrefix(vec!["x".repeat(17_000)]),
        Predicate::PathPrefix(vec!["\u{0001}".repeat(3_000)]),
        Predicate::ContainsAll(vec![FrameRef::FrameName("x".repeat(17_000))]),
        Predicate::ContainsAll(vec![FrameRef::FrameName("\u{0001}".repeat(3_000))]),
    ] {
        assert_eq!(
            start.refine(&p, invalid).unwrap_err().code,
            "query_budget_exceeded"
        );
    }
    for unsupported in [
        Predicate::TimeRange {
            begin_ns: 0,
            end_ns: 10,
        },
        Predicate::Thread { pid: 1, tid: 1 },
    ] {
        assert_eq!(
            start.refine(&p, unsupported).unwrap_err().code,
            "capability_unavailable"
        );
    }
    assert!(Metric::OpaqueWeight.validate(&p).is_ok());
    assert_eq!(
        Metric::SampleCount.validate(&p).unwrap_err().code,
        "capability_unavailable"
    );
}

#[test]
fn sparse_candidates_are_deduplicated_sorted_and_budgeted() {
    let p = profile("root;A 40\nroot;B;A 70\nroot;B;A;C 90\nroot;D 200\n");
    let a = Predicate::ContainsAll(vec![frame("A")]);
    let context = context_for(&p, &["root", "B"]);
    let pinned = selection(&p, Predicate::default())
        .refine(
            &p,
            Predicate::Any(vec![a.clone(), Predicate::UnderContext(context)]),
        )
        .unwrap();
    assert_eq!(
        pinned.evaluate(&p).unwrap().iter().collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert_eq!(pinned.evaluate(&p).unwrap().scope_weight(), 200);
    let ordered = selection(
        &p,
        Predicate::Any(vec![
            Predicate::ContainsAll(vec![frame("A")]),
            Predicate::ContainsAll(vec![frame("D")]),
        ]),
    );
    assert_eq!(
        ordered.evaluate(&p).unwrap().iter().collect::<Vec<_>>(),
        [0, 1, 2, 3]
    );
    let noncontiguous = selection(&p, Predicate::ExcludeAny(vec![frame("C")]));
    assert_eq!(
        noncontiguous
            .evaluate(&p)
            .unwrap()
            .iter_range(1..5)
            .collect::<Vec<_>>(),
        [1, 3]
    );
    assert_eq!(
        noncontiguous
            .evaluate_with_budget(&p, SelectionBudget { max_stack_ids: 1 })
            .unwrap_err()
            .code,
        "query_budget_exceeded"
    );
}

#[test]
fn cancellation_interrupts_selection_scans() {
    let folded: String = (0..4096).map(|id| format!("root;item{id} 1\n")).collect();
    let p = profile(&folded);
    let selection = selection(&p, Predicate::ExcludeAny(vec![frame("item0")]));
    let checks = Cell::new(0);
    let cancel = || {
        checks.set(checks.get() + 1);
        checks.get() == 4
    };
    assert_eq!(
        selection
            .evaluate_cancellable(&p, Cancellation::new(&cancel))
            .unwrap_err()
            .code,
        "query_cancelled"
    );
    assert_eq!(checks.get(), 4);
    assert_eq!(selection.evaluate(&p).unwrap().scope_weight(), 4095);
}

#[test]
fn randomized_selection_and_contexts_match_independent_path_scan() {
    let mut random = 17u64;
    let mut next = |n: usize| {
        random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
        ((random >> 32) as usize) % n
    };
    let names = ["A", "B", "C"];
    for _ in 0..250 {
        let mut folded = String::from("A 1\nB 1\nC 1\n");
        for _ in 0..10 {
            let path: Vec<_> = (0..1 + next(5)).map(|_| names[next(3)]).collect();
            folded.push_str(&format!("{} {}\n", path.join(";"), 1 + next(10)));
        }
        let p = profile(&folded);
        let name_id = |name: &str| p.frame_id(name).unwrap();
        let mut contexts = vec![ContextRef::ROOT];
        let mut cursor = 0;
        while cursor < contexts.len() {
            let context = contexts[cursor];
            contexts.extend(p.context_children(context).unwrap());
            let prefix = p.context_prefix(context).unwrap();
            let expected: Vec<_> = p
                .stacks()
                .iter()
                .enumerate()
                .filter_map(|(id, row)| row.frames.starts_with(prefix).then_some(id as u32))
                .collect();
            assert_eq!(
                p.context_range(context).unwrap().collect::<Vec<_>>(),
                expected
            );
            assert_eq!(p.context_for_prefix(prefix), Some(context));
            cursor += 1;
        }

        let start = selection(&p, Predicate::default());
        for _ in 0..40 {
            let a = names[next(3)];
            let b = names[next(3)];
            let c = names[next(3)];
            let context = contexts[next(contexts.len())];
            let prefix = p.context_prefix(context).unwrap();
            let pa = Predicate::ContainsAll(vec![frame(a)]);
            let pb = Predicate::ContainsAny(vec![frame(b)]);
            let ctx = Predicate::UnderContext(context);
            let pc = Predicate::ExcludeAny(vec![frame(c)]);
            let ordered = Predicate::OrderedPath(vec![frame(a), frame(b), frame(a)]);
            let choices = [
                Predicate::All(vec![pa.clone(), pb.clone(), pc.clone()]),
                Predicate::All(vec![pc, pb, pa.clone()]),
                Predicate::Any(vec![pa.clone(), ctx.clone(), ctx.clone()]),
                Predicate::All(vec![pa, Predicate::ExcludeAny(vec![frame(a)])]),
                Predicate::All(vec![ctx, ordered]),
            ];
            let kind = next(choices.len());
            let picked = start.refine(&p, choices[kind].clone()).unwrap();
            let facts = picked.evaluate(&p).unwrap();
            let (mut ids, mut weight) = (Vec::new(), 0);
            for (id, row) in p.stacks().iter().enumerate() {
                let contains = |name| row.frames.contains(&name_id(name));
                let yes = match kind {
                    0 | 1 => contains(a) && contains(b) && !contains(c),
                    2 => contains(a) || row.frames.starts_with(prefix),
                    3 => false,
                    _ => {
                        row.frames.starts_with(prefix)
                            && (0..row.frames.len()).any(|i| {
                                row.frames[i] == name_id(a)
                                    && (i + 1..row.frames.len()).any(|j| {
                                        row.frames[j] == name_id(b)
                                            && row.frames[j + 1..].contains(&name_id(a))
                                    })
                            })
                    }
                };
                if yes {
                    ids.push(id as u32);
                    weight += row.weight;
                }
            }
            assert_eq!(facts.iter().collect::<Vec<_>>(), ids);
            assert_eq!(facts.scope_weight(), weight);
        }
    }
}

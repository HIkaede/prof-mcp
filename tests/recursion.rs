mod support;

use prof_mcp::query::{self, FrameSelector};

#[test]
fn shared_callee_accounting() {
    let diamond = support::profile(support::DIAMOND);
    let x = support::frame(&diamond, "X");
    assert_eq!(diamond.frame_stats()[x as usize].self_weight, 50);
    assert_eq!(diamond.frame_stats()[x as usize].inclusive_weight, 50);
    let callers = support::json(
        query::callers(
            &diamond,
            &FrameSelector {
                frame_id: Some(x),
                frame_name: None,
            },
            5,
            64,
            0.0,
            None,
        )
        .unwrap(),
    );
    let names: Vec<_> = callers["data"]["root"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["B", "A"]);
    let shared = support::profile(support::SHARED_CALLEE);
    let x = support::frame(&shared, "X");
    assert_eq!(shared.frame_stats()[x as usize].inclusive_weight, 50);
    let callees = support::json(
        query::callees(
            &shared,
            &FrameSelector {
                frame_id: Some(x),
                frame_name: None,
            },
            5,
            64,
            0.0,
            None,
        )
        .unwrap(),
    );
    let names: Vec<_> = callees["data"]["root"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["leaf2", "leaf1"]);
}

#[test]
fn recursive_direction_anchors() {
    let profile = support::profile(support::RECURSION);
    let foo = support::frame(&profile, "foo");
    assert_eq!(profile.frame_stats()[foo as usize].inclusive_weight, 10);
    let callers = support::json(
        query::callers(
            &profile,
            &FrameSelector {
                frame_id: Some(foo),
                frame_name: None,
            },
            5,
            64,
            0.0,
            None,
        )
        .unwrap(),
    );
    assert_eq!(callers["data"]["root"]["children"][0]["name"], "foo");
    let callees = support::json(
        query::callees(
            &profile,
            &FrameSelector {
                frame_id: Some(foo),
                frame_name: None,
            },
            5,
            64,
            0.0,
            None,
        )
        .unwrap(),
    );
    assert_eq!(callees["data"]["root"]["children"][0]["name"], "foo");
    assert_eq!(
        callees["data"]["root"]["children"][0]["children"][0]["name"],
        "bar"
    );
}

#[test]
fn resume_caller_continuations() {
    let profile = support::profile("root;mid;deep;deeper;anchor 10\n");
    let anchor = support::frame(&profile, "anchor");
    let shallow = support::json(
        query::callers(
            &profile,
            &FrameSelector {
                frame_id: Some(anchor),
                frame_name: None,
            },
            1,
            64,
            0.0,
            None,
        )
        .unwrap(),
    );
    let continuations = shallow["data"]["continuations"].as_array().unwrap();
    assert!(!continuations.is_empty());
    let first = &continuations[0];
    assert_eq!(first["continuation"]["fingerprint"], profile.fingerprint());
    assert!(first["continuation"]["cursor"].as_array().unwrap().len() >= 3);

    let fingerprint = first["continuation"]["fingerprint"].as_str().unwrap();
    let node_path: Vec<u32> = first["continuation"]["cursor"]
        .as_array()
        .unwrap()
        .iter()
        .skip(1)
        .map(|value| value.as_u64().unwrap() as u32)
        .collect();
    let resumed = support::json(
        query::callers(
            &profile,
            &FrameSelector {
                frame_id: Some(anchor),
                frame_name: None,
            },
            2,
            64,
            0.0,
            Some((node_path.as_slice(), fingerprint)),
        )
        .unwrap(),
    );
    assert_eq!(resumed["data"]["root"]["name"], first["name"]);
    assert_eq!(resumed["scope_weight"], first["total_weight"]);
}

#[test]
fn continuation_frames_report_scope_local_percents() {
    let profile = support::profile("r1;anchor 5\nr2;anchor 5\nother 5\n");
    let selector = FrameSelector {
        frame_id: Some(support::frame(&profile, "anchor")),
        frame_name: None,
    };
    let shallow = support::json(query::callers(&profile, &selector, 1, 1, 0.0, None).unwrap());
    let continuations = shallow["data"]["continuations"].as_array().unwrap();
    let first = &continuations[0];
    assert_eq!(first["total_weight"], 5);
    let fingerprint = first["continuation"]["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let node_path: Vec<u32> = first["continuation"]["cursor"]
        .as_array()
        .unwrap()
        .iter()
        .skip(1)
        .map(|value| value.as_u64().unwrap() as u32)
        .collect();
    let resumed = support::json(
        query::callers(
            &profile,
            &selector,
            1,
            1,
            0.0,
            Some((node_path.as_slice(), &fingerprint)),
        )
        .unwrap(),
    );
    let frame = &resumed["data"]["frame"];
    assert_eq!(frame["name"], "anchor");
    assert_eq!(resumed["scope_weight"], 5);
    // The anchor row is scoped to the page, so its inclusive weight is the page weight.
    assert_eq!(frame["inclusive_weight"], 5);
    assert_eq!(frame["self_weight"], 5);
    assert_eq!(frame["stack_count"], 1);
    assert_eq!(frame["scope_percent"], 100.0);
    assert_eq!(
        frame["profile_percent"].as_f64().unwrap().round(),
        (100.0_f64 / 3.0).round()
    );
}

#[test]
fn reject_invalid_continuations() {
    let profile = support::profile("root;mid;anchor 10\n");
    let anchor = support::frame(&profile, "anchor");
    let selector = FrameSelector {
        frame_id: Some(anchor),
        frame_name: None,
    };
    let error =
        query::callers(&profile, &selector, 1, 8, 0.0, Some((&[999], "wrong"))).unwrap_err();
    assert_eq!(error.code, "profile_changed");
    let empty = query::callers(&profile, &selector, 1, 8, 0.0, Some((&[], "x"))).unwrap_err();
    assert_eq!(empty.code, "invalid_node_id");
}

#[test]
fn repeated_direction_pages() {
    let stack = (0..80)
        .map(|i| format!("frame{i}"))
        .collect::<Vec<_>>()
        .join(";");
    let profile = support::profile(&format!("{stack} 10\n"));
    for callers in [true, false] {
        let selector = FrameSelector {
            frame_name: Some(if callers { "frame79" } else { "frame0" }.into()),
            frame_id: None,
        };
        let direction = if callers {
            query::callers
        } else {
            query::callees
        };
        let mut path = Vec::new();
        for step in 0..80 {
            let continuation = if path.is_empty() {
                None
            } else {
                Some((path.as_slice(), profile.fingerprint()))
            };
            let page =
                support::json(direction(&profile, &selector, 0, 1, 0.0, continuation).unwrap());
            let expected = if callers { 79 - step } else { step };
            assert_eq!(page["data"]["root"]["name"], format!("frame{expected}"));
            assert_eq!(page["scope_weight"], 10);
            let omitted = page["data"]["continuations"].as_array().unwrap();
            if step == 79 {
                assert!(omitted.is_empty());
            } else {
                path = omitted[0]["continuation"]["cursor"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .skip(1)
                    .map(|id| id.as_u64().unwrap() as u32)
                    .collect();
                assert_eq!(path.len(), step + 1);
            }
        }
    }
}

#[test]
fn deep_continuation() {
    let input = format!("root;{} 7\n", vec!["a"; 4095].join(";"));
    let profile = support::profile(&input);
    let selector = FrameSelector {
        frame_name: Some("root".into()),
        frame_id: None,
    };
    let a = support::frame(&profile, "a");
    let prefix = vec![a; 4094];
    let response = support::json(
        query::callees(
            &profile,
            &selector,
            0,
            1,
            0.0,
            Some((&prefix, profile.fingerprint())),
        )
        .unwrap(),
    );
    assert_eq!(response["scope_weight"], 7);
    assert!(
        response["data"]["root"].get("self_weight").is_none(),
        "a zero self weight is omitted"
    );
    assert_eq!(response["data"]["root"]["omitted_weight"], 7);
    let next: Vec<u32> = response["data"]["continuations"][0]["continuation"]["cursor"]
        .as_array()
        .unwrap()
        .iter()
        .skip(1)
        .map(|id| id.as_u64().unwrap() as u32)
        .collect();
    assert_eq!(next.len(), 4095);
    let leaf = support::json(
        query::callees(
            &profile,
            &selector,
            0,
            1,
            0.0,
            Some((&next, profile.fingerprint())),
        )
        .unwrap(),
    );
    assert_eq!(leaf["data"]["root"]["self_weight"], 7);
    assert_eq!(leaf["data"]["root"]["total_weight"], 7);
    assert_eq!(leaf["data"]["continuations"], serde_json::json!([]));
}

#[test]
fn deep_continuation_cursors_never_exceed_the_response_budget() {
    let prefix = vec!["recursive"; 2048].join(";");
    let input: String = (0..128)
        .map(|i| format!("root;{prefix};leaf{i} 1\n"))
        .collect();
    let profile = support::profile(&input);
    let selector = prof_mcp::query::FrameSelector {
        frame_id: Some(support::frame(&profile, "root")),
        frame_name: None,
    };
    let cursor = vec![support::frame(&profile, "recursive"); 2048];
    let bounded = support::json(
        query::callees(
            &profile,
            &selector,
            0,
            1,
            0.0,
            Some((&cursor, profile.fingerprint())),
        )
        .unwrap(),
    );
    // Cursors this deep cost more than the response allows: they are dropped and
    // reported, not returned as a hard query_too_large for the whole page.
    let continuations = bounded["data"]["continuations"].as_array().unwrap();
    assert!(continuations.len() < 128);
    assert_eq!(bounded["data"]["continuations_truncated"], true);
    for continuation in continuations {
        assert!(
            continuation["continuation"]["cursor"]
                .as_array()
                .unwrap()
                .len()
                <= 4097
        );
    }
    assert!(serde_json::to_vec(&bounded).unwrap().len() <= 64 * 1024);
    // Rendering these leaves needs no repeated deep cursors and still fits.
    let page = support::json(
        query::callees(
            &profile,
            &selector,
            1,
            129,
            0.0,
            Some((&cursor, profile.fingerprint())),
        )
        .unwrap(),
    );
    assert_eq!(page["scope_weight"], 128);
    assert_eq!(
        page["data"]["root"]["children"].as_array().unwrap().len(),
        128
    );
    assert!(page["data"]["continuations"].as_array().unwrap().is_empty());
}

#[test]
fn callee_continuation_excludes_anchor_self_weight_outside_the_page() {
    let profile = support::profile("root;a 100\nroot;a;b 1\n");
    let selector = FrameSelector {
        frame_name: Some("a".into()),
        frame_id: None,
    };
    let first = query::callees(&profile, &selector, 0, 8, 0.0, None).unwrap();
    let cursor = &first.data.continuations[0].continuation.cursor;
    let page = query::callees(
        &profile,
        &selector,
        8,
        8,
        0.0,
        Some((&cursor[1..], profile.fingerprint())),
    )
    .unwrap();
    assert_eq!(page.scope_weight, 1);
    assert_eq!(page.data.frame.inclusive_weight, 1);
    assert_eq!(page.data.frame.self_weight, 0);
    assert_eq!(page.data.frame.stack_count, 1);
}

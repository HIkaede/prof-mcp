mod support;

use prof_mcp::query::{self, FrameSelector};

#[test]
fn shared_callee_accounting() {
    let diamond = support::profile(support::DIAMOND);
    let x = support::frame(&diamond, "X");
    assert_eq!(diamond.frame_stats[x as usize].self_weight, 50);
    assert_eq!(diamond.frame_stats[x as usize].inclusive_weight, 50);
    let callers = query::callers(
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
    .unwrap();
    let names: Vec<_> = callers["data"]["root"]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["B", "A"]);
    let shared = support::profile(support::SHARED_CALLEE);
    let x = support::frame(&shared, "X");
    assert_eq!(shared.frame_stats[x as usize].inclusive_weight, 50);
    let callees = query::callees(
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
    .unwrap();
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
    assert_eq!(profile.frame_stats[foo as usize].inclusive_weight, 10);
    let callers = query::callers(
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
    .unwrap();
    assert_eq!(callers["data"]["root"]["children"][0]["name"], "foo");
    let callees = query::callees(
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
    .unwrap();
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
    let shallow = query::callers(
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
    .unwrap();
    let continuations = shallow["data"]["continuations"].as_array().unwrap();
    assert!(!continuations.is_empty());
    let first = &continuations[0];
    assert_eq!(first["profile_fingerprint"], profile.source.fingerprint);
    assert!(first["node_path"].as_array().unwrap().len() >= 2);

    let fingerprint = first["profile_fingerprint"].as_str().unwrap();
    let node_path: Vec<u32> = first["node_path"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_u64().unwrap() as u32)
        .collect();
    let resumed = query::callers(
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
    .unwrap();
    assert_eq!(resumed["data"]["root"]["name"], first["name"]);
    assert_eq!(resumed["scope_weight"], first["total_weight"]);
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
                Some((path.as_slice(), profile.source.fingerprint.as_str()))
            };
            let page = direction(&profile, &selector, 0, 1, 0.0, continuation).unwrap();
            let expected = if callers { 79 - step } else { step };
            assert_eq!(page["data"]["root"]["name"], format!("frame{expected}"));
            assert_eq!(page["scope_weight"], 10);
            let omitted = page["data"]["continuations"].as_array().unwrap();
            if step == 79 {
                assert!(omitted.is_empty());
            } else {
                path = omitted[0]["node_path"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|id| id.as_u64().unwrap() as u32)
                    .collect();
                assert_eq!(path.len(), step + 1);
            }
        }
    }
}

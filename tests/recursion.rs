mod support;

use prof_mcp::query::{self, FrameSelector};

#[test]
fn diamond_and_shared_callee_use_each_stack_once() {
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
fn recursive_anchors_are_leaf_most_for_callers_and_root_most_for_callees() {
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
    )
    .unwrap();
    assert_eq!(callees["data"]["root"]["children"][0]["name"], "foo");
    assert_eq!(
        callees["data"]["root"]["children"][0]["children"][0]["name"],
        "bar"
    );
}

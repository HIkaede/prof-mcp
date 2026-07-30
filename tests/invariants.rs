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

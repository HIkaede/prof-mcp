mod support;

use prof_mcp::profile::{ContextNode, FrameStats, Profile, RecursionStats};

#[test]
fn breakdown_counts_arrays_and_spare_capacity() {
    let mut profile = support::profile("root;foo;foo 7\nroot;bar 3\n");
    let before = profile.memory_breakdown();
    assert_eq!(before.frame_name_bytes, 10);
    assert_eq!(before.frame_offsets_bytes, 4 * 4);
    assert_eq!(before.stack_offsets_bytes, 3 * 4);
    assert_eq!(before.stack_frames_bytes, 5 * 4);
    assert_eq!(before.stack_weights_bytes, 2 * 8);
    assert_eq!(before.posting_offsets_bytes, 4 * 4);
    assert_eq!(before.posting_entries_bytes, 4 * 4);
    assert_eq!(
        before.cct_nodes_bytes,
        5 * std::mem::size_of::<ContextNode>()
    );
    assert_eq!(before.cct_offsets_bytes, 6 * 4);
    assert_eq!(before.cct_edges_bytes, 4 * 4);
    assert_eq!(before.top_rankings_bytes, 3 * 8);
    assert_eq!(
        before.recursion_bytes,
        std::mem::size_of::<RecursionStats>()
    );
    assert_eq!(before.total_bytes, profile.estimated_size_bytes());

    profile.frame_stats.reserve(100);
    profile.source.fingerprint.reserve(100);
    profile.source.canonical_path.reserve(100);
    let after = profile.memory_breakdown();
    assert_eq!(
        after.frame_stats_bytes,
        profile.frame_stats.capacity() * std::mem::size_of::<FrameStats>()
    );
    assert_eq!(
        after.metadata_bytes,
        std::mem::size_of::<Profile>()
            + profile.source.canonical_path.capacity()
            + profile.source.fingerprint.capacity()
    );
    assert_eq!(
        after.total_bytes - before.total_bytes,
        after.frame_stats_bytes - before.frame_stats_bytes + after.metadata_bytes
            - before.metadata_bytes
    );
}

#[test]
fn empty_profile_still_counts_root_and_sentinels() {
    let profile = support::profile("");
    let bytes = profile.memory_breakdown();
    assert_eq!(bytes.frame_name_bytes, 0);
    assert_eq!(bytes.frame_offsets_bytes, 4);
    assert_eq!(bytes.stack_offsets_bytes, 4);
    assert_eq!(bytes.posting_offsets_bytes, 4);
    assert_eq!(bytes.cct_nodes_bytes, std::mem::size_of::<ContextNode>());
    assert_eq!(bytes.cct_offsets_bytes, 8);
    assert_eq!(bytes.cct_edges_bytes, 0);
    assert_eq!(
        bytes.total_bytes,
        bytes.metadata_bytes + 20 + bytes.cct_nodes_bytes
    );
}

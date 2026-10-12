mod support;

use prof_mcp::profile::{ContextNode, Profile, RecursionStats};

#[test]
fn breakdown_counts_frozen_arrays() {
    let profile = support::profile("root;foo;foo 7\nroot;bar 3\n");
    let before = profile.memory_breakdown();
    assert_eq!(before.frame_name_bytes, 10);
    assert_eq!(before.frame_offsets_bytes, 4 * 4);
    assert_eq!(before.stack_offsets_bytes, 3 * 4);
    assert_eq!(before.stack_frames_bytes, 5 * 4);
    assert_eq!(before.stack_weights_bytes, 2 * 8);
    assert_eq!(before.posting_offsets_bytes, 4 * 4);
    assert_eq!(before.posting_entries_bytes, 4 * 4);
    assert_eq!(before.posting_positions_bytes, 4 * 4 + 3 * 4);
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

    assert_eq!(
        before.frame_stats_bytes,
        std::mem::size_of_val(profile.frame_stats())
    );
    assert!(before.metadata_bytes >= std::mem::size_of::<Profile>() + profile.fingerprint().len());
    assert_eq!(std::mem::size_of::<ContextNode>(), 16);
}

#[test]
fn empty_profile_still_counts_root_and_sentinels() {
    let profile = support::profile("");
    let bytes = profile.memory_breakdown();
    assert_eq!(bytes.frame_name_bytes, 0);
    assert_eq!(bytes.frame_offsets_bytes, 4);
    assert_eq!(bytes.stack_offsets_bytes, 4);
    assert_eq!(bytes.posting_offsets_bytes, 4);
    assert_eq!(bytes.posting_positions_bytes, 4);
    assert_eq!(bytes.cct_nodes_bytes, std::mem::size_of::<ContextNode>());
    assert_eq!(bytes.cct_offsets_bytes, 8);
    assert_eq!(bytes.cct_edges_bytes, 0);
    assert_eq!(
        bytes.total_bytes,
        bytes.metadata_bytes + 24 + bytes.cct_nodes_bytes
    );
}

#[test]
fn snapshots_and_large_tables_cannot_be_cloned() {
    use prof_mcp::profile::{ContextCallTree, FrameTable, PostingTable, StackTable};
    trait AmbiguousIfClone<A> {
        fn check() {}
    }
    impl<T: ?Sized> AmbiguousIfClone<()> for T {}
    struct CloneMarker;
    impl<T: Clone> AmbiguousIfClone<CloneMarker> for T {}
    // Type inference becomes ambiguous if one of these types implements Clone.
    let _ = <Profile as AmbiguousIfClone<_>>::check;
    let _ = <FrameTable as AmbiguousIfClone<_>>::check;
    let _ = <StackTable as AmbiguousIfClone<_>>::check;
    let _ = <PostingTable as AmbiguousIfClone<_>>::check;
    let _ = <ContextCallTree as AmbiguousIfClone<_>>::check;
    let _ = <std::sync::Arc<Profile> as Clone>::clone;
}

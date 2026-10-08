use std::{fs, sync::Arc};

use prof_mcp::{cache::ProfileCache, registry};

#[tokio::test]
async fn aliases_share_blobs_but_replacement_refreshes_active_profile_and_metadata() {
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    fs::write(&source, "root;old 1\n").unwrap();
    registry::register(workspace.path(), &source, Some("base"), 1024, Some(100)).unwrap();
    registry::register(
        workspace.path(),
        &source,
        Some("candidate"),
        1024,
        Some(200),
    )
    .unwrap();
    let cache = ProfileCache::new(workspace.path().to_owned(), 1024, 2).unwrap();
    let base = cache.load(Some("base")).await.unwrap();
    let candidate = cache.load(None).await.unwrap();
    assert!(Arc::ptr_eq(&base.profile, &candidate.profile));
    assert_eq!(candidate.alias, "candidate");
    assert_eq!(base.sample_period_us, Some(100));
    assert_eq!(candidate.sample_period_us, Some(200));

    fs::write(&source, "root;new 7\n").unwrap();
    registry::register(workspace.path(), &source, Some("candidate"), 1024, None).unwrap();
    let replaced = cache.load(None).await.unwrap();
    assert!(!Arc::ptr_eq(&base.profile, &replaced.profile));
    assert_eq!(replaced.profile.total_weight, 7);
    assert!(replaced.profile.frame_id("new").is_some());
    assert_eq!(replaced.sample_period_us, None);
    assert!(Arc::ptr_eq(
        &base.profile,
        &cache.load(Some("base")).await.unwrap().profile
    ));
}

#[tokio::test]
async fn damaged_blob_is_rejected_and_a_repaired_blob_can_be_loaded() {
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    let original = b"root;good 1\n";
    fs::write(&source, original).unwrap();
    registry::register(workspace.path(), &source, Some("base"), 1024, None).unwrap();
    let blob = registry::resolve(workspace.path(), None).unwrap().path;
    // Same byte length: rejection must rely on the digest, not only metadata.
    fs::write(&blob, b"root;evil 1\n").unwrap();
    let cache = ProfileCache::new(workspace.path().to_owned(), 1024, 2).unwrap();
    assert_eq!(cache.load(None).await.unwrap_err().code, "registry_corrupt");
    fs::write(&blob, original).unwrap();
    assert!(
        cache
            .load(None)
            .await
            .unwrap()
            .profile
            .frame_id("good")
            .is_some()
    );
}

use std::{fs, sync::Arc};

use prof_mcp::{cache::ProfileCache, registry};

#[tokio::test]
async fn refresh_replaced_alias() {
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    fs::write(&source, "root;old 1\n").unwrap();
    registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
    registry::register(workspace.path(), &source, Some("candidate"), 1024).unwrap();
    let cache = ProfileCache::new(workspace.path().to_owned(), 1024, 2).unwrap();
    let base = cache.load(Some("base")).await.unwrap();
    let candidate = cache.load(None).await.unwrap();
    assert!(Arc::ptr_eq(&base.profile, &candidate.profile));
    assert_eq!(candidate.alias, "candidate");

    fs::write(&source, "root;new 7\n").unwrap();
    registry::register(workspace.path(), &source, Some("candidate"), 1024).unwrap();
    let replaced = cache.load(None).await.unwrap();
    assert!(!Arc::ptr_eq(&base.profile, &replaced.profile));
    assert_eq!(replaced.profile.total_weight, 7);
    assert!(replaced.profile.frame_id("new").is_some());
    assert!(Arc::ptr_eq(
        &base.profile,
        &cache.load(Some("base")).await.unwrap().profile
    ));
}

#[tokio::test]
async fn reload_repaired_blob() {
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    let original = b"root;good 1\n";
    fs::write(&source, original).unwrap();
    registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
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

#[tokio::test]
async fn concurrent_loads_share_profile() {
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("large.folded");
    let input: String = (0..20_000)
        .map(|i| format!("root;caller{i};leaf{i} 1\n"))
        .collect();
    fs::write(&source, input).unwrap();
    let limit = 2 * 1024 * 1024;
    registry::register(workspace.path(), &source, Some("base"), limit).unwrap();
    registry::register(workspace.path(), &source, Some("candidate"), limit).unwrap();
    let cache = ProfileCache::new(workspace.path().to_owned(), limit, 2).unwrap();
    let mut tasks = Vec::new();
    for alias in ["base", "candidate", "base", "candidate"] {
        let cache = cache.clone();
        tasks.push(tokio::spawn(async move {
            let loaded = cache.load(Some(alias)).await.unwrap();
            assert_eq!(loaded.alias, alias);
            loaded.profile
        }));
    }
    let first = tasks.remove(0).await.unwrap();
    for task in tasks {
        let profile = task.await.unwrap();
        assert!(Arc::ptr_eq(&first, &profile));
    }
    assert_eq!(first.total_weight, 20_000);
}

use std::{fs, sync::Arc};

use prof_mcp::{cache::ProfileCache, registry};

fn register(workspace: &std::path::Path, alias: &str, frame: &str) {
    let source = workspace.join("input.folded");
    fs::write(&source, format!("root;{frame} 1\n")).unwrap();
    registry::register(workspace, &source, Some(alias), 1024 * 1024).unwrap();
}

#[test]
fn zero_capacity_is_rejected() {
    let workspace = tempfile::tempdir().unwrap();
    let error = ProfileCache::new(workspace.path().to_owned(), 1024, 0)
        .err()
        .expect("zero capacity must be rejected");
    assert_eq!(error.code, "invalid_budget");
}

#[tokio::test]
async fn cache_hit_promotes_profile_before_capacity_eviction() {
    let workspace = tempfile::tempdir().unwrap();
    for alias in ["one", "two", "three"] {
        register(workspace.path(), alias, alias);
    }
    let cache = ProfileCache::new(workspace.path().to_owned(), 1024, 2).unwrap();
    let one = cache.load(Some("one")).await.unwrap();
    let two = cache.load(Some("two")).await.unwrap();
    assert!(Arc::ptr_eq(
        &one.profile,
        &cache.load(Some("one")).await.unwrap().profile
    ));
    let three = cache.load(Some("three")).await.unwrap();
    assert!(Arc::ptr_eq(
        &one.profile,
        &cache.load(Some("one")).await.unwrap().profile
    ));
    assert!(Arc::ptr_eq(
        &three.profile,
        &cache.load(Some("three")).await.unwrap().profile
    ));
    assert!(!Arc::ptr_eq(
        &two.profile,
        &cache.load(Some("two")).await.unwrap().profile
    ));
}

#[tokio::test]
async fn byte_budget_evicts_multiple_profiles_and_skips_oversized_admission() {
    let workspace = tempfile::tempdir().unwrap();
    for alias in ["one", "two", "six"] {
        register(workspace.path(), alias, alias);
    }
    let probe = ProfileCache::new(workspace.path().to_owned(), 1024 * 1024, 8).unwrap();
    let small_bytes = probe
        .load(Some("one"))
        .await
        .unwrap()
        .profile
        .estimated_size_bytes();
    let budget = 3 * small_bytes;
    register(workspace.path(), "large", &"x".repeat(2 * small_bytes));
    register(workspace.path(), "oversized", &"x".repeat(4 * small_bytes));
    let large_bytes = probe
        .load(Some("large"))
        .await
        .unwrap()
        .profile
        .estimated_size_bytes();
    assert!(large_bytes > 2 * small_bytes && large_bytes <= budget);
    assert!(
        probe
            .load(Some("oversized"))
            .await
            .unwrap()
            .profile
            .estimated_size_bytes()
            > budget
    );
    let cache = ProfileCache::new(workspace.path().to_owned(), 1024 * 1024, 8)
        .unwrap()
        .with_byte_budget(budget);
    let one = cache.load(Some("one")).await.unwrap();
    let two = cache.load(Some("two")).await.unwrap();
    let six = cache.load(Some("six")).await.unwrap();
    let large = cache.load(Some("large")).await.unwrap();
    let oversized = cache.load(Some("oversized")).await.unwrap();
    assert!(!Arc::ptr_eq(
        &oversized.profile,
        &cache.load(Some("oversized")).await.unwrap().profile
    ));
    assert!(Arc::ptr_eq(
        &large.profile,
        &cache.load(Some("large")).await.unwrap().profile
    ));
    for (alias, original) in [("six", six), ("two", two), ("one", one)] {
        assert!(!Arc::ptr_eq(
            &original.profile,
            &cache.load(Some(alias)).await.unwrap().profile
        ));
    }
}

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
    assert_eq!(replaced.profile.total_weight(), 7);
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
    assert_eq!(first.total_weight(), 20_000);
}

#[tokio::test]
async fn zero_byte_budget_disables_cache() {
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    fs::write(&source, "root;leaf 1\n").unwrap();
    registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
    let cache = ProfileCache::new(workspace.path().to_owned(), 1024, 2)
        .unwrap()
        .with_byte_budget(0);

    let first = cache.load(None).await.unwrap();
    let second = cache.load(None).await.unwrap();
    assert!(!Arc::ptr_eq(&first.profile, &second.profile));
}

#[tokio::test]
async fn warm_cache_rejects_same_size_blob_changes() {
    use std::time::{Duration, UNIX_EPOCH};
    for change in ["submillisecond", "preserved_mtime", "replaced_inode"] {
        let workspace = tempfile::tempdir().unwrap();
        let source = workspace.path().join("input.folded");
        fs::write(&source, "root;good 1\n").unwrap();
        registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
        let blob = registry::resolve(workspace.path(), None).unwrap().path;
        let stamp = UNIX_EPOCH + Duration::new(1_800_000_000, 100_100);
        fs::File::open(&blob).unwrap().set_modified(stamp).unwrap();
        let cache = ProfileCache::new(workspace.path().to_owned(), 1024, 2).unwrap();
        cache.load(None).await.unwrap();
        let destination = if change == "replaced_inode" {
            source.as_path()
        } else {
            blob.as_path()
        };
        fs::write(destination, "root;evil 1\n").unwrap();
        let modified = if change == "submillisecond" {
            stamp + Duration::from_nanos(100)
        } else {
            stamp
        };
        fs::File::open(destination)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        if change == "replaced_inode" {
            fs::rename(&source, &blob).unwrap();
        }
        assert_eq!(
            cache.load(None).await.unwrap_err().code,
            "registry_corrupt",
            "{change}"
        );
    }
}

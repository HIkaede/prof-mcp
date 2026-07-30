use std::fs;

use prof_mcp::{cache::ProfileCache, registry};
use tempfile::tempdir;

#[tokio::test]
async fn registry_resolves_only_registered_aliases_and_rejects_corruption() {
    let workspace = tempdir().unwrap();
    let source = workspace.path().join("ok.folded");
    fs::write(&source, "root;safe 1\n").unwrap();
    registry::register(workspace.path(), &source, Some("ok"), 1024).unwrap();
    let cache = ProfileCache::new(workspace.path().to_owned(), 1024, 2).unwrap();
    assert_eq!(cache.load(None).await.unwrap().alias, "ok");
    assert_eq!(
        cache.load(Some("missing")).await.unwrap_err().code,
        "profile_alias_not_found"
    );
    assert_eq!(
        cache.load(Some("../../bad")).await.unwrap_err().code,
        "invalid_profile_alias"
    );

    let manifest = workspace.path().join(".prof-mcp/manifest.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    value["profiles"]["ok"]["file"] = serde_json::json!("../escape.folded");
    fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(cache.load(None).await.unwrap_err().code, "registry_corrupt");
}

#[tokio::test]
async fn missing_workspace_is_a_business_error_not_startup_failure() {
    // Discovery walks ancestors, so Linux tmpfs keeps this test independent
    // from an unrelated developer registry under `/tmp`.
    #[cfg(target_os = "linux")]
    let workspace = tempfile::tempdir_in("/dev/shm").unwrap();
    #[cfg(not(target_os = "linux"))]
    let workspace = tempdir().unwrap();
    let cache = ProfileCache::new(workspace.path().to_owned(), 1024, 2).unwrap();
    assert_eq!(
        cache.load(None).await.unwrap_err().code,
        "workspace_not_registered"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn non_utf8_workspace_path_returns_json_safe_profile_metadata() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let parent = tempdir().unwrap();
    let workspace = parent
        .path()
        .join(OsString::from_vec(b"\xff-workspace".to_vec()));
    fs::create_dir(&workspace).unwrap();
    let source = workspace.join("input.folded");
    fs::write(&source, "root;safe 1\n").unwrap();
    registry::register(&workspace, &source, Some("safe"), 1024).unwrap();
    let cache = ProfileCache::new(workspace, 1024, 2).unwrap();
    let loaded = cache.load(None).await.unwrap();
    let summary = prof_mcp::query::summary(&loaded.profile);
    assert_eq!(summary["schema_version"], "2");
    assert!(summary["profile"]["canonical_path"].is_string());
}

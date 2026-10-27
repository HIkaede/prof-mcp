mod support;

use std::fs;

use prof_mcp::{cache::ProfileCache, registry};
use tempfile::tempdir;

#[tokio::test]
async fn resolve_aliases_safely() {
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
    let original: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    assert_eq!(original["schema_version"], 2);
    for (field, malicious) in [
        ("file", "../escape.folded"),
        ("fingerprint", "../escape"),
        ("fingerprint", "/tmp/escape"),
        ("fingerprint", &"A".repeat(64)),
        ("source_name", "../escape.folded"),
    ] {
        let mut value = original.clone();
        value["profiles"]["ok"][field] = serde_json::json!(malicious);
        fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(cache.load(None).await.unwrap_err().code, "registry_corrupt");
    }
    fs::write(&manifest, serde_json::to_vec(&original).unwrap()).unwrap();
    assert_eq!(cache.load(None).await.unwrap().alias, "ok");
}

#[tokio::test]
async fn missing_workspace_error() {
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
async fn non_utf8_workspace_metadata() {
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
    let summary = support::json(prof_mcp::query::summary(&loaded.profile).unwrap());
    assert_eq!(summary["schema_version"], "3");
    assert_eq!(
        summary["profile"]["fingerprint"],
        loaded.profile.fingerprint()
    );
}

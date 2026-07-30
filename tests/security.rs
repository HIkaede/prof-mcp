use std::fs;

use rmdb_prof_mcp::cache::ProfileCache;
use tempfile::tempdir;

#[tokio::test]
async fn cache_enforces_root_regular_file_and_size() {
    assert_eq!(
        ProfileCache::new(Vec::new(), 1024, 2).err().unwrap().code,
        "invalid_budget"
    );
    let root = tempdir().unwrap();
    fs::write(root.path().join("ok.folded"), "root;safe 1\n").unwrap();
    fs::create_dir(root.path().join("directory")).unwrap();
    let cache = ProfileCache::new(vec![root.path().to_owned()], 1024, 2).unwrap();
    assert_eq!(cache.load("ok.folded").await.unwrap().total_weight, 1);
    assert_eq!(
        cache.load("directory").await.unwrap_err().code,
        "not_a_regular_file"
    );
    assert_eq!(
        cache.load("../outside.folded").await.unwrap_err().code,
        "profile_not_found"
    );
    let tiny = ProfileCache::new(vec![root.path().to_owned()], 2, 2).unwrap();
    assert_eq!(
        tiny.load("ok.folded").await.unwrap_err().code,
        "profile_too_large"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_escape_is_rejected() {
    use std::os::unix::fs::symlink;
    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    fs::write(outside.path().join("outside.folded"), "root;outside 1\n").unwrap();
    symlink(
        outside.path().join("outside.folded"),
        root.path().join("escape.folded"),
    )
    .unwrap();
    let cache = ProfileCache::new(vec![root.path().to_owned()], 1024, 2).unwrap();
    assert_eq!(
        cache.load("escape.folded").await.unwrap_err().code,
        "path_outside_root"
    );
}

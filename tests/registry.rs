use std::fs;

use prof_mcp::registry::{self, Manifest};
use tempfile::tempdir;

#[test]
fn registration_copies_exact_bytes_deduplicates_and_replaces_active_alias() {
    let workspace = tempdir().unwrap();
    let source = workspace.path().join("first.folded");
    let input = b"root;A 3\r\nroot;B 4\n";
    fs::write(&source, input).unwrap();
    let first = registry::register(workspace.path(), &source, None, 1024).unwrap();
    assert_eq!(first.alias, "first");
    assert_eq!(fs::read(&source).unwrap(), input);
    let registered = workspace
        .path()
        .join(".prof-mcp/profiles")
        .join(format!("{}.folded", first.fingerprint));
    assert_eq!(fs::read(&registered).unwrap(), input);
    assert_eq!(
        fs::read_to_string(workspace.path().join(".prof-mcp/.gitignore")).unwrap(),
        "# prof-mcp data files are local to this workspace.\n# Keep this file visible so the registry directory can be intentionally ignored.\n*\n!.gitignore\n"
    );

    let second = registry::register(workspace.path(), &source, Some("candidate"), 1024).unwrap();
    assert_eq!(second.fingerprint, first.fingerprint);
    let manifest: Manifest = serde_json::from_slice(
        &fs::read(workspace.path().join(".prof-mcp/manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest.active, "candidate");
    assert_eq!(manifest.profiles.len(), 2);
    assert_eq!(manifest.profiles["candidate"].source_name, "first.folded");
    assert_eq!(
        manifest.profiles["candidate"].file,
        format!("profiles/{}.folded", first.fingerprint)
    );

    fs::write(&source, b"root;new 1\n").unwrap();
    let replacement =
        registry::register(workspace.path(), &source, Some("candidate"), 1024).unwrap();
    let manifest: Manifest = serde_json::from_slice(
        &fs::read(workspace.path().join(".prof-mcp/manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        manifest.profiles["candidate"].fingerprint,
        replacement.fingerprint
    );
    assert_ne!(replacement.fingerprint, first.fingerprint);
    let status = registry::status(workspace.path()).unwrap();
    assert_eq!(status.active, "candidate");
    assert_eq!(status.profiles.len(), 2);
    let switched = registry::set_active(workspace.path(), "first").unwrap();
    assert_eq!(switched.active, "first");
    assert_eq!(
        registry::resolve(workspace.path(), None).unwrap().alias,
        "first"
    );
    assert_eq!(
        registry::set_active(workspace.path(), "missing")
            .unwrap_err()
            .code,
        "profile_alias_not_found"
    );
}

#[test]
fn registration_rejects_invalid_alias_regular_file_and_size_without_manifest() {
    let workspace = tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    fs::write(&source, "root;A 1\n").unwrap();
    assert_eq!(
        registry::register(workspace.path(), &source, Some("bad/slash"), 1024)
            .unwrap_err()
            .code,
        "invalid_profile_alias"
    );
    assert_eq!(
        registry::register(workspace.path(), workspace.path(), Some("dir"), 1024)
            .unwrap_err()
            .code,
        "not_a_regular_file"
    );
    assert_eq!(
        registry::register(workspace.path(), &source, Some("big"), 1)
            .unwrap_err()
            .code,
        "profile_too_large"
    );
    assert!(!workspace.path().join(".prof-mcp/manifest.json").exists());
}

#[test]
fn manifest_unknown_fields_and_escape_paths_are_rejected() {
    let workspace = tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    fs::write(&source, "root;A 1\n").unwrap();
    registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
    let manifest = workspace.path().join(".prof-mcp/manifest.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    value["unexpected"] = serde_json::json!(true);
    fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(
        registry::resolve(workspace.path(), None).unwrap_err().code,
        "registry_corrupt"
    );
}

#[test]
fn manifest_absolute_and_parent_profile_paths_are_rejected() {
    let workspace = tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    fs::write(&source, "root;A 1\n").unwrap();
    registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
    let manifest = workspace.path().join(".prof-mcp/manifest.json");
    let valid_manifest = fs::read(&manifest).unwrap();
    for escaped in ["../outside.folded", "/tmp/outside.folded"] {
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
        value["profiles"]["base"]["file"] = serde_json::json!(escaped);
        fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            registry::resolve(workspace.path(), None).unwrap_err().code,
            "registry_corrupt"
        );
        fs::write(&manifest, &valid_manifest).unwrap();
    }
}

#[test]
fn discovery_finds_root_from_descendant() {
    let workspace = tempdir().unwrap();
    let child = workspace.path().join("a/b");
    fs::create_dir_all(&child).unwrap();
    let source = workspace.path().join("input.folded");
    fs::write(&source, "root;A 1\n").unwrap();
    registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
    assert_eq!(registry::resolve(&child, None).unwrap().alias, "base");
}

#[test]
fn registry_busy_is_recoverable_for_api_and_cli_with_persistent_advisory_lock() {
    use std::fs::OpenOptions;
    use std::process::Command;

    use fs2::FileExt;

    let workspace = tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    fs::write(&source, "root;A 1\n").unwrap();
    registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
    let lock = workspace.path().join(".prof-mcp/.register.lock");
    assert!(lock.exists());
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock)
        .unwrap();
    lock_file.lock_exclusive().unwrap();
    assert_eq!(
        registry::register(workspace.path(), &source, Some("api"), 1024)
            .unwrap_err()
            .code,
        "registry_busy"
    );
    let output = Command::new(env!("CARGO_BIN_EXE_prof-mcp"))
        .current_dir(workspace.path())
        .arg(&source)
        .arg("--name")
        .arg("cli")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Another registry operation"));
    drop(lock_file);
    registry::register(workspace.path(), &source, Some("api"), 1024).unwrap();
    assert!(lock.exists());
}

#[test]
fn malformed_and_invalid_manifest_matrix_is_rejected_without_publishing_or_leaking_lock() {
    let workspace = tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    let bytes = b"root;A 1\n";
    fs::write(&source, bytes).unwrap();
    registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
    let manifest_path = workspace.path().join(".prof-mcp/manifest.json");
    let valid_manifest = fs::read(&manifest_path).unwrap();
    let mut variants: Vec<serde_json::Value> = Vec::new();
    let valid: serde_json::Value = serde_json::from_slice(&valid_manifest).unwrap();
    let mutations: [fn(&mut serde_json::Value); 4] = [
        |value: &mut serde_json::Value| value["schema_version"] = serde_json::json!(2),
        |value: &mut serde_json::Value| value["active"] = serde_json::json!("missing"),
        |value: &mut serde_json::Value| {
            value["profiles"]["base"]["fingerprint"] = serde_json::json!("not-a-fingerprint")
        },
        |value: &mut serde_json::Value| {
            value["profiles"]["base"]["byte_len"] = serde_json::json!(999)
        },
    ];
    for mutate in mutations {
        let mut value = valid.clone();
        mutate(&mut value);
        variants.push(value);
    }
    for value in variants {
        fs::write(&manifest_path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(
            registry::resolve(workspace.path(), None).unwrap_err().code,
            "registry_corrupt"
        );
    }

    let before_profiles = fs::read_dir(workspace.path().join(".prof-mcp/profiles"))
        .unwrap()
        .count();
    let failed_source = workspace.path().join("new.folded");
    let failed_bytes = b"root;new 2\n";
    fs::write(&failed_source, failed_bytes).unwrap();
    fs::write(&manifest_path, b"{").unwrap();
    assert_eq!(
        registry::register(workspace.path(), &failed_source, Some("new"), 1024)
            .unwrap_err()
            .code,
        "registry_corrupt"
    );
    assert_eq!(fs::read(&manifest_path).unwrap(), b"{");
    assert_eq!(
        fs::read_dir(workspace.path().join(".prof-mcp/profiles"))
            .unwrap()
            .count(),
        before_profiles
    );
    assert!(workspace.path().join(".prof-mcp/.register.lock").exists());
    assert!(
        fs::read_dir(workspace.path().join(".prof-mcp/profiles"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp"))
    );
    assert_eq!(fs::read(&source).unwrap(), bytes);
    assert_eq!(fs::read(&failed_source).unwrap(), failed_bytes);
}

#[test]
fn gc_only_removes_unreferenced_regular_profile_blobs_and_supports_dry_run() {
    let workspace = tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    fs::write(&source, "root;A 1\n").unwrap();
    registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
    fs::write(&source, "root;B 2\n").unwrap();
    let old_candidate =
        registry::register(workspace.path(), &source, Some("candidate"), 1024).unwrap();
    fs::write(&source, "root;C 3\n").unwrap();
    registry::register(workspace.path(), &source, Some("candidate"), 1024).unwrap();

    let profiles = workspace.path().join(".prof-mcp/profiles");
    let manual_orphan = format!("{}.folded", "f".repeat(64));
    fs::write(profiles.join(&manual_orphan), "orphan").unwrap();
    #[cfg(unix)]
    let symlink_orphan = {
        use std::os::unix::fs::symlink;

        let name = format!("{}.folded", "e".repeat(64));
        let target = workspace.path().join("outside-profile-target");
        fs::write(&target, "outside").unwrap();
        symlink(&target, profiles.join(&name)).unwrap();
        (name, target)
    };
    fs::write(profiles.join("notes.txt"), "leave me").unwrap();
    fs::create_dir(profiles.join("directory")).unwrap();
    let manifest = fs::read(workspace.path().join(".prof-mcp/manifest.json")).unwrap();

    let dry = registry::gc(workspace.path(), true).unwrap();
    assert!(dry.dry_run);
    assert_eq!(
        dry.removed,
        vec![
            format!("profiles/{}.folded", old_candidate.fingerprint),
            format!("profiles/{manual_orphan}"),
        ]
    );
    assert!(dry.skipped.contains(&"profiles/directory".to_string()));
    assert!(dry.skipped.contains(&"profiles/notes.txt".to_string()));
    #[cfg(unix)]
    assert!(
        dry.skipped
            .contains(&format!("profiles/{}", symlink_orphan.0))
    );
    assert!(profiles.join(&manual_orphan).exists());
    assert_eq!(
        fs::read(workspace.path().join(".prof-mcp/manifest.json")).unwrap(),
        manifest
    );

    let applied = registry::gc(workspace.path(), false).unwrap();
    assert!(!applied.dry_run);
    assert_eq!(applied.removed, dry.removed);
    assert!(!profiles.join(&manual_orphan).exists());
    assert!(
        !profiles
            .join(format!("{}.folded", old_candidate.fingerprint))
            .exists()
    );
    assert!(profiles.join("notes.txt").exists());
    #[cfg(unix)]
    {
        assert!(profiles.join(&symlink_orphan.0).is_symlink());
        assert_eq!(fs::read(&symlink_orphan.1).unwrap(), b"outside");
    }
    assert_eq!(
        fs::read(workspace.path().join(".prof-mcp/manifest.json")).unwrap(),
        manifest
    );
}

#[cfg(unix)]
#[test]
fn registry_storage_symlinks_are_rejected_without_writing_outside_workspace() {
    use std::os::unix::fs::symlink;

    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    fs::write(&source, "root;A 1\n").unwrap();

    symlink(outside.path(), workspace.path().join(".prof-mcp")).unwrap();
    assert_eq!(
        registry::register(workspace.path(), &source, Some("base"), 1024)
            .unwrap_err()
            .code,
        "registry_corrupt"
    );
    assert!(!outside.path().join("profiles").exists());
    fs::remove_file(workspace.path().join(".prof-mcp")).unwrap();

    fs::create_dir(workspace.path().join(".prof-mcp")).unwrap();
    symlink(outside.path(), workspace.path().join(".prof-mcp/profiles")).unwrap();
    assert_eq!(
        registry::register(workspace.path(), &source, Some("base"), 1024)
            .unwrap_err()
            .code,
        "registry_corrupt"
    );
    assert!(!outside.path().join("manifest.json").exists());
}

#[cfg(unix)]
#[test]
fn registered_profile_file_symlink_is_rejected() {
    use std::os::unix::fs::symlink;

    let workspace = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    fs::write(&source, "root;A 1\n").unwrap();
    let registered = registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
    let file = workspace
        .path()
        .join(".prof-mcp/profiles")
        .join(format!("{}.folded", registered.fingerprint));
    let outside_file = outside.path().join("outside.folded");
    fs::write(&outside_file, "root;outside 1\n").unwrap();
    fs::remove_file(&file).unwrap();
    symlink(&outside_file, &file).unwrap();
    assert_eq!(
        registry::resolve(workspace.path(), None).unwrap_err().code,
        "registry_corrupt"
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_profile_basename_registers_as_default_via_api_and_cli() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, process::Command};

    let api_workspace = tempdir().unwrap();
    let source = api_workspace
        .path()
        .join(OsString::from_vec(b"\xff.folded".to_vec()));
    let bytes = b"root;safe 1\n";
    fs::write(&source, bytes).unwrap();
    let registration = registry::register(api_workspace.path(), &source, None, 1024).unwrap();
    assert_eq!(registration.alias, "default");
    assert_eq!(fs::read(&source).unwrap(), bytes);
    let manifest: Manifest = serde_json::from_slice(
        &fs::read(api_workspace.path().join(".prof-mcp/manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest.active, "default");
    assert!(!manifest.profiles["default"].source_name.is_empty());

    let cli_workspace = tempdir().unwrap();
    let cli_source = cli_workspace
        .path()
        .join(OsString::from_vec(b"\xfe.folded".to_vec()));
    fs::write(&cli_source, bytes).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_prof-mcp"))
        .current_dir(cli_workspace.path())
        .arg(&cli_source)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("alias=default"));
    assert_eq!(fs::read(&cli_source).unwrap(), bytes);
}

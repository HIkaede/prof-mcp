use std::fs;

use prof_mcp::registry::{self, Manifest};
use tempfile::tempdir;

#[test]
fn register_and_deduplicate() {
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
fn validate_registration() {
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
fn reject_invalid_manifest_fields() {
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
fn reject_escaping_profile_paths() {
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
fn recover_lock_contention() {
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
fn reject_invalid_manifests() {
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
    fs::write(&manifest_path, valid_manifest).unwrap();
    registry::register(workspace.path(), &failed_source, Some("new"), 1024).unwrap();
    assert_eq!(
        registry::resolve(workspace.path(), None).unwrap().alias,
        "new"
    );
}

#[test]
fn gc_unreferenced_blobs() {
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
fn reject_registry_symlinks() {
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
fn reject_profile_symlink() {
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
fn register_non_utf8_basename() {
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

#[test]
fn remove_alias_preserves_blob() {
    let workspace = tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    fs::write(&source, "root;A 1\n").unwrap();
    let first = registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
    registry::register(workspace.path(), &source, Some("candidate"), 1024).unwrap();

    let removal = registry::remove(workspace.path(), "base", None).unwrap();
    assert_eq!(removal.alias, "base");
    assert_eq!(removal.active, "candidate");
    assert_eq!(
        registry::resolve(workspace.path(), None).unwrap().alias,
        "candidate"
    );
    assert!(
        workspace
            .path()
            .join(format!(".prof-mcp/profiles/{}.folded", first.fingerprint))
            .exists()
    );
    assert_eq!(
        registry::remove(workspace.path(), "candidate", None)
            .unwrap_err()
            .code,
        "cannot_remove_last_alias"
    );
}

#[test]
fn validate_active_alias_removal() {
    let workspace = tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    fs::write(&source, "root;A 1\n").unwrap();
    registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
    assert_eq!(
        registry::remove(workspace.path(), "base", None)
            .unwrap_err()
            .code,
        "cannot_remove_last_alias"
    );

    fs::write(&source, "root;B 2\n").unwrap();
    registry::register(workspace.path(), &source, Some("candidate"), 1024).unwrap();
    assert_eq!(
        registry::remove(workspace.path(), "candidate", None)
            .unwrap_err()
            .code,
        "active_alias_requires_replacement"
    );
    let removal = registry::remove(workspace.path(), "candidate", Some("base")).unwrap();
    assert_eq!(removal.active, "base");
    assert_eq!(registry::status(workspace.path()).unwrap().active, "base");
}

#[test]
fn reader_matches_file() {
    use std::io::Cursor;
    let file_root = tempdir().unwrap();
    let stream_root = tempdir().unwrap();
    let bytes = "root;函数 4\r\nroot;函数 6".as_bytes();
    let source = file_root.path().join("stdin.folded");
    fs::write(&source, bytes).unwrap();
    let file = registry::register(file_root.path(), &source, Some("base"), 1024).unwrap();
    let stream = registry::register_reader(
        stream_root.path(),
        Cursor::new(bytes),
        "stdin.folded",
        Some("base"),
        1024,
    )
    .unwrap();
    assert_eq!(file.fingerprint, stream.fingerprint);
    assert_eq!(file.byte_len, stream.byte_len);
    let builder = prof_mcp::profile::ProfileBuilder::new(Default::default());
    let parse = |root| {
        let resolved = registry::resolve(root, None).unwrap();
        builder
            .from_file(resolved.path, resolved.byte_len, None)
            .unwrap()
    };
    assert_eq!(
        prof_mcp::query::summary(&parse(file_root.path())),
        prof_mcp::query::summary(&parse(stream_root.path()))
    );
    for root in [file_root.path(), stream_root.path()] {
        let resolved = registry::resolve(root, None).unwrap();
        assert_eq!(resolved.alias, "base");
        assert_eq!(fs::read(resolved.path).unwrap(), bytes);
    }
    let mut left: serde_json::Value = serde_json::from_slice(
        &fs::read(file_root.path().join(".prof-mcp/manifest.json")).unwrap(),
    )
    .unwrap();
    let mut right: serde_json::Value = serde_json::from_slice(
        &fs::read(stream_root.path().join(".prof-mcp/manifest.json")).unwrap(),
    )
    .unwrap();
    left["profiles"]["base"]
        .as_object_mut()
        .unwrap()
        .remove("registered_unix_ms");
    right["profiles"]["base"]
        .as_object_mut()
        .unwrap()
        .remove("registered_unix_ms");
    assert_eq!(left, right);
}

#[test]
fn reader_failure_is_atomic() {
    use std::io::{self, Cursor, Read};
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("injected read error"))
        }
    }
    let root = tempdir().unwrap();
    registry::register_reader(
        root.path(),
        Cursor::new(b"root;good 1\n"),
        "stdin.folded",
        Some("base"),
        64,
    )
    .unwrap();
    let manifest = root.path().join(".prof-mcp/manifest.json");
    let before = fs::read(&manifest).unwrap();
    for bytes in [
        &b"invalid"[..],
        &b"root;bad 0\n"[..],
        &b"root;\xff 1\n"[..],
        &b"root;large 123456\n"[..],
    ] {
        assert!(
            registry::register_reader(
                root.path(),
                Cursor::new(bytes),
                "stdin.folded",
                Some("base"),
                16
            )
            .is_err()
        );
        assert_eq!(fs::read(&manifest).unwrap(), before);
    }
    assert!(
        registry::register_reader(root.path(), Broken, "stdin.folded", Some("base"), 64).is_err()
    );
    assert!(
        registry::register_reader(
            root.path(),
            Cursor::new(b"root 1\n"),
            "../escape",
            Some("base"),
            64
        )
        .is_err()
    );
    assert_eq!(fs::read(&manifest).unwrap(), before);
    assert_eq!(
        fs::read_dir(root.path().join(".prof-mcp/profiles"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn reader_stops_at_limit() {
    use std::io::{self, Read};
    struct Endless(usize);
    impl Read for Endless {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            buffer.fill(b'x');
            self.0 += buffer.len();
            Ok(buffer.len())
        }
    }
    let root = tempdir().unwrap();
    let mut input = Endless(0);
    let error =
        registry::register_reader(root.path(), &mut input, "stdin.folded", None, 64).unwrap_err();
    assert_eq!(error.code, "profile_too_large");
    assert_eq!(input.0, 65);
    assert!(!root.path().join(".prof-mcp").exists());
}

#[test]
fn persistence_failure_is_atomic() {
    let root = tempdir().unwrap();
    let original = b"root;good 1\n";
    registry::register_reader(
        root.path(),
        &original[..],
        "stdin.folded",
        Some("base"),
        1024,
    )
    .unwrap();
    let state = root.path().join(".prof-mcp");
    let manifest = state.join("manifest.json");
    let before = fs::read(&manifest).unwrap();
    let replacement = b"root;new 2\n";
    let fingerprint = blake3::hash(replacement).to_hex().to_string();
    let blob = state.join(format!("profiles/{fingerprint}.folded"));
    for destination in [&blob, &manifest] {
        let blocker = destination.parent().unwrap().join(format!(
            ".{}.{}.tmp",
            destination.file_name().unwrap().to_str().unwrap(),
            std::process::id()
        ));
        fs::create_dir(&blocker).unwrap();
        let result = registry::register_reader(
            root.path(),
            &replacement[..],
            "stdin.folded",
            Some("new"),
            1024,
        );
        assert!(result.is_err());
        assert_eq!(fs::read(&manifest).unwrap(), before);
        assert!(!blob.exists());
        assert_eq!(registry::resolve(root.path(), None).unwrap().alias, "base");
        fs::remove_dir(blocker).unwrap();
    }
    registry::register_reader(
        root.path(),
        &replacement[..],
        "stdin.folded",
        Some("new"),
        1024,
    )
    .unwrap();
    assert_eq!(registry::resolve(root.path(), None).unwrap().alias, "new");
}

#[test]
fn reject_corrupt_deduplication() {
    let root = tempdir().unwrap();
    let bytes = b"root;good 1\n";
    registry::register_reader(root.path(), &bytes[..], "stdin.folded", Some("base"), 1024).unwrap();
    let resolved = registry::resolve(root.path(), None).unwrap();
    let manifest = root.path().join(".prof-mcp/manifest.json");
    let before = fs::read(&manifest).unwrap();
    for corruption in [&b"root;evil 1\n"[..], &b"short"[..]] {
        fs::write(&resolved.path, corruption).unwrap();
        assert_eq!(
            registry::register_reader(root.path(), &bytes[..], "stdin.folded", Some("new"), 1024)
                .unwrap_err()
                .code,
            "registry_corrupt"
        );
        assert_eq!(fs::read(&manifest).unwrap(), before);
        assert_eq!(fs::read(&resolved.path).unwrap(), corruption);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn register_with_bounded_memory() {
    use std::io::{BufWriter, Write};
    use std::process::Command;

    let root = tempdir().unwrap();
    let source = root.path().join("large.folded");
    let mut writer = BufWriter::new(fs::File::create(&source).unwrap());
    let mut line = [b' '; 8192];
    line[..11].copy_from_slice(b"root;leaf 1");
    line[8191] = b'\n';
    for _ in 0..12288 {
        writer.write_all(&line).unwrap();
    }
    writer.flush().unwrap();
    drop(writer);
    // A 96 MiB input must fit a 64 MiB address-space limit, including deduplication.
    for _ in 0..2 {
        let output = Command::new("bash")
            .args([
                "-c",
                "ulimit -v 65536; exec \"$1\" register \"$2\" --name base",
                "register-test",
            ])
            .arg(env!("CARGO_BIN_EXE_prof-mcp"))
            .arg(&source)
            .current_dir(root.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let resolved = registry::resolve(root.path(), None).unwrap();
    assert_eq!(resolved.byte_len, 96 * 1024 * 1024);
    let parsed = prof_mcp::profile::ProfileBuilder::new(Default::default())
        .from_file(resolved.path, resolved.byte_len, None)
        .unwrap();
    assert_eq!(parsed.total_weight, 12288);
    assert_eq!(parsed.stacks.len(), 1);
}

#[test]
fn manifest_byte_limit_preserves_registry() {
    const LIMIT: usize = 4 * 1024 * 1024;
    let workspace = tempdir().unwrap();
    registry::register_reader(
        workspace.path(),
        &b"root;old 1\n"[..],
        "old.folded",
        Some("old"),
        1024,
    )
    .unwrap();
    let path = workspace.path().join(".prof-mcp/manifest.json");
    let original = fs::read(&path).unwrap();
    let mut padded = original.clone();
    padded.resize(LIMIT, b' ');
    fs::write(&path, &padded).unwrap();
    assert_eq!(
        registry::resolve(workspace.path(), None).unwrap().alias,
        "old"
    );
    padded.push(b' ');
    fs::write(&path, &padded).unwrap();
    assert_eq!(
        registry::status(workspace.path()).unwrap_err().code,
        "registry_too_large"
    );
    assert_eq!(
        registry::resolve(workspace.path(), None).unwrap_err().code,
        "registry_too_large"
    );
    fs::write(&path, &original).unwrap();
    let error = registry::register_reader(
        workspace.path(),
        &b"root;new 1\n"[..],
        &"x".repeat(LIMIT),
        Some("new"),
        1024,
    )
    .unwrap_err();
    assert_eq!(error.code, "registry_too_large");
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(registry::status(workspace.path()).unwrap().active, "old");
    assert_eq!(
        fs::read_dir(workspace.path().join(".prof-mcp/profiles"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn missing_alias_details_are_bounded() {
    let workspace = tempdir().unwrap();
    for index in 0..101 {
        registry::register_reader(
            workspace.path(),
            &b"root 1\n"[..],
            "input.folded",
            Some(&format!("p{index:03}")),
            1024,
        )
        .unwrap();
    }
    let error = registry::resolve(workspace.path(), Some("missing")).unwrap_err();
    assert_eq!(error.code, "profile_alias_not_found");
    assert_eq!(error.details["available"].as_array().unwrap().len(), 100);
    assert_eq!(error.details["available_count"], 101);
    assert_eq!(error.details["omitted"], 1);
    assert_eq!(error.details["available"][0], "p000");
}

use std::{fs, process::Command};

use tempfile::tempdir;

#[test]
fn manage_aliases() {
    let workspace = tempdir().unwrap();
    let first = workspace.path().join("first.folded");
    let second = workspace.path().join("second.folded");
    fs::write(&first, "root;A 1\n").unwrap();
    fs::write(&second, "root;B 2\n").unwrap();
    let binary = env!("CARGO_BIN_EXE_prof-mcp");

    let registered = Command::new(binary)
        .current_dir(workspace.path())
        .args(["register", first.to_str().unwrap(), "--name", "baseline"])
        .output()
        .unwrap();
    assert!(registered.status.success());
    let legacy = Command::new(binary)
        .current_dir(workspace.path())
        .arg(&second)
        .args(["--name", "candidate"])
        .output()
        .unwrap();
    assert!(legacy.status.success());

    let listed = Command::new(binary)
        .current_dir(workspace.path())
        .arg("list")
        .output()
        .unwrap();
    let listed = String::from_utf8(listed.stdout).unwrap();
    assert!(listed.contains("active=candidate"));
    assert!(listed.contains("baseline"));
    assert!(listed.contains("* candidate"));

    let selected = Command::new(binary)
        .current_dir(workspace.path())
        .args(["use", "baseline"])
        .output()
        .unwrap();
    assert!(selected.status.success());
    assert!(
        String::from_utf8(selected.stdout)
            .unwrap()
            .contains("active alias=baseline")
    );
    let removed = Command::new(binary)
        .current_dir(workspace.path())
        .args(["remove", "candidate"])
        .output()
        .unwrap();
    assert!(removed.status.success());
    assert!(String::from_utf8_lossy(&removed.stdout).contains("removed alias=candidate"));
    let gc = Command::new(binary)
        .current_dir(workspace.path())
        .args(["gc", "--dry-run"])
        .output()
        .unwrap();
    assert!(gc.status.success());
    let gc_stdout = String::from_utf8(gc.stdout).unwrap();
    assert!(gc_stdout.contains("dry_run=true"));
    assert!(gc_stdout.contains("registry="));
}

#[cfg(target_os = "linux")]
#[test]
fn capture_and_register() {
    use std::os::unix::fs::PermissionsExt;

    let workspace = tempdir().unwrap();
    let bin_dir = workspace.path().join("bin");
    fs::create_dir(&bin_dir).unwrap();
    let perf = bin_dir.join("perf");
    fs::write(
        &perf,
        "#!/bin/sh
if [ \"$1\" = record ]; then
  shift
  while [ \"$1\" != -o ]; do shift; done
  shift
  : > \"$1\"
  exit 0
fi
if [ \"$1\" = script ]; then
  printf 'captured 123 1.000: 7 cpu/cycles/P:\\n'
  printf '        7 leaf+0x4 (/bin/true)\\n'
  printf '        8 caller (/bin/true)\\n\\n'
  printf 'captured 123 2.000: 3 cpu/cycles/P:\\n'
  printf '        7 leaf+0x4 (/bin/true)\\n'
  printf '        8 caller (/bin/true)\\n'
  exit 0
fi
exit 2
",
    )
    .unwrap();
    fs::set_permissions(&perf, fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_prof-mcp"))
        .current_dir(workspace.path())
        .args(["capture", "--name", "captured", "--", "/bin/true"])
        .env("PATH", format!("{}:/usr/bin:/bin", bin_dir.display()))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("registered alias=captured"));
    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(workspace.path().join(".prof-mcp/manifest.json")).unwrap(),
    )
    .unwrap();
    assert!(manifest["profiles"]["captured"].is_object());
    let fingerprint = manifest["profiles"]["captured"]["fingerprint"]
        .as_str()
        .unwrap();
    let folded = fs::read_to_string(
        workspace
            .path()
            .join(".prof-mcp/profiles")
            .join(format!("{fingerprint}.folded")),
    )
    .unwrap();
    assert_eq!(folded, "captured;caller [/bin/true];leaf [/bin/true] 10\n");
}

#[test]
fn serve_flag_and_version() {
    let binary = env!("CARGO_BIN_EXE_prof-mcp");
    assert!(
        !Command::new(binary)
            .arg("serve")
            .status()
            .unwrap()
            .success()
    );
    let version = Command::new(binary).arg("--version").output().unwrap();
    assert!(
        String::from_utf8(version.stdout)
            .unwrap()
            .contains("prof-mcp 0.5.0")
    );
}

#[test]
fn plain_cli_diagnostics() {
    for args in [
        vec!["--help"],
        vec!["serve", "--help"],
        vec!["capture", "--help"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_prof-mcp"))
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
        assert!(!output.stdout.contains(&0x1b));
        assert!(output.stderr.is_empty());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_prof-mcp"))
        .args(["serve", "--unknown-option"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("unexpected argument"));
    assert!(error.contains("Usage:"));
    assert!(!output.stderr.contains(&0x1b));
}

#[cfg(unix)]
#[test]
fn explicit_setup() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let sandbox = tempdir().unwrap();
    let bin_dir = sandbox.path().join("bin");
    let codex_home = sandbox.path().join("codex-home");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let prof = bin_dir.join("prof-mcp");
    symlink(env!("CARGO_BIN_EXE_prof-mcp"), &prof).unwrap();
    let codex = bin_dir.join("codex");
    fs::write(
        &codex,
        "#!/bin/sh\nif [ \"$1 $2 $3\" = \"mcp list --json\" ]; then\n  if [ -f \"$CODEX_HOME/installed\" ]; then printf '[{\"name\":\"prof-mcp\",\"enabled\":true,\"transport\":{\"type\":\"stdio\",\"command\":\"prof-mcp\",\"args\":[\"serve\",\"--mcp\"],\"env\":null,\"env_vars\":[],\"cwd\":null},\"startup_timeout_sec\":null,\"tool_timeout_sec\":null}]\\n'; else printf '[]\\n'; fi\n  exit 0\nfi\nprintf '%s\\n' \"$*\" >> \"$CODEX_HOME/calls\"\nif [ \"$1 $2 $3\" = \"mcp add prof-mcp\" ]; then : > \"$CODEX_HOME/installed\"; fi\n",
    )
    .unwrap();
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o755)).unwrap();

    let personal = codex_home.join("AGENTS.md");
    fs::write(&personal, "# Personal guidance\n").unwrap();
    for _ in 0..2 {
        let output = Command::new(&prof)
            .arg("setup")
            .env("CODEX_HOME", &codex_home)
            .env("PATH", &bin_dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(
        fs::read_to_string(&personal).unwrap(),
        "# Personal guidance\n"
    );
    let calls = fs::read_to_string(codex_home.join("calls")).unwrap();
    assert!(calls.contains("mcp add prof-mcp -- prof-mcp serve --mcp"));
    assert_eq!(calls.lines().count(), 1);
}

#[cfg(unix)]
#[test]
fn propagate_codex_errors() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let sandbox = tempdir().unwrap();
    let bin_dir = sandbox.path().join("bin");
    let codex_home = sandbox.path().join("codex-home");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    let prof = bin_dir.join("prof-mcp");
    symlink(env!("CARGO_BIN_EXE_prof-mcp"), &prof).unwrap();
    let codex = bin_dir.join("codex");
    fs::write(
        &codex,
        "#!/bin/sh\necho 'configuration permission denied' >&2\nexit 2\n",
    )
    .unwrap();
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(&prof)
        .args(["setup", "--dry-run"])
        .env("CODEX_HOME", &codex_home)
        .env("PATH", &bin_dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("codex mcp list --json` failed"));
    assert!(!codex_home.join("AGENTS.md").exists());
}

#[cfg(unix)]
#[test]
fn restore_legacy_registration() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let sandbox = tempdir().unwrap();
    let bin_dir = sandbox.path().join("bin");
    let codex_home = sandbox.path().join("codex-home");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::create_dir_all(&codex_home).unwrap();
    fs::write(codex_home.join("AGENTS.md"), "# Personal guidance\n").unwrap();
    fs::write(codex_home.join("state"), "legacy").unwrap();
    let prof = bin_dir.join("prof-mcp");
    symlink(env!("CARGO_BIN_EXE_prof-mcp"), &prof).unwrap();
    let codex = bin_dir.join("codex");
    fs::write(
        &codex,
        r#"#!/bin/sh
if [ "$1 $2 $3" = "mcp list --json" ]; then
  if [ -f "$CODEX_HOME/state" ]; then printf '[{"name":"prof-mcp","enabled":true,"transport":{"type":"stdio","command":"prof-mcp","args":[],"env":null,"env_vars":[],"cwd":null},"startup_timeout_sec":null,"tool_timeout_sec":null}]\n'; else printf '[]\n'; fi
  exit 0
fi
if [ "$1 $2 $3" = "mcp remove prof-mcp" ]; then rm -f "$CODEX_HOME/state"; exit 0; fi
if [ "$1 $2 $3" = "mcp add prof-mcp" ]; then
  if [ "$6" = "serve" ]; then echo 'add failed' >&2; exit 9; fi
  : > "$CODEX_HOME/state"; exit 0
fi
exit 3
"#,
    )
    .unwrap();
    fs::set_permissions(&codex, fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(&prof)
        .arg("setup")
        .env("CODEX_HOME", &codex_home)
        .env("PATH", &bin_dir)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        codex_home.join("state").exists(),
        "legacy setup was restored"
    );
    assert_eq!(
        fs::read_to_string(codex_home.join("AGENTS.md")).unwrap(),
        "# Personal guidance\n"
    );
}

#[test]
fn no_arguments_show_help() {
    let workspace = tempdir().unwrap();
    let home = workspace.path().join("codex-home");
    let output = Command::new(env!("CARGO_BIN_EXE_prof-mcp"))
        .current_dir(workspace.path())
        .env("CODEX_HOME", &home)
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
    assert!(!home.exists());
    assert!(!workspace.path().join(".prof-mcp").exists());
}

#[test]
fn register_stdin() {
    use std::io::Write;
    use std::process::Stdio;
    let root = tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_prof-mcp"))
        .current_dir(root.path())
        .args(["register", "-", "--name", "base"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"root;leaf 7\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let resolved = prof_mcp::registry::resolve(root.path(), None).unwrap();
    assert_eq!(resolved.alias, "base");
    assert_eq!(fs::read(resolved.path).unwrap(), b"root;leaf 7\n");
}

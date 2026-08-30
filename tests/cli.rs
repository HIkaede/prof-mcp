use std::{fs, process::Command};

use tempfile::tempdir;

#[test]
fn register_list_use_and_legacy_registration_are_compatible() {
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
fn capture_runs_perf_pipeline_and_registers_folded_output() {
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
        .args([
            "capture",
            "--name",
            "captured",
            "--sample-period-us",
            "100",
            "--",
            "/bin/true",
        ])
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
    assert_eq!(manifest["profiles"]["captured"]["sample_period_us"], 100);
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
    assert_eq!(folded, "captured;caller;leaf 10\n");
}

#[test]
fn serve_requires_explicit_mcp_flag_and_version_is_available() {
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
            .contains("prof-mcp 0.4.0")
    );
}

#[cfg(unix)]
#[test]
fn direct_invocation_idempotently_installs_codex_mcp_and_agents_guidance() {
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

    for _ in 0..2 {
        let output = Command::new(&prof)
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
    let agents = fs::read_to_string(codex_home.join("AGENTS.md")).unwrap();
    assert_eq!(agents.matches("<!-- PROF_MCP_START -->").count(), 1);
    assert!(agents.contains("Do not infer optimization targets from inclusive weight alone."));
    let calls = fs::read_to_string(codex_home.join("calls")).unwrap();
    assert!(calls.contains("mcp add prof-mcp -- prof-mcp serve --mcp"));
    assert_eq!(calls.lines().count(), 1);
}

#[cfg(unix)]
#[test]
fn setup_does_not_treat_codex_errors_as_a_missing_registration() {
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
fn setup_rolls_back_legacy_mcp_before_leaving_agents_unchanged_on_add_failure() {
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

#![cfg(target_os = "linux")]

use prof_mcp::registry;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

struct Capture {
    root: tempfile::TempDir,
}

impl Capture {
    fn new(input: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("input"), input).unwrap();
        let bin = root.path().join("bin");
        fs::create_dir(&bin).unwrap();
        let perf = bin.join("perf");
        fs::write(
            &perf,
            r#"#!/bin/sh
if [ "$1" = record ]; then
  printf '%s\0' "$@" > "$FAKE_ROOT/record.argv"
  if [ "$FAKE_MODE" = record_failure ]; then exit 7; fi
  : > "$4"
  exit 0
fi
if [ "$1" = script ]; then
  printf '%s\0' "$@" > "$FAKE_ROOT/script.argv"
  if [ "$FAKE_MODE" = waiting ]; then
    printf '%s' "$$" > "$FAKE_ROOT/script.pid"
    printf 'invalid perf data\n'
    exec /bin/sleep 30
  fi
  if [ "$FAKE_MODE" != empty ]; then /bin/cat "$FAKE_ROOT/input"; fi
  if [ "$FAKE_MODE" = script_failure ]; then exit 9; fi
  exit 0
fi
exit 2
"#,
        )
        .unwrap();
        fs::set_permissions(perf, fs::Permissions::from_mode(0o755)).unwrap();
        Self { root }
    }

    fn run(&self, mode: &str, command: &[&str]) -> Output {
        let stdout = self.root.path().join("stdout");
        let stderr = self.root.path().join("stderr");
        let mut child = Command::new(env!("CARGO_BIN_EXE_prof-mcp"))
            .current_dir(self.root.path())
            .args([
                "--max-file-size-mib",
                "1",
                "capture",
                "--name",
                "candidate",
                "--",
            ])
            .args(command)
            .env("PATH", self.root.path().join("bin"))
            .env("FAKE_ROOT", self.root.path())
            .env("FAKE_MODE", mode)
            .stdout(Stdio::from(fs::File::create(&stdout).unwrap()))
            .stderr(Stdio::from(fs::File::create(&stderr).unwrap()))
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                // Clean up the fake perf before the CLI, including on the RED
                // version that waits forever instead of rejecting malformed input.
                if let Ok(pid) = fs::read_to_string(self.root.path().join("script.pid")) {
                    let _ = Command::new("/bin/kill")
                        .args(["-KILL", pid.trim()])
                        .status();
                }
                let _ = child.kill();
                let _ = child.wait();
                panic!("capture timed out in {mode}");
            }
            thread::sleep(Duration::from_millis(10));
        };
        Output {
            status,
            stdout: fs::read(stdout).unwrap(),
            stderr: fs::read(stderr).unwrap(),
        }
    }

    fn argv(&self, name: &str) -> Vec<String> {
        fs::read(self.root.path().join(name))
            .unwrap()
            .split(|byte| *byte == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| String::from_utf8(arg.to_vec()).unwrap())
            .collect()
    }
}

const VALID: &str = "worker 12 1.0: 2 cycles:\n  7 overload(int)+0x4 (/tmp/a)\n\nworker 12 2.0: 3 cycles:\n  7 overload(double) (/tmp/a)\n";

#[test]
fn capture_argv_and_overloads() {
    let capture = Capture::new(VALID);
    let command = [
        "target with spaces",
        "$(touch injected)",
        "semi;colon",
        "quote\"'",
        "--flag",
    ];
    let output = capture.run("success", &command);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record = capture.argv("record.argv");
    assert_eq!(&record[..3], ["record", "-g", "-o"]);
    assert_eq!(record[4], "--");
    assert_eq!(&record[5..], command);
    assert_eq!(capture.argv("script.argv"), ["script", "-i", &record[3]]);
    assert!(
        !std::path::Path::new(&record[3]).exists(),
        "temporary perf data was cleaned up"
    );
    assert!(!capture.root.path().join("injected").exists());
    let profile = registry::resolve(capture.root.path(), Some("candidate")).unwrap();
    assert_eq!(
        fs::read_to_string(profile.path).unwrap(),
        "worker;overload(double) [/tmp/a] 3\nworker;overload(int) [/tmp/a] 2\n"
    );
}

#[test]
fn capture_identity() {
    let capture = Capture::new(
        "a  b 12 1.0: 1 cycles:\n  7 foo (/tmp/dir (/nested)/lib.so)\n\na b 12 [000] 2.0: 2 cycles:\n  7 foo (/tmp/dir (/nested)/lib.so)\n\na  b 12/13 [001] 3.0: 3 cycles:\n  7 foo (/tmp/dir ([nested])/lib.so)\n",
    );
    let output = capture.run("success", &["/bin/true"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let profile = registry::resolve(capture.root.path(), Some("candidate")).unwrap();
    assert_eq!(
        fs::read_to_string(profile.path).unwrap(),
        "a  b;foo [/tmp/dir (%5Bnested%5D)/lib.so] 3\na  b;foo [/tmp/dir (/nested)/lib.so] 1\na b;foo [/tmp/dir (/nested)/lib.so] 2\n"
    );
}

#[test]
fn preserve_registry_on_failure() {
    let huge = format!("{}\n", "x".repeat(1024 * 1024 + 1));
    let deep = format!(
        "worker 12 1.0: 1 cycles:\n{}\n",
        "  7 leaf (/tmp/a)\n".repeat(4096)
    );
    for (mode, input, expected) in [
        ("record_failure", VALID, "perf record failed"),
        ("script_failure", VALID, "perf script failed"),
        ("empty", "", "no samples"),
        ("success", "not perf data\n", "invalid perf script line"),
        (
            "success",
            "worker 12 1.0: 18446744073709551616 cycles:\n  7 leaf (/tmp/a)\n",
            "invalid perf script line",
        ),
        (
            "success",
            "worker 12 1.0: 18446744073709551615 cycles:\n  7 leaf (/tmp/a)\n",
            "weight",
        ),
        (
            "success",
            "worker 12 1.0: 1 cycles:\n  7 leaf (/tmp/a)\n\nworker 12 2.0: 1 instructions:\n  7 leaf (/tmp/a)\n",
            "multiple event types",
        ),
        ("success", &huge, "byte limit"),
        ("success", &deep, "depth"),
    ] {
        let capture = Capture::new(input);
        let source = capture.root.path().join("base.folded");
        fs::write(&source, "root;base 1\n").unwrap();
        registry::register(capture.root.path(), &source, Some("candidate"), 1024).unwrap();
        let manifest = capture.root.path().join(".prof-mcp/manifest.json");
        let before = fs::read(&manifest).unwrap();
        let output = capture.run(mode, &["/bin/true"]);
        assert!(!output.status.success(), "{mode}: {input:.80}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(&manifest).unwrap(), before);
        assert_eq!(
            fs::read_dir(capture.root.path().join(".prof-mcp/profiles"))
                .unwrap()
                .count(),
            1
        );
        assert!(
            fs::read_to_string(registry::resolve(capture.root.path(), None).unwrap().path)
                .unwrap()
                .contains("base")
        );
    }
}

#[test]
fn reap_perf_on_parse_error() {
    let capture = Capture::new("");
    let output = capture.run("waiting", &["/bin/true"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid perf script line"));
    let pid = fs::read_to_string(capture.root.path().join("script.pid")).unwrap();
    assert!(
        !std::path::Path::new(&format!("/proc/{}", pid.trim())).exists(),
        "perf must be reaped"
    );
    assert!(!capture.root.path().join(".prof-mcp").exists());
}

use std::{
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

use serde_json::{Value, json};

fn cli(directory: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_prof-mcp"));
    command.current_dir(directory);
    command
}

fn query_samples(directory: &Path, dataset: Value, request: Value) -> Output {
    std::fs::write(directory.join("samples.json"), dataset.to_string()).unwrap();
    let mut child = cli(directory)
        .args(["query-samples", "samples.json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    write!(child.stdin.take().unwrap(), "{request}").unwrap();
    child.wait_with_output().unwrap()
}

fn record(time: u64, tid: u32, leaf: &str, period: Option<u64>) -> Value {
    json!({"comm":"work","pid":7,"tid":tid,"cpu":2,"timestamp_ns":time,
           "period":period,"stack":["root",leaf]})
}

fn snapshot(records: Vec<Value>) -> Value {
    json!({"version":1,"event":"cpu-clock:u","records":records})
}

fn weights(output: Output) -> (u64, u64) {
    let result = result(output);
    (
        result["scope"]["profile_weight"]
            .as_u64()
            .unwrap_or_else(|| panic!("{result}")),
        result["scope"]["selected_weight"]
            .as_u64()
            .unwrap_or_else(|| panic!("{result}")),
    )
}

#[test]
fn samples_count_and_period_are_independent() {
    let dir = tempfile::tempdir().unwrap();
    let data = snapshot(vec![
        record(1, 8, "A", Some(10)),
        record(2, 8, "B", Some(1000)),
    ]);
    for (metric, expected) in [("sample_count", 2), ("sample_period", 1010)] {
        let output = query_samples(
            dir.path(),
            data.clone(),
            json!({"metric":metric,"plan":{"view":{"kind":"frames","stat":"self","limit":10}}}),
        );
        assert_eq!(weights(output), (expected, expected));
    }
    assert!(!dir.path().join(".prof-mcp").exists());
}

#[test]
fn samples_filter_records_before_aggregating_duplicate_paths() {
    let dir = tempfile::tempdir().unwrap();
    let data = snapshot(vec![
        record(10, 8, "A", Some(10)),
        record(20, 8, "A", Some(20)),
        record(20, 9, "A", Some(30)),
        record(30, 8, "B", Some(40)),
        record(20, 8, "A", Some(20)),
    ]);
    for (metric, full, range, thread) in [("sample_count", 5, 3, 2), ("sample_period", 120, 70, 40)]
    {
        let mut request = json!({"metric":metric,"start_ns":20,"end_ns":30,"plan":{"view":{"kind":"frames","stat":"self","limit":10}}});
        assert_eq!(
            weights(query_samples(dir.path(), data.clone(), request.clone())),
            (full, range)
        );
        request["thread"] = json!({"pid":7,"tid":8});
        assert_eq!(
            weights(query_samples(dir.path(), data.clone(), request)),
            (full, thread)
        );
    }
}

fn result(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_cli_error(output: Output, message: &str) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains(message),
        "expected {message:?}, got {error:?}"
    );
}

#[test]
fn unknown_dimensions_reject_slicing_and_period_without_guessing() {
    let dir = tempfile::tempdir().unwrap();
    let mut missing = record(1, 8, "A", None);
    missing["timestamp_ns"] = Value::Null;
    missing["tid"] = Value::Null;
    let data = snapshot(vec![missing, record(2, 8, "B", Some(20))]);
    let request =
        json!({"metric":"sample_count","plan":{"view":{"kind":"frames","stat":"self","limit":10}}});
    let out = result(query_samples(dir.path(), data.clone(), request.clone()));
    assert_eq!(out["scope"]["selected_weight"], 2);
    assert_eq!(
        out["sample"]["timestamp"],
        json!({"source":"unknown","retained":"partial","queryable":false})
    );
    assert_eq!(out["sample"]["timestamp_bounds_ns"], Value::Null);
    for (field, value, message) in [
        ("metric", json!("sample_period"), "capability_unavailable"),
        ("start_ns", json!(2), "capability_unavailable"),
        ("thread", json!({"pid":7,"tid":8}), "capability_unavailable"),
        ("event", json!("instructions"), "single-stream"),
    ] {
        let mut invalid = request.clone();
        invalid[field] = value;
        assert_cli_error(query_samples(dir.path(), data.clone(), invalid), message);
    }
}

#[test]
fn cursor_and_numeric_refs_bind_to_slice_and_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let data = snapshot(vec![
        record(20, 8, "A", Some(10)),
        record(21, 8, "B", Some(20)),
    ]);
    let mut request = json!({"metric":"sample_count","start_ns":20,"end_ns":30,"thread":{"pid":7,"tid":8},"plan":{"view":{"kind":"paths","limit":1}}});
    let first = result(query_samples(dir.path(), data.clone(), request.clone()));
    assert_eq!(
        first["slice"],
        json!({"start_ns":20,"end_ns":30,"thread":{"pid":7,"tid":8}})
    );
    assert!(first["cursor"].is_object(), "{first}");
    request["plan"]["cursor"] = first["cursor"].clone();
    let next = result(query_samples(dir.path(), data.clone(), request.clone()));
    assert_ne!(first["table"]["rows"], next["table"]["rows"]);
    request["end_ns"] = json!(31);
    assert_cli_error(query_samples(dir.path(), data.clone(), request), "Cursor");

    let mut numeric = json!({"metric":"sample_count","predicate":{"contains_all":[{"frame_id":0}]},"plan":{"view":{"kind":"contexts","frame":{"frame_id":0},"limit":10}}});
    assert_cli_error(
        query_samples(dir.path(), data.clone(), numeric.clone()),
        "snapshot",
    );
    numeric["snapshot_id"] = first["selection"]["snapshot_id"].clone();
    let pinned = result(query_samples(dir.path(), data.clone(), numeric.clone()));
    assert_eq!(pinned["scope"]["selected_weight"], 1);
    let mut changed = data;
    changed["records"][0]["timestamp_ns"] = json!(55);
    assert_cli_error(
        query_samples(dir.path(), changed, numeric),
        "snapshot_mismatch",
    );
}

#[cfg(target_os = "linux")]
#[test]
fn perf_import_declares_missing_dimensions_and_creates_snapshot_exclusively() {
    use std::{fs, os::unix::fs::PermissionsExt};

    let dir = tempfile::tempdir().unwrap();
    let fake_perf = dir.path().join("perf");
    fs::write(&fake_perf, r#"#!/bin/sh
case "$1:$2" in
--version:) printf '%s\n' 'perf version fixture' ;;
evlist:-v) printf '%s\n' 'clock: type: 1, sample_type: IP|TID|TIME|CALLCHAIN' ;;
report:--stats) printf '%s\n' 'Aggregated stats:' 'TOTAL events: 4' 'SAMPLE events: 4' 'LOST events: 1' 'clock stats:' 'SAMPLE events: 4' 'LOST_SAMPLES events: 9' ;;
script:--header-only) printf '%s\n' '# perf version : fixture' ;;
script:--ns)
  case " $* " in
  *' --hide-call-graph '*)
    printf '%s\n' 'worker group 7/8 9007199.254740991: clock: 12 leaf (/fixture)' 'worker group 7/8 9007199.254740992: clock: 12 leaf (/fixture)' 'worker group 7/8 9007199.254740993: clock: 12 foo [A (B)' 'worker group 7/8 9007199.254740994: clock: 13 foo (A [B)' ;;
  *)
    printf '%s\n' 'worker group 7/8 9007199.254740991: clock:' ' 12 leaf (/fixture)' ' 13 caller (/fixture)' '' 'worker group 7/8 9007199.254740992: clock:' '' 'worker group 7/8 9007199.254740993: clock:' '' 'worker group 7/8 9007199.254740994: clock:' ' 14 other (/fixture)' ' 15 caller (/fixture)' ;;
  esac ;;
buildid-list:-i) ;;
*) exit 3 ;;
esac
"#).unwrap();
    fs::set_permissions(&fake_perf, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(dir.path().join("perf.data"), b"PERFILE2 test fixture").unwrap();
    let import = || {
        cli(dir.path())
            .env("PATH", dir.path())
            .args(["import-samples", "perf.data", "result.json"])
            .output()
            .unwrap()
    };
    let output = import();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let path = dir.path().join("result.json");
    let original = fs::read(&path).unwrap();
    let data: Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(
        data["records"][0]["stack"],
        json!(["caller [/fixture]", "leaf [/fixture]"])
    );
    assert_eq!(data["records"][1]["stack"], json!(["leaf [/fixture]"]));
    assert_eq!(data["records"][0]["sampled_ip"], "12");
    assert_eq!(data["records"][2]["stack"], json!(["foo %5BA [B]"]));
    assert_eq!(data["records"][3]["stack"], json!(["foo [A [B]"]));
    assert_eq!(data["records"][3]["sampled_ip"], "13");
    assert_eq!(data["records"][3]["stack_source"], "sampled_ip_only");
    assert_eq!(data["records"][2]["timestamp_ns"], 9007199254740993_u64);
    assert_eq!(
        data["source"]["script_fields"],
        "comm,pid,tid,time,event,ip,sym,dso"
    );
    let request =
        json!({"metric":"sample_count","plan":{"view":{"kind":"frames","stat":"self","limit":10}}});
    let filtered = result(query_samples(dir.path(), data, request));
    assert_eq!(
        filtered["sample"]["cpu"],
        json!({"source":"absent","retained":"absent","queryable":false})
    );
    assert_eq!(
        filtered["sample"]["period"],
        json!({"source":"absent","retained":"absent","queryable":false})
    );
    assert_eq!(
        filtered["sample"]["perf_reported_loss"],
        json!({"lost_record_notifications":1,"lost_samples":9})
    );
    assert_eq!(filtered["sample"]["perf_callchain_records"], 1);
    assert_cli_error(import(), "could not create sample snapshot");
    assert_eq!(fs::read(path).unwrap(), original);
    assert!(!dir.path().join(".prof-mcp").exists());
}

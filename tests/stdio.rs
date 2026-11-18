use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use prof_mcp::registry;
use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(10);

struct StdioServer {
    child: Child,
    stdin: Option<ChildStdin>,
    responses: Receiver<Result<Value, String>>,
    diagnostics: Receiver<String>,
    readers: Vec<JoinHandle<()>>,
}

impl StdioServer {
    fn start(workspace: &std::path::Path, options: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_prof-mcp"))
            .arg("serve")
            .args(options)
            .current_dir(workspace)
            .env("RUST_LOG", "debug")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (send, responses) = mpsc::channel();
        let output = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let response = line.map_err(|e| e.to_string()).and_then(|line| {
                    serde_json::from_str(&line)
                        .map_err(|e| format!("non-protocol stdout: {line:?}: {e}"))
                });
                if send.send(response).is_err() {
                    break;
                }
            }
        });
        let (send, diagnostics) = mpsc::channel();
        let errors = thread::spawn(move || {
            let mut text = String::new();
            stderr.read_to_string(&mut text).unwrap();
            let _ = send.send(text);
        });
        Self {
            child,
            stdin,
            responses,
            diagnostics,
            readers: vec![output, errors],
        }
    }

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        serde_json::to_writer(&mut *stdin, &message).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}));
        let response = self
            .responses
            .recv_timeout(TIMEOUT)
            .expect("stdio response timed out or transport closed")
            .unwrap();
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], id);
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    fn finish(&mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "server exited with {status}");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "server did not stop after stdin EOF"
            );
            thread::sleep(Duration::from_millis(10));
        }
        for reader in self.readers.drain(..) {
            reader.join().unwrap();
        }
        assert!(
            self.responses.try_iter().collect::<Vec<_>>().is_empty(),
            "unexpected stdout after last response"
        );
        let stderr = self.diagnostics.recv_timeout(TIMEOUT).unwrap();
        assert!(!stderr.is_empty(), "debug logging must reach stderr");
        assert!(
            !stderr.contains('\u{1b}'),
            "diagnostics must remain plain text"
        );
    }
}

impl Drop for StdioServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

#[test]
fn stdio_protocol() {
    #[cfg(target_os = "linux")]
    let workspace = tempfile::tempdir_in("/dev/shm").unwrap();
    #[cfg(not(target_os = "linux"))]
    let workspace = tempfile::tempdir().unwrap();
    let mut server = StdioServer::start(workspace.path(), &[]);
    let initialized = server.request(
        1,
        "initialize",
        json!({
            "protocolVersion":"2025-11-25", "capabilities":{},
            "clientInfo":{"name":"stdio-contract-test", "version":"1"}
        }),
    );
    assert_eq!(initialized["protocolVersion"], "2025-11-25");
    server.send(json!({"jsonrpc":"2.0", "method":"notifications/initialized"}));
    let tools = server.request(2, "tools/list", json!({}));
    let names: Vec<_> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "profile_summary",
            "profile_find_symbols",
            "profile_top",
            "profile_tree",
            "profile_callers",
            "profile_callees",
            "profile_paths",
            "profile_diff"
        ]
    );
    let missing = server.request(
        3,
        "tools/call",
        json!({"name":"profile_summary", "arguments":{}}),
    );
    assert_eq!(missing["isError"], true);
    assert_eq!(
        serde_json::from_str::<Value>(missing["content"][0]["text"].as_str().unwrap()).unwrap()["code"],
        "workspace_not_registered"
    );

    let source = workspace.path().join("sample.folded");
    fs::write(&source, "root;函数 3\n").unwrap();
    registry::register(workspace.path(), &source, Some("sample"), 1024).unwrap();
    let available = server.request(
        4,
        "tools/call",
        json!({"name":"profile_summary", "arguments":{}}),
    );
    assert_eq!(available["isError"], false);
    assert_eq!(available["structuredContent"]["schema_version"], "3");
    assert_eq!(available["structuredContent"]["profile"]["alias"], "sample");
    assert_eq!(available["structuredContent"]["scope_weight"], 3);
    let symbols = server.request(
        5,
        "tools/call",
        json!({"name":"profile_find_symbols", "arguments":{"query":"函数"}}),
    );
    assert_eq!(symbols["isError"], false);
    let matches = &symbols["structuredContent"]["data"]["matches"];
    let columns = matches["columns"].as_array().unwrap();
    let name = columns.iter().position(|c| c == "name").unwrap();
    assert_eq!(matches["rows"][0][name], "函数");
    server.finish();
}

fn open_experimental(workspace: &std::path::Path, mode: &str) -> StdioServer {
    let mut server = StdioServer::start(
        workspace,
        &["--toolset", "experimental", "--output-format", mode],
    );
    server.request(
        1,
        "initialize",
        json!({
            "protocolVersion":"2025-11-25", "capabilities":{},
            "clientInfo":{"name":"stdio-contract-test", "version":"1"}
        }),
    );
    server.send(json!({"jsonrpc":"2.0", "method":"notifications/initialized"}));
    server
}

fn executable(workspace: &std::path::Path, args: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_prof-mcp"))
        .args(args)
        .current_dir(workspace)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn tool_error(response: &Value) -> Value {
    assert_eq!(response["isError"], true, "{response}");
    serde_json::from_str(response["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn experimental_text_and_structured_share_the_cli_result() {
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path();
    let source = path.join("sample.folded");
    fs::write(&source, "root;函数 7\nroot;other 3\n").unwrap();
    registry::register(path, &source, Some("sample"), 1024).unwrap();

    let mut structured = open_experimental(path, "structured");
    let tools = structured.request(2, "tools/list", json!({}));
    let tools = tools["tools"].as_array().unwrap();
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "profile_summary",
            "profile_find_symbols",
            "profile_query",
            "profile_explore",
            "profile_diff"
        ]
    );
    assert!(tools.iter().all(|tool| tool.get("outputSchema").is_none()));
    assert_eq!(tools[2]["inputSchema"]["additionalProperties"], false);

    let arguments = json!({"view":{"kind":"frames","stat":"self", "limit":1}});
    let request = json!({"name":"profile_query", "arguments":arguments});
    let response = structured.request(3, "tools/call", request.clone());
    assert_eq!(response["isError"], false, "{response}");
    let result = &response["structuredContent"];
    assert_eq!(result["schema_version"], "experimental-1");
    assert_eq!(result["scope"]["selected_weight"], 10);
    assert!(response["content"][0]["text"].as_str().unwrap().len() < 128);

    let mut text_server = open_experimental(path, "text");
    let text = text_server.request(2, "tools/call", request);
    assert_eq!(text["isError"], false, "{text}");
    assert!(text.get("structuredContent").is_none());
    assert_eq!(
        serde_json::from_str::<Value>(text["content"][0]["text"].as_str().unwrap()).unwrap(),
        *result
    );
    text_server.finish();

    let next_page = structured.request(
        4,
        "tools/call",
        json!({
            "name":"profile_query", "arguments":{
                "source":{"selection":result["selection"]},
                "cursor":result["cursor"],
                "view":arguments["view"]
            }
        }),
    );
    let next_page = &next_page["structuredContent"];
    assert_eq!(next_page["table"]["rows"][0][1], "other");
    assert_eq!(next_page["totals"]["matched_weight"], 10);
    assert!(next_page.get("cursor").is_none());

    let oversized = structured.request(
        5,
        "tools/call",
        json!({"name":"profile_query", "arguments":{
            "view":{"kind":"frames", "stat":"self","where":{"name_regex":"x".repeat(70 * 1024)}}
        }}),
    );
    assert_eq!(tool_error(&oversized)["code"], "query_budget_exceeded");
    let conflicting_anchor = structured.request(
        6,
        "tools/call",
        json!({"name":"profile_explore", "arguments":{
            "anchor":{"frame_name":"other", "context":{"node_id":0, "offset":0}}
        }}),
    );
    assert_eq!(tool_error(&conflicting_anchor)["code"], "invalid_request");
    let recovered = structured.request(7, "tools/call", json!({"name":"profile_explore"}));
    assert_eq!(recovered["isError"], false, "{recovered}");
    assert_eq!(
        recovered["structuredContent"]["scope"]["selected_weight"],
        10
    );

    let manifest = path.join(".prof-mcp/manifest.json");
    let original_manifest = fs::read(&manifest).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_prof-mcp"))
        .args(["query", "--jsonl"])
        .current_dir(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "not JSON").unwrap();
    writeln!(stdin, "{}", "x".repeat(70 * 1024)).unwrap();
    writeln!(
        stdin,
        "{}",
        json!({
            "tool": "profile_query",
            "arguments": {"view": {"kind": "frames", "stat": "self"}, "extra": true}
        })
    )
    .unwrap();
    writeln!(
        stdin,
        "{}",
        json!({"tool":"profile_query", "arguments":arguments})
    )
    .unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let lines: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0]["error"]["code"], "invalid_request");
    assert_eq!(lines[1]["error"]["code"], "query_budget_exceeded");
    assert_eq!(lines[2]["error"]["code"], "invalid_request");
    assert_eq!(lines[3]["result"], *result);
    assert_eq!(fs::read(manifest).unwrap(), original_manifest);
    structured.finish();
}

#[test]
fn experimental_pages_account_for_text_escaping() {
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path();
    let source = path.join("escaped.folded");
    let count = 55;
    let names: Vec<_> = (0..count)
        .map(|index| format!("leaf{index}{}", "\"".repeat(500)))
        .collect();
    let folded: String = names
        .iter()
        .enumerate()
        .map(|(index, name)| format!("root;{name} {}\n", count - index))
        .collect();
    fs::write(&source, folded).unwrap();
    registry::register(path, &source, Some("escaped"), 64 * 1024).unwrap();

    let mut structured = open_experimental(path, "structured");
    let mut text = open_experimental(path, "text");
    let mut cli = Command::new(env!("CARGO_BIN_EXE_prof-mcp"))
        .args(["query", "--jsonl"])
        .current_dir(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut cli_input = cli.stdin.take().unwrap();
    let mut cli_output = BufReader::new(cli.stdout.take().unwrap());
    let view = json!({"kind":"frames", "stat":"self", "limit":count});
    let mut arguments = json!({"view":view});
    let mut position = 0;
    let mut pages = 0;
    let matched = count * (count + 1) / 2;

    for id in 2..=count as u64 + 1 {
        let request = json!({"name":"profile_query", "arguments":arguments});
        let response = structured.request(id, "tools/call", request.clone());
        let text_response = text.request(id, "tools/call", request);
        assert_eq!(response["isError"], false);
        assert_eq!(text_response["isError"], false, "{text_response}");
        assert!(text_response.get("structuredContent").is_none());
        for wire in [&response, &text_response] {
            assert!(serde_json::to_vec(wire).unwrap().len() <= 64 * 1024);
        }
        let page = &response["structuredContent"];
        assert_eq!(
            serde_json::from_str::<Value>(text_response["content"][0]["text"].as_str().unwrap())
                .unwrap(),
            *page,
        );

        writeln!(
            cli_input,
            "{}",
            json!({"tool":"profile_query", "arguments":arguments})
        )
        .unwrap();
        cli_input.flush().unwrap();
        let mut line = String::new();
        cli_output.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap()["result"],
            *page
        );

        let rows = page["table"]["rows"].as_array().unwrap();
        assert!(!rows.is_empty());
        assert_eq!(page["completeness"]["query_exact"], true);
        assert_eq!(page["totals"]["matched_weight"], matched);
        let mut returned_weight = 0;
        for row in rows {
            assert_eq!(row[1], names[position]);
            let weight = count - position;
            assert_eq!(row[2], weight);
            returned_weight += weight;
            position += 1;
        }
        assert_eq!(page["totals"]["returned_weight"], returned_weight);
        assert_eq!(page["totals"]["omitted_weight"], matched - returned_weight);
        pages += 1;

        let Some(cursor) = page.get("cursor") else {
            break;
        };
        assert_eq!(cursor["position"], position);
        assert!(
            page["truncation"].as_array().unwrap().iter().any(|reason| {
                reason["kind"] == "response_bytes" && reason["available"] == count
            })
        );
        arguments = json!({"source":{"selection":page["selection"]}, "view":view, "cursor":cursor});
    }
    assert!(pages > 1);
    assert_eq!(position, count);
    drop(cli_input);
    assert!(cli.wait().unwrap().success());
    structured.finish();
    text.finish();
}

#[test]
fn experimental_selection_survives_alias_rebind_and_restart_until_gc() {
    let workspace = tempfile::tempdir().unwrap();
    let path = workspace.path();
    let source = path.join("sample.folded");
    fs::write(&source, "root;old 3\n").unwrap();
    registry::register(path, &source, Some("sample"), 1024).unwrap();

    let mut server = open_experimental(path, "structured");
    let query =
        json!({"name":"profile_query","arguments":{"view":{"kind":"frames","stat":"self"}}});
    let response = server.request(2, "tools/call", query.clone());
    let selection = response["structuredContent"]["selection"].clone();
    assert!(selection["source_digest"].is_string(), "{response}");
    server.finish();

    fs::write(&source, "root;new 7\n").unwrap();
    registry::register(path, &source, Some("sample"), 1024).unwrap();
    let mut server = open_experimental(path, "structured");
    let latest = server.request(2, "tools/call", query);
    assert_eq!(latest["structuredContent"]["scope"]["selected_weight"], 7);
    let by_reference = json!({"name":"profile_query","arguments":{
        "source":{"selection":selection}, "view":{"kind":"frames","stat":"self"}
    }});
    let previous = server.request(3, "tools/call", by_reference.clone());
    assert_eq!(
        previous["structuredContent"]["scope"]["selected_weight"], 3,
        "{previous}"
    );
    let conflict = server.request(4, "tools/call", json!({"name":"profile_query","arguments":{
        "source":{"profile":"sample", "selection":selection}, "view":{"kind":"frames","stat":"self"}
    }}));
    assert_eq!(tool_error(&conflict)["code"], "invalid_request");
    let mut mismatched = by_reference.clone();
    mismatched["arguments"]["source"]["selection"]["snapshot_id"] =
        latest["structuredContent"]["selection"]["snapshot_id"].clone();
    let mismatch = server.request(5, "tools/call", mismatched);
    assert_eq!(tool_error(&mismatch)["code"], "snapshot_mismatch");
    executable(path, &["gc"]);
    let unavailable = server.request(6, "tools/call", by_reference);
    assert_eq!(tool_error(&unavailable)["code"], "snapshot_unavailable");
    server.finish();
}

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
    fn start(workspace: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_prof-mcp"))
            .args(["serve"])
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
    let mut server = StdioServer::start(workspace.path());
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
        missing["structuredContent"]["code"],
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
    assert_eq!(available["structuredContent"]["schema_version"], "2");
    assert_eq!(available["structuredContent"]["profile"]["alias"], "sample");
    assert_eq!(available["structuredContent"]["scope_weight"], 3);
    let symbols = server.request(
        5,
        "tools/call",
        json!({"name":"profile_find_symbols", "arguments":{"query":"函数"}}),
    );
    assert_eq!(symbols["isError"], false);
    assert_eq!(
        symbols["structuredContent"]["data"]["matches"][0]["name"],
        "函数"
    );
    server.finish();
}

use std::{fs, path::Path, time::Duration};

use prof_mcp::{config::Config, registry, server::ProfileServer};
use rmcp::{
    ClientHandler, Peer, RoleClient, ServiceExt,
    model::{CallToolRequestParams, ClientConfig, ProtocolVersion},
    service::{ClientLifecycleMode, ClientServiceExt},
};
use tempfile::tempdir;

#[derive(Clone, Debug)]
struct TestClient {
    protocol_version: ProtocolVersion,
}
impl ClientHandler for TestClient {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::default().with_protocol_version(self.protocol_version.clone())
    }
}

// Bound the entire protocol exchange, including handshake and shutdown. Aborting
// the server task also cleans it up if an assertion panics or a timeout fires.
struct ServerTask(tokio::task::JoinHandle<anyhow::Result<()>>);
impl Drop for ServerTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn check(workspace: &Path, modern: bool, test: impl AsyncFnOnce(Peer<RoleClient>)) {
    tokio::time::timeout(Duration::from_secs(10), async {
        let server = ProfileServer::new_in_workspace(
            Config {
                max_file_size_mib: 1,
            },
            workspace.to_owned(),
        )
        .unwrap();
        let (server_transport, client_transport) = tokio::io::duplex(65536);
        let mut server_task = ServerTask(tokio::spawn(async move {
            server.serve(server_transport).await?.waiting().await?;
            anyhow::Ok(())
        }));
        let handler = TestClient {
            protocol_version: if modern {
                ProtocolVersion::V_2026_07_28
            } else {
                ProtocolVersion::V_2025_11_25
            },
        };
        let client = if modern {
            handler
                .serve_with_lifecycle(
                    client_transport,
                    ClientLifecycleMode::Discover {
                        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
                    },
                )
                .await
                .unwrap()
        } else {
            handler.serve(client_transport).await.unwrap()
        };
        test(client.peer().clone()).await;
        client.cancel().await.unwrap();
        (&mut server_task.0).await.unwrap().unwrap();
    })
    .await
    .expect("MCP exchange timed out");
}

async fn sample(test: impl AsyncFnOnce(Peer<RoleClient>)) {
    let root = tempdir().unwrap();
    let source = root.path().join("sample.folded");
    fs::write(&source, "root;A 3\nroot;A;B 2\n").unwrap();
    registry::register(root.path(), &source, Some("sample"), 1024 * 1024).unwrap();
    check(root.path(), true, test).await;
}

#[tokio::test]
async fn discover_live_registration() {
    // Registry discovery intentionally walks ancestors. Use tmpfs on Linux so
    // a developer's unrelated `/tmp/.prof-mcp` cannot turn this into a
    // registered-workspace test.
    #[cfg(target_os = "linux")]
    let workspace = tempfile::tempdir_in("/dev/shm").unwrap();
    #[cfg(not(target_os = "linux"))]
    let workspace = tempdir().unwrap();
    check(workspace.path(), false, async |client| {
        let tools = client.list_tools(None).await.unwrap();
        assert_eq!(tools.tools.len(), 8);
        assert_eq!(tools.ttl_ms, None);
        assert_eq!(tools.cache_scope, None);
        let unavailable = client
            .call_tool(CallToolRequestParams::new("profile_summary"))
            .await
            .unwrap();
        assert_eq!(unavailable.is_error, Some(true));
        assert_eq!(
            unavailable.structured_content.unwrap()["code"],
            "workspace_not_registered"
        );

        let source = workspace.path().join("started.folded");
        fs::write(&source, "root;visible 1\n").unwrap();
        registry::register(workspace.path(), &source, Some("visible"), 1024 * 1024).unwrap();
        let available = client
            .call_tool(CallToolRequestParams::new("profile_summary"))
            .await
            .unwrap();
        assert_eq!(available.is_error, Some(false));
        assert_eq!(
            available.structured_content.unwrap()["profile"]["alias"],
            "visible"
        );
    })
    .await;
}

#[path = "mcp/contracts.rs"]
mod contracts;
#[path = "mcp/responses.rs"]
mod responses;

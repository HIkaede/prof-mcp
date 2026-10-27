use rmcp::handler::server::wrapper::Parameters;
use std::time::Duration;

use super::*;

const TIMEOUT: Duration = Duration::from_secs(3);

#[test]
fn listed_tools_match_the_router() {
    let mut routed: Vec<String> = ProfileServer::build_tool_router()
        .list_all()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect();
    routed.sort_unstable();
    let mut listed: Vec<String> = TOOL_NAMES.iter().map(|name| (*name).to_owned()).collect();
    listed.sort_unstable();
    assert_eq!(listed, routed);
}

#[test]
fn cold_query() {
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    std::fs::write(&source, "root;leaf 1\n").unwrap();
    crate::registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
    let server = ProfileServer::new_in_workspace(
        Config {
            max_file_size_mib: 1,
            max_cache_mib: 512,
        },
        workspace.path().to_owned(),
    )
    .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    let result = runtime.block_on(async {
        tokio::time::timeout(TIMEOUT, async {
            server
                .profile_summary(Parameters(inputs::ProfileInput { profile: None }))
                .await
        })
        .await
    });
    // A nested blocking-task regression must fail rather than hang runtime teardown.
    runtime.shutdown_timeout(TIMEOUT);
    assert_ne!(result.unwrap().is_error, Some(true));
}

#[tokio::test]
async fn query_worker() {
    let workspace = tempfile::tempdir().unwrap();
    let server = Arc::new(
        ProfileServer::new_in_workspace(
            Config {
                max_file_size_mib: 1,
                max_cache_mib: 512,
            },
            workspace.path().to_owned(),
        )
        .unwrap(),
    );
    let (release, wait) = std::sync::mpsc::channel::<()>();
    tokio::time::timeout(TIMEOUT, async {
        let (server_transport, client_transport) = tokio::io::duplex(65536);
        let service = (*server).clone();
        let server_task = tokio::spawn(async move {
            service
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await
        });
        let client = ().serve(client_transport).await.unwrap();
        let worker = server.clone();
        let workspace = server.top_workspace.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let request = tokio::spawn(async move {
            worker
                .query(async { Ok(()) }, move |()| {
                    let _workspace = workspace.blocking_lock();
                    started.send(()).unwrap();
                    let _ = wait.recv();
                    Ok(rmcp::model::CallToolResult::default())
                })
                .await
        });
        ready.await.unwrap();
        let waiting_server = server.clone();
        let (loading, mut loaded) = tokio::sync::oneshot::channel();
        let (queued, waiting_ready) = tokio::sync::oneshot::channel();
        let waiting = tokio::spawn(async move {
            queued.send(()).unwrap();
            waiting_server
                .query(
                    async move {
                        loading.send(()).unwrap();
                        Ok(())
                    },
                    |()| Ok(rmcp::model::CallToolResult::default()),
                )
                .await
        });
        waiting_ready.await.unwrap();
        client
            .peer()
            .send_request(rmcp::model::ClientRequest::PingRequest(
                rmcp::model::PingRequest::default(),
            ))
            .await
            .unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert_eq!(server.query_gate.available_permits(), 0);
        assert!(server.top_workspace.try_lock().is_err());
        assert!(matches!(
            loaded.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        release.send(()).unwrap();
        loaded.await.unwrap();
        assert_ne!(waiting.await.unwrap().is_error, Some(true));
        let _permit = server.query_gate.acquire().await.unwrap();
        assert!(server.top_workspace.try_lock().is_ok());
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    })
    .await
    .unwrap();
}

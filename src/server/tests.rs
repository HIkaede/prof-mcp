use std::time::Duration;

use super::*;

const TIMEOUT: Duration = Duration::from_secs(3);

#[test]
fn cold_query() {
    let workspace = tempfile::tempdir().unwrap();
    let source = workspace.path().join("input.folded");
    std::fs::write(&source, "root;leaf 1\n").unwrap();
    crate::registry::register(workspace.path(), &source, Some("base"), 1024).unwrap();
    let server = ProfileServer::new_in_workspace(
        Config {
            max_file_size_mib: 1,
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
        let (started, ready) = tokio::sync::oneshot::channel();
        let request = tokio::spawn(async move {
            worker
                .query(async { Ok(()) }, move |()| {
                    started.send(()).unwrap();
                    let _ = wait.recv();
                    Ok(json!({}))
                })
                .await
        });
        ready.await.unwrap();
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
        release.send(()).unwrap();
        let _permit = server.query_gate.acquire().await.unwrap();
        client.cancel().await.unwrap();
        server_task.await.unwrap().unwrap();
    })
    .await
    .unwrap();
}

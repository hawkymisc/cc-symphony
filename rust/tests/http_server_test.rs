//! HTTP server integration tests (requires `--features http-server`)
//!
//! Run with:  cargo test --features http-server --test http_server_test

#![cfg(feature = "http-server")]

use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use symphony::observability::RuntimeSnapshot;
use symphony::orchestrator::OrchestratorMsg;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Bind an ephemeral TCP port, start the HTTP server, and return a reqwest client
/// configured with the base URL plus the cancel token.
///
/// A mock orchestrator task is spawned that responds to SnapshotRequest and
/// RefreshRequest automatically.
async fn start_test_server() -> (reqwest::Client, String, CancellationToken) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base_url = format!("http://127.0.0.1:{}", port);

    let (tx, mut rx) = mpsc::unbounded_channel::<OrchestratorMsg>();

    // Mock orchestrator — auto-responds to every known message
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            match msg {
                OrchestratorMsg::SnapshotRequest { reply } => {
                    let _ = reply.send(RuntimeSnapshot::default());
                }
                OrchestratorMsg::RefreshRequest { reply } => {
                    let _ = reply.send(());
                }
                _ => {}
            }
        }
    });

    let cancel = CancellationToken::new();
    let cancel_srv = cancel.clone();
    tokio::spawn(async move {
        symphony::http_server::start_server(listener, tx, cancel_srv)
            .await
            .unwrap();
    });

    // Give the server a moment to start accepting connections
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    (client, base_url, cancel)
}

// ---------------------------------------------------------------------------
// Tests: GET /api/status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_api_status_returns_200() {
    let (client, base, _cancel) = start_test_server().await;

    let res = client
        .get(format!("{}/api/status", base))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn get_api_status_content_type_is_json() {
    let (client, base, _cancel) = start_test_server().await;

    let res = client
        .get(format!("{}/api/status", base))
        .send()
        .await
        .unwrap();

    let ct = res.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("application/json"), "content-type was: {}", ct);
}

#[tokio::test]
async fn get_api_status_body_has_running_count() {
    let (client, base, _cancel) = start_test_server().await;

    let json: serde_json::Value = client
        .get(format!("{}/api/status", base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert!(json.get("running_count").is_some(), "missing running_count field");
    assert!(json.get("retrying_count").is_some(), "missing retrying_count field");
    assert!(json.get("completed_count").is_some(), "missing completed_count field");
    assert_eq!(json["running_count"].as_u64().unwrap(), 0);
}

#[tokio::test]
async fn get_api_status_body_deserializes_to_runtime_snapshot() {
    let (client, base, _cancel) = start_test_server().await;

    let snapshot: RuntimeSnapshot = client
        .get(format!("{}/api/status", base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(snapshot.running_count, 0);
    assert_eq!(snapshot.retrying_count, 0);
    assert!(snapshot.running.is_empty());
}

// ---------------------------------------------------------------------------
// Tests: POST /api/refresh
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_api_refresh_returns_200() {
    let (client, base, _cancel) = start_test_server().await;

    let res = client
        .post(format!("{}/api/refresh", base))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn post_api_refresh_sends_refresh_request_to_orchestrator() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let base_url = format!("http://127.0.0.1:{}", port);

    let (tx, mut rx) = mpsc::unbounded_channel::<OrchestratorMsg>();
    let (saw_refresh_tx, mut saw_refresh_rx) = mpsc::unbounded_channel::<bool>();

    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            match msg {
                OrchestratorMsg::RefreshRequest { reply } => {
                    let _ = saw_refresh_tx.send(true);
                    let _ = reply.send(());
                }
                OrchestratorMsg::SnapshotRequest { reply } => {
                    let _ = reply.send(RuntimeSnapshot::default());
                }
                _ => {}
            }
        }
    });

    let cancel = CancellationToken::new();
    let cancel_srv = cancel.clone();
    tokio::spawn(async move {
        symphony::http_server::start_server(listener, tx, cancel_srv)
            .await
            .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    client
        .post(format!("{}/api/refresh", base_url))
        .send()
        .await
        .unwrap();

    let got = tokio::time::timeout(Duration::from_secs(1), saw_refresh_rx.recv())
        .await
        .expect("timed out waiting for RefreshRequest")
        .unwrap();

    assert!(got);
}

// ---------------------------------------------------------------------------
// Tests: GET /  (dashboard)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_root_returns_200() {
    let (client, base, _cancel) = start_test_server().await;

    let res = client
        .get(format!("{}/", base))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn get_root_content_type_is_html() {
    let (client, base, _cancel) = start_test_server().await;

    let res = client
        .get(format!("{}/", base))
        .send()
        .await
        .unwrap();

    let ct = res.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.contains("text/html"), "content-type was: {}", ct);
}

#[tokio::test]
async fn get_root_body_contains_symphony_title() {
    let (client, base, _cancel) = start_test_server().await;

    let body = client
        .get(format!("{}/", base))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(body.contains("Symphony"), "body did not contain 'Symphony': {}", &body[..200]);
    assert!(body.contains("/api/status"), "body did not reference /api/status");
}

// ---------------------------------------------------------------------------
// Tests: graceful shutdown
// ---------------------------------------------------------------------------

#[tokio::test]
async fn server_shuts_down_on_cancel() {
    let (client, base, cancel) = start_test_server().await;

    // Verify server is up
    let res = client
        .get(format!("{}/api/status", base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    // Cancel the server
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Subsequent request should fail (connection refused)
    let err = client
        .get(format!("{}/api/status", base))
        .send()
        .await;
    assert!(err.is_err(), "expected connection error after shutdown");
}

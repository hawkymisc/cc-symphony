//! HTTP server for Symphony observability dashboard (feature-gated)
//!
//! Enabled by the `http-server` Cargo feature.
//! Provides:
//!   GET  /            — HTML dashboard (auto-refreshing)
//!   GET  /api/status  — JSON RuntimeSnapshot
//!   POST /api/refresh — trigger immediate orchestrator poll
//!
//! Security note: the server binds to 127.0.0.1 (loopback only).
//! There is no authentication; do not expose this port externally.

#[cfg(feature = "http-server")]
pub use server::{start_server, bind_localhost};

#[cfg(feature = "http-server")]
mod server {
    use std::sync::Arc;
    use std::time::Duration;

    use axum::{
        Router,
        extract::State,
        http::StatusCode,
        response::{Html, IntoResponse},
        routing::{get, post},
        Json,
    };
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use crate::orchestrator::OrchestratorMsg;

    /// Timeout for waiting on orchestrator responses (prevents hanging workers).
    const ORCHESTRATOR_TIMEOUT: Duration = Duration::from_secs(5);

    #[derive(Clone)]
    struct AppState {
        tx: mpsc::UnboundedSender<OrchestratorMsg>,
    }

    /// Start the HTTP server on the given listener.
    ///
    /// The listener should be bound to `127.0.0.1` (loopback) — see
    /// [`bind_localhost`] for a helper that enforces this.
    /// Shuts down gracefully when `cancel` is fired.
    pub async fn start_server(
        listener: TcpListener,
        tx: mpsc::UnboundedSender<OrchestratorMsg>,
        cancel: CancellationToken,
    ) -> std::io::Result<()> {
        let state = Arc::new(AppState { tx });

        let app = Router::new()
            .route("/", get(get_dashboard))
            .route("/api/status", get(get_status))
            .route("/api/refresh", post(post_refresh))
            .with_state(state);

        axum::serve(listener, app)
            .with_graceful_shutdown(async move { cancel.cancelled().await })
            .await
    }

    /// Bind to localhost on `port` (127.0.0.1, not 0.0.0.0).
    pub async fn bind_localhost(port: u16) -> std::io::Result<TcpListener> {
        TcpListener::bind(format!("127.0.0.1:{port}")).await
    }

    async fn get_dashboard() -> Html<&'static str> {
        Html(DASHBOARD_HTML)
    }

    async fn get_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        if state
            .tx
            .send(OrchestratorMsg::SnapshotRequest { reply: reply_tx })
            .is_err()
        {
            // Orchestrator channel closed
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }

        match tokio::time::timeout(ORCHESTRATOR_TIMEOUT, reply_rx).await {
            Ok(Ok(snapshot)) => (StatusCode::OK, Json(snapshot)).into_response(),
            // Timeout or orchestrator dropped the sender
            _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    }

    async fn post_refresh(State(state): State<Arc<AppState>>) -> StatusCode {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        if state
            .tx
            .send(OrchestratorMsg::RefreshRequest { reply: reply_tx })
            .is_err()
        {
            return StatusCode::SERVICE_UNAVAILABLE;
        }

        match tokio::time::timeout(ORCHESTRATOR_TIMEOUT, reply_rx).await {
            Ok(Ok(())) => StatusCode::OK,
            _ => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    // Dashboard HTML — uses only DOM textContent/createElement, never innerHTML,
    // to prevent XSS from attacker-controlled field values (issue titles, errors).
    const DASHBOARD_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Symphony Dashboard</title>
  <style>
    body { font-family: monospace; padding: 2rem; background: #0d1117; color: #e6edf3; }
    h1   { color: #58a6ff; margin-bottom: 1rem; }
    .card { background: #161b22; border: 1px solid #30363d; border-radius: 6px;
            padding: 1rem; margin-bottom: 1rem; }
    .stat { font-size: 2rem; font-weight: bold; color: #58a6ff; }
    .label { color: #8b949e; font-size: 0.85rem; }
    .grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 1rem; }
    table { width: 100%; border-collapse: collapse; margin-top: 0.5rem; }
    th, td { text-align: left; padding: 0.4rem 0.6rem; border-bottom: 1px solid #30363d; }
    th { color: #8b949e; font-weight: normal; }
    .ts { color: #8b949e; font-size: 0.8rem; }
    .refresh-btn { background: #21262d; color: #e6edf3; border: 1px solid #30363d;
                   border-radius: 6px; padding: 0.4rem 0.8rem; cursor: pointer; }
    .refresh-btn:hover { background: #30363d; }
  </style>
</head>
<body>
  <h1>Symphony</h1>
  <button class="refresh-btn" onclick="load()">Refresh now</button>
  <span class="ts" id="ts"></span>
  <div class="grid" style="margin-top:1rem">
    <div class="card"><div class="stat" id="running">—</div><div class="label">Running</div></div>
    <div class="card"><div class="stat" id="retrying">—</div><div class="label">Retrying</div></div>
    <div class="card"><div class="stat" id="completed">—</div><div class="label">Completed</div></div>
  </div>
  <div class="card">
    <div class="label">Running agents</div>
    <table>
      <thead><tr><th>#</th><th>Issue ID</th><th>Turns</th><th>Tokens</th><th>Running</th></tr></thead>
      <tbody id="running-table"></tbody>
    </table>
  </div>
  <div class="card">
    <div class="label">Retry queue</div>
    <table>
      <thead><tr><th>Issue ID</th><th>Attempt</th><th>Error</th></tr></thead>
      <tbody id="retry-table"></tbody>
    </table>
  </div>
  <script>
    // Safe helper: creates a <tr> with text-only <td> cells (no innerHTML).
    function makeRow(cells) {
      const tr = document.createElement('tr');
      cells.forEach(function(text) {
        const td = document.createElement('td');
        td.textContent = String(text == null ? '' : text);
        tr.appendChild(td);
      });
      return tr;
    }

    async function load() {
      const res = await fetch('/api/status');
      if (!res.ok) {
        document.getElementById('ts').textContent = ' — orchestrator unavailable (503)';
        return;
      }
      const d = await res.json();
      document.getElementById('running').textContent   = d.running_count;
      document.getElementById('retrying').textContent  = d.retrying_count;
      document.getElementById('completed').textContent = d.completed_count;
      document.getElementById('ts').textContent = ' — ' + new Date(d.generated_at).toLocaleTimeString();

      const runTbody = document.getElementById('running-table');
      runTbody.textContent = '';
      (d.running || []).forEach(function(e) {
        runTbody.appendChild(makeRow([
          e.identifier, e.issue_id, e.turn_count,
          e.total_tokens, Math.round(e.seconds_running) + 's'
        ]));
      });

      const retryTbody = document.getElementById('retry-table');
      retryTbody.textContent = '';
      (d.retrying || []).forEach(function(e) {
        retryTbody.appendChild(makeRow([e.issue_id, e.attempt, e.error]));
      });
    }

    load();
    setInterval(load, 5000);
  </script>
</body>
</html>"#;
}

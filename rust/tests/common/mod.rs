//! Shared test helpers for integration tests.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

use symphony::agent::{AgentError, AgentRunConfig, AgentRunner, AgentUpdate};
use symphony::config::AppConfig;
use symphony::domain::Issue;
use symphony::orchestrator::{Orchestrator, OrchestratorMsg};
use symphony::tracker::MemoryTracker;

/// Create a default AppConfig with fast polling for tests.
pub fn make_app_config() -> AppConfig {
    let mut config = AppConfig::default();
    config.polling.interval_ms = 50;
    config
}

/// Create a default AppConfig with a custom concurrency limit and fast polling.
pub fn make_app_config_with_concurrency(max: usize) -> AppConfig {
    let mut config = make_app_config();
    config.agent.max_concurrent_agents = max;
    config
}

/// Create a test Issue with state set to "open".
pub fn make_open_issue(id: &str, identifier: &str) -> Issue {
    let mut issue = Issue::new(id, identifier, "Test issue");
    issue.state = "open".to_string();
    issue
}

// ─── MockAgentRunner ──────────────────────────────────────────────────────────

/// Records which issue IDs were dispatched to it.
pub struct MockAgentRunner {
    pub dispatched: Arc<Mutex<Vec<String>>>,
    pub result: Result<(), ()>,
    /// Artificial delay to simulate a long-running agent (0 = instant)
    pub delay_ms: u64,
}

impl MockAgentRunner {
    pub fn success() -> Self {
        Self {
            dispatched: Arc::new(Mutex::new(Vec::new())),
            result: Ok(()),
            delay_ms: 0,
        }
    }

    /// Agent that takes `delay_ms` to complete — useful for concurrency tests
    pub fn slow_success(delay_ms: u64) -> Self {
        Self {
            dispatched: Arc::new(Mutex::new(Vec::new())),
            result: Ok(()),
            delay_ms,
        }
    }

    pub fn failure() -> Self {
        Self {
            dispatched: Arc::new(Mutex::new(Vec::new())),
            result: Err(()),
            delay_ms: 0,
        }
    }
}

#[async_trait]
impl AgentRunner for MockAgentRunner {
    async fn run(
        &self,
        issue: &Issue,
        _attempt: Option<u32>,
        _config: &AgentRunConfig,
        _update_tx: mpsc::UnboundedSender<(String, AgentUpdate)>,
        cancel: CancellationToken,
    ) -> Result<(), AgentError> {
        self.dispatched.lock().await.push(issue.id.clone());
        if self.delay_ms > 0 {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(self.delay_ms)) => {}
                _ = cancel.cancelled() => { return Ok(()); }
            }
        }
        match self.result {
            Ok(()) => Ok(()),
            Err(()) => Err(AgentError::TurnFailed("mock failure".to_string())),
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

pub fn make_config(max_concurrent: usize) -> AppConfig {
    make_app_config_with_concurrency(max_concurrent)
}

/// Run the orchestrator in background, fire one tick, then shut it down.
/// Returns the orchestrator's sender channel for control.
pub async fn run_orchestrator_for(
    tracker: MemoryTracker,
    agent: MockAgentRunner,
    config: AppConfig,
    run_duration: Duration,
) -> mpsc::UnboundedSender<OrchestratorMsg> {
    let (orchestrator, tx) = Orchestrator::new(tracker, agent, config);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        orchestrator.run(cancel_clone).await;
    });

    // Let it run, then shut down
    tokio::time::sleep(run_duration).await;
    cancel.cancel();

    tx
}

//! Orchestrator tests: state management, snapshots, token aggregation, workspace cleanup.

use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use symphony::agent::{AgentError, AgentRunConfig, AgentRunner, AgentUpdate};
use symphony::domain::Issue;
use symphony::orchestrator::{Orchestrator, OrchestratorMsg};
use symphony::tracker::{MemoryTracker, Tracker, TrackerError};

mod common;
use common::{make_config, make_open_issue, MockAgentRunner};

// ─── TokenReportingAgent ──────────────────────────────────────────────────────

/// Agent runner that sends AgentUpdate events with known token counts, then blocks
/// until cancelled so the running entry is still visible in the snapshot.
struct TokenReportingAgent {
    input_tokens: u64,
    output_tokens: u64,
}

#[async_trait]
impl AgentRunner for TokenReportingAgent {
    async fn run(
        &self,
        issue: &Issue,
        _attempt: Option<u32>,
        _config: &AgentRunConfig,
        update_tx: mpsc::UnboundedSender<(String, AgentUpdate)>,
        cancel: CancellationToken,
    ) -> Result<(), AgentError> {
        // Send token event immediately
        let _ = update_tx.send((
            issue.id.clone(),
            AgentUpdate::Event {
                event_type: "assistant".to_string(),
                message: Some("test message".to_string()),
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        ));
        // Block until cancelled so the running entry stays visible in snapshots
        cancel.cancelled().await;
        Ok(())
    }
}

/// Agent updates carrying token counts must be tracked in the running entry
/// and visible in the RuntimeSnapshot while the agent is still running.
#[tokio::test]
async fn agent_update_token_deltas_visible_in_snapshot() {
    let tracker = MemoryTracker::with_issues(vec![make_open_issue("I_1", "1")]);
    let agent = TokenReportingAgent { input_tokens: 100, output_tokens: 50 };

    let mut config = make_config(5);
    config.polling.interval_ms = 50;

    let (orchestrator, tx) = Orchestrator::new(tracker, agent, config);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        orchestrator.run(cancel_clone).await;
    });

    // Poll until the running entry shows the expected token counts.
    // The agent is blocked (not yet finished), so the entry remains in `running`.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let snapshot = loop {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = tx.send(OrchestratorMsg::SnapshotRequest { reply: reply_tx });
        let snap = timeout(Duration::from_millis(200), reply_rx).await.unwrap().unwrap();

        if snap.running.first().map(|e| e.input_tokens).unwrap_or(0) >= 100 {
            break snap;
        }

        assert!(tokio::time::Instant::now() < deadline, "Timed out waiting for token counts in running entry");
        tokio::time::sleep(Duration::from_millis(30)).await;
    };

    let entry = &snapshot.running[0];
    assert_eq!(entry.input_tokens, 100, "input_tokens in running entry should match the agent's event");
    assert_eq!(entry.output_tokens, 50, "output_tokens in running entry should match the agent's event");
    assert_eq!(entry.total_tokens, 150, "total_tokens should be sum of input and output");

    cancel.cancel();
}

// ─── Snapshot request ─────────────────────────────────────────────────────────

/// Requesting a snapshot from a running orchestrator returns a valid snapshot.
#[tokio::test]
async fn snapshot_returns_valid_data() {
    let tracker = MemoryTracker::new();
    let agent = MockAgentRunner::success();
    let config = make_config(5);

    let (orchestrator, tx) = Orchestrator::new(tracker, agent, config);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        orchestrator.run(cancel_clone).await;
    });

    // Give it a moment to start
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let _ = tx.send(OrchestratorMsg::SnapshotRequest { reply: reply_tx });

    let snapshot = timeout(Duration::from_millis(500), reply_rx)
        .await
        .expect("snapshot request should complete")
        .expect("snapshot channel should not be closed");

    assert_eq!(snapshot.running_count, 0);
    assert_eq!(snapshot.retrying_count, 0);

    cancel.cancel();
}

// ─── workspace cleanup tests ──────────────────────────────────────────────────

/// Tracker that returns issues normally on fetch_candidate_issues, but returns
/// them as closed on fetch_issues_by_ids (simulates issue closing between runs).
struct IssueClosedOnRetryTracker {
    inner: MemoryTracker,
}

impl IssueClosedOnRetryTracker {
    fn with_issues(issues: Vec<Issue>) -> Self {
        Self { inner: MemoryTracker::with_issues(issues) }
    }
}

#[async_trait]
impl Tracker for IssueClosedOnRetryTracker {
    async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError> {
        self.inner.fetch_candidate_issues().await
    }

    async fn fetch_issues_by_ids(&self, ids: &[String]) -> Result<Vec<Issue>, TrackerError> {
        // Return the issues as closed so handle_retry thinks they are inactive
        let mut issues = self.inner.fetch_issues_by_ids(ids).await?;
        for issue in &mut issues {
            issue.state = "closed".to_string();
        }
        Ok(issues)
    }

    async fn fetch_issues_by_states(&self, states: &[String]) -> Result<Vec<Issue>, TrackerError> {
        self.inner.fetch_issues_by_states(states).await
    }

    async fn add_label(&self, issue_identifier: &str, label: &str) -> Result<(), TrackerError> {
        self.inner.add_label(issue_identifier, label).await
    }

    async fn remove_label(&self, issue_identifier: &str, label: &str) -> Result<(), TrackerError> {
        self.inner.remove_label(issue_identifier, label).await
    }
}

/// When handle_retry finds the issue is no longer active (closed), cleanup_workspace
/// should be called. Verified by a before_remove hook creating a flag file.
#[tokio::test]
async fn cleanup_workspace_called_when_issue_closed_on_retry() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let workspace_root = temp_dir.path().join("workspaces");
    let flag_file = temp_dir.path().join("before_remove_ran");

    let tracker = IssueClosedOnRetryTracker::with_issues(vec![make_open_issue("I_1", "TEST-1")]);
    let agent = MockAgentRunner::success();

    let mut config = make_config(5);
    config.polling.interval_ms = 50;
    // Short backoff so retry fires quickly
    config.agent.max_retry_backoff_ms = 100;
    config.workspace.root = workspace_root.clone();
    config.hooks.before_remove = Some(format!("touch {}", flag_file.display()));

    let (orchestrator, _tx) = Orchestrator::new(tracker, agent, config);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        orchestrator.run(cancel_clone).await;
    });

    // Wait for the before_remove flag file to appear (created by cleanup_workspace)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if flag_file.exists() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Timed out waiting for before_remove hook to run"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Also verify the workspace directory itself was removed
    let workspace_dir = workspace_root.join("TEST-1");
    assert!(!workspace_dir.exists(), "Workspace directory should be removed after cleanup");

    cancel.cancel();
}

/// When handle_retry gets an empty result (issue not found in tracker), cleanup_workspace
/// should also be called.
#[tokio::test]
async fn cleanup_workspace_called_when_issue_not_found_on_retry() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let workspace_root = temp_dir.path().join("workspaces");
    let flag_file = temp_dir.path().join("before_remove_ran2");

    struct IssueNotFoundOnRetryTracker {
        inner: MemoryTracker,
    }

    #[async_trait]
    impl Tracker for IssueNotFoundOnRetryTracker {
        async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError> {
            self.inner.fetch_candidate_issues().await
        }

        async fn fetch_issues_by_ids(&self, _ids: &[String]) -> Result<Vec<Issue>, TrackerError> {
            Ok(vec![]) // issue not found
        }

        async fn fetch_issues_by_states(&self, states: &[String]) -> Result<Vec<Issue>, TrackerError> {
            self.inner.fetch_issues_by_states(states).await
        }

        async fn add_label(&self, issue_identifier: &str, label: &str) -> Result<(), TrackerError> {
            self.inner.add_label(issue_identifier, label).await
        }

        async fn remove_label(&self, issue_identifier: &str, label: &str) -> Result<(), TrackerError> {
            self.inner.remove_label(issue_identifier, label).await
        }
    }

    let tracker = IssueNotFoundOnRetryTracker { inner: MemoryTracker::with_issues(vec![make_open_issue("I_2", "TEST-2")]) };
    let agent = MockAgentRunner::success();

    let mut config = make_config(5);
    config.polling.interval_ms = 50;
    config.agent.max_retry_backoff_ms = 100;
    config.workspace.root = workspace_root.clone();
    config.hooks.before_remove = Some(format!("touch {}", flag_file.display()));

    let (orchestrator, _tx) = Orchestrator::new(tracker, agent, config);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        orchestrator.run(cancel_clone).await;
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if flag_file.exists() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Timed out waiting for before_remove hook to run (issue not found case)"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    cancel.cancel();
}

// ─── FastForOneAgent ──────────────────────────────────────────────────────────

/// Agent that finishes instantly for one issue ID and blocks (until cancelled) for all others.
struct FastForOneAgent {
    fast_id: String,
}

#[async_trait]
impl AgentRunner for FastForOneAgent {
    async fn run(
        &self,
        issue: &Issue,
        _attempt: Option<u32>,
        _config: &AgentRunConfig,
        _update_tx: mpsc::UnboundedSender<(String, AgentUpdate)>,
        cancel: CancellationToken,
    ) -> Result<(), AgentError> {
        if issue.id == self.fast_id {
            Ok(()) // finish instantly
        } else {
            cancel.cancelled().await; // block until orchestrator shuts down
            Ok(())
        }
    }
}

/// When handle_retry fires but all concurrency slots are occupied by other running issues,
/// the workspace must NOT be cleaned up — the issue is still open and will be re-dispatched.
///
/// Flow:
///   1. I_target dispatched, finishes instantly (success) -> enters retry_attempts (1s delay)
///   2. I_blocker added to tracker and dispatched (blocks until cancel) -> occupies the single slot
///   3. I_target's retry timer fires -> fetch returns active -> slot full -> claim released, NO cleanup
///   4. Workspace directory and before_remove hook must NOT have been triggered
#[tokio::test]
async fn workspace_preserved_when_slots_exhausted_on_retry() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let workspace_root = temp_dir.path().join("workspaces");
    let flag_file = temp_dir.path().join("before_remove_must_not_run");

    // Start with only I_target
    let tracker = MemoryTracker::with_issues(vec![make_open_issue("I_target", "TARGET")]);
    let tracker_handle = tracker.clone();

    // I_target finishes instantly; I_blocker blocks until cancel
    let agent = FastForOneAgent { fast_id: "I_target".to_string() };

    let mut config = make_config(1); // only 1 slot
    config.polling.interval_ms = 50;
    config.workspace.root = workspace_root.clone();
    config.hooks.before_remove = Some(format!("touch {}", flag_file.display()));

    let (orchestrator, tx) = Orchestrator::new(tracker, agent, config);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        orchestrator.run(cancel_clone).await;
    });

    // Wait for I_target to finish and enter retry_attempts (retrying_count = 1, running_count = 0)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = tx.send(OrchestratorMsg::SnapshotRequest { reply: reply_tx });
        let snap = timeout(Duration::from_millis(200), reply_rx).await.unwrap().unwrap();
        if snap.retrying_count == 1 && snap.running_count == 0 {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "Timed out waiting for I_target to enter retry queue");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Add I_blocker to occupy the single slot before I_target's 1s retry fires
    tracker_handle.add_issue(make_open_issue("I_blocker", "BLOCKER")).await;

    // Wait for I_blocker to be dispatched (running_count = 1)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = tx.send(OrchestratorMsg::SnapshotRequest { reply: reply_tx });
        let snap = timeout(Duration::from_millis(200), reply_rx).await.unwrap().unwrap();
        if snap.running_count >= 1 {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "Timed out waiting for I_blocker to start running");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Wait past I_target's 1s retry delay so handle_retry fires and finds the slot full
    tokio::time::sleep(Duration::from_millis(1_200)).await;

    // Workspace directory must still exist
    let target_workspace = workspace_root.join("TARGET");
    assert!(target_workspace.exists(), "TARGET workspace should NOT be removed when issue is active but slot is exhausted");
    // before_remove hook must not have fired
    assert!(!flag_file.exists(), "before_remove hook must not fire when issue is still active (slots exhausted)");

    cancel.cancel();
}

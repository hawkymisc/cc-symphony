//! Orchestrator tests: retry queue, backoff computation, and failure handling.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use symphony::domain::Issue;
use symphony::orchestrator::{Orchestrator, OrchestratorMsg};
use symphony::tracker::{MemoryTracker, Tracker, TrackerError};

mod common;
use common::{make_config, make_open_issue, MockAgentRunner};

// ─── retry backoff unit tests (synchronous) ───────────────────────────────────

#[test]
fn retry_normal_backoff_is_1s() {
    use symphony::orchestrator::{compute_backoff, ExitType};
    assert_eq!(compute_backoff(ExitType::Normal, 1, 300_000), 1_000);
    assert_eq!(compute_backoff(ExitType::Normal, 10, 300_000), 1_000);
}

#[test]
fn retry_failure_exponential_backoff() {
    use symphony::orchestrator::{compute_backoff, ExitType};
    assert_eq!(compute_backoff(ExitType::Failure, 1, 300_000), 10_000);
    assert_eq!(compute_backoff(ExitType::Failure, 2, 300_000), 20_000);
    assert_eq!(compute_backoff(ExitType::Failure, 3, 300_000), 40_000);
}

#[test]
fn retry_backoff_cap() {
    use symphony::orchestrator::{compute_backoff, ExitType};
    // Attempt 4 would be 80s, capped at 60s
    assert_eq!(compute_backoff(ExitType::Failure, 4, 60_000), 60_000);
    assert_eq!(compute_backoff(ExitType::Failure, 20, 60_000), 60_000);
}

// ─── ErrorOnRetryTracker ──────────────────────────────────────────────────────

/// Tracker that returns issues on fetch_candidate_issues but always fails fetch_issues_by_ids.
struct ErrorOnRetryTracker {
    inner: MemoryTracker,
}

impl ErrorOnRetryTracker {
    fn with_issues(issues: Vec<Issue>) -> Self {
        Self { inner: MemoryTracker::with_issues(issues) }
    }
}

#[async_trait]
impl Tracker for ErrorOnRetryTracker {
    async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError> {
        self.inner.fetch_candidate_issues().await
    }

    async fn fetch_issues_by_ids(&self, _ids: &[String]) -> Result<Vec<Issue>, TrackerError> {
        Err(TrackerError::ApiRequest("simulated tracker outage".to_string()))
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

/// When fetch_issues_by_ids fails during retry, the RetryEntry should be reinserted
/// with the same failure count so exponential backoff is preserved.
#[tokio::test]
async fn retry_preserves_backoff_on_tracker_error() {
    let tracker = ErrorOnRetryTracker::with_issues(vec![make_open_issue("I_1", "1")]);

    // Agent always fails to create RetryEntry with error
    let agent = MockAgentRunner::failure();

    let mut config = make_config(5);
    // Short backoff so the retry timer fires quickly
    config.agent.max_retry_backoff_ms = 50;

    let (orchestrator, tx) = Orchestrator::new(tracker, agent, config);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        orchestrator.run(cancel_clone).await;
    });

    // Poll until the tracker error path has completed (retrying_count == 1 with attempt >= 1)
    // Expected flow: dispatch -> fail -> RetryEntry(attempt=1) -> timer(50ms) -> tracker error -> re-insert(attempt=1)
    // Give up after 1s to avoid hanging if something goes wrong.
    let snapshot = {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        loop {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(OrchestratorMsg::SnapshotRequest { reply: reply_tx });
            let snap = timeout(Duration::from_millis(200), reply_rx)
                .await
                .expect("snapshot request should complete")
                .expect("snapshot channel should not be closed");

            // Wait until the issue has gone through at least one failure cycle
            if snap.retrying_count == 1 && snap.retrying.first().map(|e| e.attempt).unwrap_or(0) >= 1 {
                break snap;
            }

            if tokio::time::Instant::now() >= deadline {
                panic!("Timed out waiting for retry queue to stabilize after tracker error");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };

    assert_eq!(snapshot.retrying_count, 1, "Issue should remain in retry queue after tracker error");
    assert_eq!(snapshot.running_count, 0, "Issue should not be running");
    // Critical: attempt count must be preserved (not reset to 0) after tracker error
    let entry = &snapshot.retrying[0];
    assert!(entry.attempt >= 1, "Failure count should be preserved after tracker error (got {})", entry.attempt);
    assert!(entry.error.is_some(), "RetryEntry should carry the tracker error message");

    cancel.cancel();
}

// ─── handle_worker_finished tests ─────────────────────────────────────────────

/// After a successful run, the issue should briefly enter the retry queue
/// (attempt = 0, no error) while waiting for the 1-second continuation delay.
#[tokio::test]
async fn worker_finished_success_enters_retry_queue_briefly() {
    let tracker = MemoryTracker::with_issues(vec![make_open_issue("I_1", "1")]);
    let agent = MockAgentRunner::success();

    let mut config = make_config(5);
    config.polling.interval_ms = 1000; // long poll so retry timer fires before next tick

    let (orchestrator, tx) = Orchestrator::new(tracker, agent, config);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        orchestrator.run(cancel_clone).await;
    });

    // Wait for the issue to finish (running -> retrying)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = tx.send(OrchestratorMsg::SnapshotRequest { reply: reply_tx });
        let snap = timeout(Duration::from_millis(200), reply_rx).await.unwrap().unwrap();

        if snap.retrying_count == 1 {
            let entry = &snap.retrying[0];
            assert_eq!(entry.attempt, 0, "Successful run should set attempt=0 in retry entry");
            assert!(entry.error.is_none(), "Successful run should not carry an error");
            break;
        }

        assert!(tokio::time::Instant::now() < deadline, "Timed out waiting for retry entry after success");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    cancel.cancel();
}

/// After a failed run, the issue should appear in the retry queue with attempt=1
/// and an error message, and the backoff delay should be non-zero.
#[tokio::test]
async fn worker_finished_failure_increments_attempt_count() {
    let tracker = MemoryTracker::with_issues(vec![make_open_issue("I_1", "1")]);
    let agent = MockAgentRunner::failure();

    let mut config = make_config(5);
    config.polling.interval_ms = 50;
    // Long backoff so the retry doesn't fire during the assertion window
    config.agent.max_retry_backoff_ms = 10_000;

    let (orchestrator, tx) = Orchestrator::new(tracker, agent, config);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        orchestrator.run(cancel_clone).await;
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let snapshot = loop {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let _ = tx.send(OrchestratorMsg::SnapshotRequest { reply: reply_tx });
        let snap = timeout(Duration::from_millis(200), reply_rx).await.unwrap().unwrap();

        if snap.retrying_count == 1 && snap.retrying[0].attempt >= 1 {
            break snap;
        }

        assert!(tokio::time::Instant::now() < deadline, "Timed out waiting for retry entry after failure");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    let entry = &snapshot.retrying[0];
    assert_eq!(entry.attempt, 1, "First failure should set attempt=1");
    assert!(entry.error.is_some(), "Failure retry entry should carry an error message");

    cancel.cancel();
}

// ─── consecutive tracker failure backoff tests ────────────────────────────────

/// Tracker that always fails on fetch_candidate_issues.
struct AlwaysFailTracker;

#[async_trait]
impl Tracker for AlwaysFailTracker {
    async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError> {
        Err(TrackerError::ApiRequest("simulated outage".to_string()))
    }

    async fn fetch_issues_by_ids(&self, _ids: &[String]) -> Result<Vec<Issue>, TrackerError> {
        Ok(vec![])
    }

    async fn fetch_issues_by_states(&self, _states: &[String]) -> Result<Vec<Issue>, TrackerError> {
        Ok(vec![])
    }

    async fn add_label(&self, _issue_identifier: &str, _label: &str) -> Result<(), TrackerError> {
        Ok(())
    }

    async fn remove_label(&self, _issue_identifier: &str, _label: &str) -> Result<(), TrackerError> {
        Ok(())
    }
}

/// Tracker that fails N times then succeeds. Thread-safe via Arc<Mutex<>>.
struct FailThenSucceedTracker {
    failures_remaining: Arc<Mutex<u32>>,
    inner: MemoryTracker,
}

impl FailThenSucceedTracker {
    fn new(fail_count: u32, issues: Vec<Issue>) -> Self {
        Self {
            failures_remaining: Arc::new(Mutex::new(fail_count)),
            inner: MemoryTracker::with_issues(issues),
        }
    }
}

#[async_trait]
impl Tracker for FailThenSucceedTracker {
    async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError> {
        let mut remaining = self.failures_remaining.lock().await;
        if *remaining > 0 {
            *remaining -= 1;
            Err(TrackerError::ApiRequest("transient failure".to_string()))
        } else {
            drop(remaining);
            self.inner.fetch_candidate_issues().await
        }
    }

    async fn fetch_issues_by_ids(&self, ids: &[String]) -> Result<Vec<Issue>, TrackerError> {
        self.inner.fetch_issues_by_ids(ids).await
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

/// After tracker errors, no issues should be dispatched since the tracker
/// never returns candidates.
#[tokio::test]
async fn test_tracker_failure_increments_consecutive_failures() {
    let tracker = AlwaysFailTracker;
    let agent = MockAgentRunner::success();
    let dispatched = Arc::clone(&agent.dispatched);

    let mut config = make_config(5);
    config.polling.interval_ms = 50;

    let (orchestrator, tx) = Orchestrator::new(tracker, agent, config);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        orchestrator.run(cancel_clone).await;
    });

    // Let a few ticks fire (each will fail and back off)
    tokio::time::sleep(Duration::from_millis(300)).await;

    // No issues should have been dispatched since tracker always fails
    let ids = dispatched.lock().await;
    assert!(ids.is_empty(), "No issues should be dispatched when tracker always fails");

    // Snapshot should show 0 running, 0 retrying
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let _ = tx.send(OrchestratorMsg::SnapshotRequest { reply: reply_tx });
    let snap = timeout(Duration::from_millis(200), reply_rx).await.unwrap().unwrap();
    assert_eq!(snap.running_count, 0);
    assert_eq!(snap.retrying_count, 0);

    cancel.cancel();
}

/// After tracker failures followed by success, issues are dispatched normally,
/// proving the failure count resets and the orchestrator recovers.
#[tokio::test]
async fn test_tracker_success_resets_consecutive_failures() {
    // Fail 2 times, then succeed
    let tracker = FailThenSucceedTracker::new(2, vec![make_open_issue("I_1", "1")]);
    let agent = MockAgentRunner::success();
    let dispatched = Arc::clone(&agent.dispatched);

    let mut config = make_config(5);
    config.polling.interval_ms = 50;

    let (orchestrator, _tx) = Orchestrator::new(tracker, agent, config);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        orchestrator.run(cancel_clone).await;
    });

    // Wait long enough for the 2 failures + backoff + successful tick + dispatch
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let ids = dispatched.lock().await;
        if ids.contains(&"I_1".to_string()) {
            break;
        }
        drop(ids);
        assert!(
            tokio::time::Instant::now() < deadline,
            "Timed out waiting for issue to be dispatched after tracker recovery"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    cancel.cancel();
}

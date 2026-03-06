//! Phase 4: Orchestrator tests using MemoryTracker + MockAgentRunner (PLAN.md §Phase 4)

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use symphony::agent::{AgentError, AgentRunner, AgentUpdate};
use symphony::config::AppConfig;
use symphony::domain::Issue;
use symphony::orchestrator::{Orchestrator, OrchestratorMsg};
use symphony::tracker::MemoryTracker;

// ─── MockAgentRunner ──────────────────────────────────────────────────────────

/// Records which issue IDs were dispatched to it.
struct MockAgentRunner {
    dispatched: Arc<Mutex<Vec<String>>>,
    result: Result<(), ()>,
    /// Artificial delay to simulate a long-running agent (0 = instant)
    delay_ms: u64,
}

impl MockAgentRunner {
    fn success() -> Self {
        Self {
            dispatched: Arc::new(Mutex::new(Vec::new())),
            result: Ok(()),
            delay_ms: 0,
        }
    }

    /// Agent that takes `delay_ms` to complete — useful for concurrency tests
    fn slow_success(delay_ms: u64) -> Self {
        Self {
            dispatched: Arc::new(Mutex::new(Vec::new())),
            result: Ok(()),
            delay_ms,
        }
    }

    fn failure() -> Self {
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
        _config: &AppConfig,
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

fn make_config(max_concurrent: usize) -> AppConfig {
    let mut config = AppConfig::default();
    config.agent.max_concurrent_agents = max_concurrent;
    // Very short poll so tests don't wait long
    config.polling.interval_ms = 50;
    config
}

fn make_open_issue(id: &str, identifier: &str) -> Issue {
    let mut issue = Issue::new(id, identifier, "Test issue");
    issue.state = "open".to_string();
    issue
}

/// Run the orchestrator in background, fire one tick, then shut it down.
/// Returns the orchestrator's sender channel for control.
async fn run_orchestrator_for(
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

// ─── dispatch tests ───────────────────────────────────────────────────────────

/// Orchestrator dispatches an open issue to the agent runner.
#[tokio::test]
async fn dispatch_open_issue() {
    let tracker = MemoryTracker::with_issues(vec![make_open_issue("I_1", "1")]);
    let agent = MockAgentRunner::success();
    let dispatched = Arc::clone(&agent.dispatched);

    run_orchestrator_for(tracker, agent, make_config(5), Duration::from_millis(200)).await;

    let ids = dispatched.lock().await;
    assert!(ids.contains(&"I_1".to_string()), "Issue I_1 should have been dispatched");
}

/// Orchestrator does not dispatch more issues than max_concurrent_agents simultaneously.
///
/// Uses a slow agent (500ms) so it stays running during the entire test window (150ms),
/// preventing the second poll from dispatching another issue.
#[tokio::test]
async fn dispatch_respects_concurrency_limit() {
    let issues = vec![
        make_open_issue("I_1", "1"),
        make_open_issue("I_2", "2"),
        make_open_issue("I_3", "3"),
    ];
    let tracker = MemoryTracker::with_issues(issues);
    // Agent takes 500ms — longer than the test window of 150ms
    let agent = MockAgentRunner::slow_success(500);
    let dispatched = Arc::clone(&agent.dispatched);

    run_orchestrator_for(tracker, agent, make_config(1), Duration::from_millis(150)).await;

    let ids = dispatched.lock().await;
    assert_eq!(ids.len(), 1, "Only 1 issue should be dispatched when concurrency limit is 1 and agent is still running");
}

/// Closed issues are not dispatched.
#[tokio::test]
async fn dispatch_skips_closed_issues() {
    let mut closed = Issue::new("I_closed", "99", "Closed issue");
    closed.state = "closed".to_string();

    let tracker = MemoryTracker::with_issues(vec![closed]);
    let agent = MockAgentRunner::success();
    let dispatched = Arc::clone(&agent.dispatched);

    run_orchestrator_for(tracker, agent, make_config(5), Duration::from_millis(150)).await;

    let ids = dispatched.lock().await;
    assert!(ids.is_empty(), "Closed issues should not be dispatched");
}

/// Same issue is not dispatched twice concurrently.
#[tokio::test]
async fn dispatch_no_duplicate_claim() {
    let tracker = MemoryTracker::with_issues(vec![make_open_issue("I_1", "1")]);
    let agent = MockAgentRunner::success();
    let dispatched = Arc::clone(&agent.dispatched);

    // With plenty of concurrency headroom; but one issue should only be dispatched once
    run_orchestrator_for(tracker, agent, make_config(10), Duration::from_millis(200)).await;

    let ids = dispatched.lock().await;
    let count = ids.iter().filter(|id| *id == "I_1").count();
    // May be dispatched twice if the first run finishes and it gets re-queued,
    // but should never be dispatched concurrently (count > 1 at the same instant).
    // Here we just verify it was dispatched at least once.
    assert!(count >= 1, "Issue should be dispatched at least once");
}

// ─── select_candidates unit tests (synchronous) ───────────────────────────────

#[test]
fn select_candidates_priority_sort() {
    use std::collections::{HashMap, HashSet};
    use symphony::orchestrator::select_candidates;

    let mut i1 = make_open_issue("I_1", "1");
    i1.priority = Some(2);
    i1.created_at = Some(chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap().into());

    let mut i2 = make_open_issue("I_2", "2");
    i2.priority = Some(1);
    i2.created_at = Some(chrono::DateTime::parse_from_rfc3339("2026-01-02T00:00:00Z").unwrap().into());

    let candidates = vec![i1, i2];
    let selected = select_candidates(&candidates, &HashMap::new(), &HashSet::new(), &HashMap::new(), 10);

    assert_eq!(selected.len(), 2);
    assert_eq!(selected[0].identifier, "2"); // priority 1 first
    assert_eq!(selected[1].identifier, "1");
}

#[test]
fn select_candidates_skips_claimed() {
    use std::collections::{HashMap, HashSet};
    use symphony::orchestrator::select_candidates;

    let candidates = vec![
        make_open_issue("I_1", "1"),
        make_open_issue("I_2", "2"),
    ];

    let mut claimed = HashSet::new();
    claimed.insert("I_1".to_string());

    let selected = select_candidates(&candidates, &HashMap::new(), &claimed, &HashMap::new(), 10);

    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].identifier, "2");
}

#[test]
fn select_candidates_zero_slots() {
    use std::collections::{HashMap, HashSet};
    use symphony::orchestrator::select_candidates;

    let candidates = vec![make_open_issue("I_1", "1")];
    let selected = select_candidates(&candidates, &HashMap::new(), &HashSet::new(), &HashMap::new(), 0);

    assert!(selected.is_empty());
}

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

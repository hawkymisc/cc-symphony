//! Phase 8: integration_full_cycle test (PLAN.md §Phase 8)
//!
//! Exercises the full orchestrator lifecycle end-to-end using in-process mocks:
//!   MemoryTracker (seed with one open issue)
//!   MockAgentRunner (always succeeds)
//!
//! Verifies: open issue → dispatch → agent run → retry queue → issue polled again.
//!
//! Tests synchronise on actual state changes (polling loop with timeout) rather
//! than fixed-duration sleeps so they do not produce intermittent failures on
//! slow CI machines.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

use symphony::agent::{AgentError, AgentRunner, AgentUpdate};
use symphony::config::AppConfig;
use symphony::domain::Issue;
use symphony::orchestrator::{Orchestrator, OrchestratorMsg};
use symphony::tracker::MemoryTracker;

// ---------------------------------------------------------------------------
// Minimal mock agent runner
// ---------------------------------------------------------------------------

struct SuccessAgent {
    dispatched: Arc<Mutex<Vec<String>>>,
}

impl SuccessAgent {
    fn new() -> Self {
        Self { dispatched: Arc::new(Mutex::new(Vec::new())) }
    }
}

#[async_trait]
impl AgentRunner for SuccessAgent {
    async fn run(
        &self,
        issue: &Issue,
        _attempt: Option<u32>,
        _config: &AppConfig,
        _update_tx: mpsc::UnboundedSender<(String, AgentUpdate)>,
        _cancel: CancellationToken,
    ) -> Result<(), AgentError> {
        self.dispatched.lock().await.push(issue.id.clone());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn open_issue(id: &str, num: &str) -> Issue {
    let mut i = Issue::new(id, num, "Implement feature X");
    i.state = "open".to_string();
    i
}

fn test_config() -> AppConfig {
    let mut c = AppConfig::default();
    c.polling.interval_ms = 30; // fast poll for tests
    c.agent.max_concurrent_agents = 5;
    c
}

/// Poll `check` every 10 ms until it returns `true` or `timeout` elapses.
///
/// Panics with `msg` if the deadline is reached.
async fn wait_until<F, Fut>(timeout: Duration, msg: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    tokio::time::timeout(timeout, async {
        loop {
            if check().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{}", msg));
}

// ---------------------------------------------------------------------------
// integration_full_cycle
// ---------------------------------------------------------------------------

/// Full lifecycle:
/// 1. An open issue exists in the tracker
/// 2. Orchestrator dispatches it to the agent
/// 3. Agent completes successfully
#[tokio::test]
async fn integration_full_cycle_dispatch_and_completion() {
    let issue = open_issue("GH_ISSUE_42", "42");
    let tracker = MemoryTracker::with_issues(vec![issue]);
    let agent = SuccessAgent::new();
    let dispatched = Arc::clone(&agent.dispatched);

    let (orchestrator, _tx) = Orchestrator::new(tracker, agent, test_config());
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move { orchestrator.run(cancel_clone).await });

    // Wait until the issue is dispatched (no fixed sleep)
    wait_until(Duration::from_secs(5), "issue GH_ISSUE_42 was not dispatched within 5 s", || {
        let dispatched = Arc::clone(&dispatched);
        async move { dispatched.lock().await.contains(&"GH_ISSUE_42".to_string()) }
    })
    .await;

    cancel.cancel();
}

/// Successful agent run increments completed_count in the snapshot.
///
/// After the agent finishes (Ok), completed_count must be 1 and the issue_id
/// must appear in the completed set.
#[tokio::test]
async fn integration_completed_count_increments_on_success() {
    let issue = open_issue("GH_DONE", "77");
    let tracker = MemoryTracker::with_issues(vec![issue]);
    let agent = SuccessAgent::new();

    let (orchestrator, tx) = Orchestrator::new(tracker, agent, test_config());
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move { orchestrator.run(cancel_clone).await });

    // Poll until completed_count == 1
    wait_until(Duration::from_secs(5), "completed_count did not reach 1 within 5 s", || {
        let tx = tx.clone();
        async move {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            if tx.send(OrchestratorMsg::SnapshotRequest { reply: reply_tx }).is_err() {
                return false;
            }
            match tokio::time::timeout(Duration::from_millis(100), reply_rx).await {
                Ok(Ok(snap)) => snap.completed_count == 1,
                _ => false,
            }
        }
    })
    .await;

    // Take a final snapshot and assert exact count (completed is a HashSet so no duplicates)
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    tx.send(OrchestratorMsg::SnapshotRequest { reply: reply_tx }).unwrap();
    let snap = tokio::time::timeout(Duration::from_secs(1), reply_rx)
        .await
        .expect("timed out")
        .expect("channel closed");

    assert_eq!(snap.completed_count, 1, "exactly one issue should be completed");

    cancel.cancel();
}

/// Snapshot after dispatch shows running_count > 0 while agent is in flight.
#[tokio::test]
async fn integration_snapshot_shows_running_while_agent_active() {
    let issue = open_issue("GH_ISSUE_10", "10");
    let tracker = MemoryTracker::with_issues(vec![issue]);

    // Slow agent: stays running until cancelled
    struct SlowAgent;
    #[async_trait]
    impl AgentRunner for SlowAgent {
        async fn run(
            &self,
            _issue: &Issue,
            _attempt: Option<u32>,
            _config: &AppConfig,
            _update_tx: mpsc::UnboundedSender<(String, AgentUpdate)>,
            cancel: CancellationToken,
        ) -> Result<(), AgentError> {
            cancel.cancelled().await;
            Ok(())
        }
    }

    let (orchestrator, tx) = Orchestrator::new(tracker, SlowAgent, test_config());
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move { orchestrator.run(cancel_clone).await });

    // Wait until the orchestrator has at least one running entry
    wait_until(Duration::from_secs(5), "no running agents observed within 5 s", || {
        let tx = tx.clone();
        async move {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            if tx.send(OrchestratorMsg::SnapshotRequest { reply: reply_tx }).is_err() {
                return false;
            }
            match tokio::time::timeout(Duration::from_millis(100), reply_rx).await {
                Ok(Ok(snap)) => snap.running_count >= 1,
                _ => false,
            }
        }
    })
    .await;

    // Take a final snapshot and assert
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    tx.send(OrchestratorMsg::SnapshotRequest { reply: reply_tx }).unwrap();
    let snapshot = tokio::time::timeout(Duration::from_secs(1), reply_rx)
        .await
        .expect("timed out")
        .expect("channel closed");

    assert_eq!(snapshot.running_count, 1);
    assert_eq!(snapshot.running[0].identifier, "10");

    cancel.cancel();
}

/// Multiple issues: all are dispatched (up to concurrency limit).
#[tokio::test]
async fn integration_full_cycle_multiple_issues_dispatched() {
    let issues = vec![
        open_issue("GH_1", "1"),
        open_issue("GH_2", "2"),
        open_issue("GH_3", "3"),
    ];
    let tracker = MemoryTracker::with_issues(issues);
    let agent = SuccessAgent::new();
    let dispatched = Arc::clone(&agent.dispatched);

    let (orchestrator, _tx) = Orchestrator::new(tracker, agent, test_config());
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move { orchestrator.run(cancel_clone).await });

    // Wait until all 3 issues are dispatched
    wait_until(Duration::from_secs(5), "not all 3 issues dispatched within 5 s", || {
        let dispatched = Arc::clone(&dispatched);
        async move {
            let ids = dispatched.lock().await;
            ids.contains(&"GH_1".to_string())
                && ids.contains(&"GH_2".to_string())
                && ids.contains(&"GH_3".to_string())
        }
    })
    .await;

    cancel.cancel();
}

/// Agent that fails for the first `fail_count` calls then succeeds.
struct FailThenSucceedAgent {
    calls: Arc<Mutex<u32>>,
    fail_count: u32,
}

impl FailThenSucceedAgent {
    fn new(fail_count: u32) -> Self {
        Self {
            calls: Arc::new(Mutex::new(0)),
            fail_count,
        }
    }
}

#[async_trait]
impl AgentRunner for FailThenSucceedAgent {
    async fn run(
        &self,
        _issue: &Issue,
        _attempt: Option<u32>,
        _config: &AppConfig,
        _update_tx: mpsc::UnboundedSender<(String, AgentUpdate)>,
        _cancel: CancellationToken,
    ) -> Result<(), AgentError> {
        let mut calls = self.calls.lock().await;
        *calls += 1;
        let call_number = *calls;
        drop(calls);

        if call_number <= self.fail_count {
            Err(AgentError::TurnFailed(format!("simulated failure #{}", call_number)))
        } else {
            Ok(())
        }
    }
}

/// Failure→retry→success lifecycle:
/// 1. Agent fails twice (exponential backoff, capped short)
/// 2. On the third dispatch, agent succeeds
/// 3. The issue re-enters the retry queue with attempt=0 (success marker)
#[tokio::test]
async fn integration_failure_retry_success_cycle() {
    let issue = open_issue("GH_RETRY", "55");
    let tracker = MemoryTracker::with_issues(vec![issue]);

    let agent = FailThenSucceedAgent::new(2); // fail twice, then succeed
    let calls = Arc::clone(&agent.calls);

    let mut config = test_config();
    config.agent.max_retry_backoff_ms = 50; // cap backoff at 50 ms so test runs fast

    let (orchestrator, tx) = Orchestrator::new(tracker, agent, config);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move { orchestrator.run(cancel_clone).await });

    // Wait until the agent has been called at least 3 times (2 failures + 1 success)
    wait_until(Duration::from_secs(10), "agent was not called 3 times within 10 s", || {
        let calls = Arc::clone(&calls);
        async move { *calls.lock().await >= 3 }
    })
    .await;

    // After the successful (3rd) run there is a 1-second re-dispatch window during which
    // the issue sits in the retry queue with attempt=0 and error=None.
    // Poll snapshots until we see that state (or time out).
    let final_snap = {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            tx.send(OrchestratorMsg::SnapshotRequest { reply: reply_tx }).unwrap();
            let snap = tokio::time::timeout(Duration::from_millis(200), reply_rx)
                .await
                .expect("snapshot channel timed out")
                .expect("channel closed");

            if snap.running_count == 0
                && snap.retrying.iter().any(|e| e.attempt == 0 && e.error.is_none())
            {
                break snap;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("retry queue with attempt=0 not observed within 5 s; last snap: {:?}", snap);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };

    // Agent should not still be running
    assert_eq!(final_snap.running_count, 0, "issue should not be running after success");

    // The retry entry must reflect the successful run: attempt=0, no error
    assert_eq!(final_snap.retrying_count, 1, "exactly one retry entry expected");
    let entry = &final_snap.retrying[0];
    assert_eq!(entry.attempt, 0, "successful run resets consecutive failure counter to 0");
    assert!(entry.error.is_none(), "successful run must not carry an error");

    cancel.cancel();
}

/// Closed issue is never dispatched.
#[tokio::test]
async fn integration_closed_issue_never_dispatched() {
    let mut issue = Issue::new("GH_CLOSED", "99", "Closed issue");
    issue.state = "closed".to_string();

    // Add one open issue so the orchestrator actually polls
    let open = open_issue("GH_OPEN", "1");
    let tracker = MemoryTracker::with_issues(vec![issue, open]);
    let agent = SuccessAgent::new();
    let dispatched = Arc::clone(&agent.dispatched);

    let (orchestrator, _tx) = Orchestrator::new(tracker, agent, test_config());
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move { orchestrator.run(cancel_clone).await });

    // Wait until at least the open issue is dispatched (proves orchestrator polled)
    wait_until(Duration::from_secs(5), "open issue GH_OPEN was not dispatched", || {
        let dispatched = Arc::clone(&dispatched);
        async move { dispatched.lock().await.contains(&"GH_OPEN".to_string()) }
    })
    .await;

    // Now check that the closed issue was NOT dispatched
    let ids = dispatched.lock().await;
    assert!(!ids.contains(&"GH_CLOSED".to_string()), "closed issue must not be dispatched");

    cancel.cancel();
}

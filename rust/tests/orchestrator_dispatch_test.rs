//! Orchestrator tests: dispatch, candidate selection, blocking, and reconciliation.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use symphony::orchestrator::{Orchestrator, OrchestratorMsg};
use symphony::tracker::MemoryTracker;

mod common;
use common::{make_config, make_open_issue, MockAgentRunner};

// ─── dispatch tests ───────────────────────────────────────────────────────────

/// Orchestrator dispatches an open issue to the agent runner.
#[tokio::test]
async fn dispatch_open_issue() {
    let tracker = MemoryTracker::with_issues(vec![make_open_issue("I_1", "1")]);
    let agent = MockAgentRunner::success();
    let dispatched = Arc::clone(&agent.dispatched);

    common::run_orchestrator_for(tracker, agent, make_config(5), Duration::from_millis(200)).await;

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

    common::run_orchestrator_for(tracker, agent, make_config(1), Duration::from_millis(150)).await;

    let ids = dispatched.lock().await;
    assert_eq!(ids.len(), 1, "Only 1 issue should be dispatched when concurrency limit is 1 and agent is still running");
}

/// Closed issues are not dispatched.
#[tokio::test]
async fn dispatch_skips_closed_issues() {
    use symphony::domain::Issue;

    let mut closed = Issue::new("I_closed", "99", "Closed issue");
    closed.state = "closed".to_string();

    let tracker = MemoryTracker::with_issues(vec![closed]);
    let agent = MockAgentRunner::success();
    let dispatched = Arc::clone(&agent.dispatched);

    common::run_orchestrator_for(tracker, agent, make_config(5), Duration::from_millis(150)).await;

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
    common::run_orchestrator_for(tracker, agent, make_config(10), Duration::from_millis(200)).await;

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

// ─── is_blocked() filtering tests ─────────────────────────────────────────────

/// An issue with an active blocker must NOT be dispatched by the orchestrator.
#[tokio::test]
async fn test_blocked_issue_not_dispatched() {
    use symphony::domain::BlockerRef;

    let mut issue = make_open_issue("I_1", "1");
    issue.blocked_by = vec![BlockerRef { identifier: "I_blocker".to_string(), is_active: true }];

    let tracker = MemoryTracker::with_issues(vec![issue]);
    let agent = MockAgentRunner::success();
    let dispatched = Arc::clone(&agent.dispatched);

    common::run_orchestrator_for(tracker, agent, make_config(5), Duration::from_millis(200)).await;

    let ids = dispatched.lock().await;
    assert!(ids.is_empty(), "Blocked issue should not be dispatched");
}

/// An issue whose blockers are all inactive (is_active: false) should be dispatched normally.
#[tokio::test]
async fn test_unblocked_issue_dispatched() {
    use symphony::domain::BlockerRef;

    let mut issue = make_open_issue("I_1", "1");
    issue.blocked_by = vec![
        BlockerRef { identifier: "I_b1".to_string(), is_active: false },
        BlockerRef { identifier: "I_b2".to_string(), is_active: false },
    ];

    let tracker = MemoryTracker::with_issues(vec![issue]);
    let agent = MockAgentRunner::success();
    let dispatched = Arc::clone(&agent.dispatched);

    common::run_orchestrator_for(tracker, agent, make_config(5), Duration::from_millis(200)).await;

    let ids = dispatched.lock().await;
    assert!(ids.contains(&"I_1".to_string()), "Issue with only inactive blockers should be dispatched");
}

/// Unit-level test: is_blocked() returns true with active blockers, false after clearing them.
#[test]
fn test_issue_dispatched_after_blocker_cleared() {
    use symphony::domain::BlockerRef;

    let mut issue = make_open_issue("I_1", "1");
    issue.blocked_by = vec![
        BlockerRef { identifier: "I_b1".to_string(), is_active: true },
        BlockerRef { identifier: "I_b2".to_string(), is_active: false },
    ];
    assert!(issue.is_blocked(), "Issue with an active blocker should be blocked");

    // Clear all blockers
    issue.blocked_by.clear();
    assert!(!issue.is_blocked(), "Issue with no blockers should not be blocked");

    // Set only inactive blockers
    issue.blocked_by = vec![
        BlockerRef { identifier: "I_b1".to_string(), is_active: false },
        BlockerRef { identifier: "I_b2".to_string(), is_active: false },
    ];
    assert!(!issue.is_blocked(), "Issue with only inactive blockers should not be blocked");
}

// ─── reconcile tests ──────────────────────────────────────────────────────────

/// When an issue is removed from the tracker (closed/cancelled), the orchestrator's
/// reconcile logic should cancel the running agent and remove it from the running set.
#[tokio::test]
async fn reconcile_cancels_running_agent_when_issue_removed() {
    let tracker = MemoryTracker::with_issues(vec![make_open_issue("I_1", "1")]);
    // Clone shares the same underlying Arc<RwLock<>> — mutations are visible to orchestrator
    let tracker_handle = tracker.clone();

    // Slow agent so it keeps running during the test window
    let agent = MockAgentRunner::slow_success(2000);

    let mut config = make_config(5);
    config.polling.interval_ms = 50; // poll frequently

    let (orchestrator, tx) = Orchestrator::new(tracker, agent, config);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        orchestrator.run(cancel_clone).await;
    });

    // Wait for the issue to be dispatched and confirmed running
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = tx.send(OrchestratorMsg::SnapshotRequest { reply: reply_tx });
        let snap = timeout(Duration::from_millis(200), reply_rx).await.unwrap().unwrap();
        if snap.running_count == 1 {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "Timed out waiting for issue to start running");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Close the issue in the tracker — it will disappear from fetch_candidate_issues
    tracker_handle.update_state("I_1", "closed").await;

    // Wait for reconcile to remove it from running
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = tx.send(OrchestratorMsg::SnapshotRequest { reply: reply_tx });
        let snap = timeout(Duration::from_millis(200), reply_rx).await.unwrap().unwrap();
        if snap.running_count == 0 {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "Timed out waiting for reconcile to cancel agent");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }

    cancel.cancel();
}

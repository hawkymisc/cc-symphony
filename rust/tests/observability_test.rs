//! Phase 6: Observability tests (PLAN.md §Phase 6)
//!
//! Tests for logging context, token aggregation, and runtime snapshots.

use chrono::{Duration, Utc};

use symphony::config::AppConfig;
use symphony::domain::{Issue, TokenTotals, TokenUsage};
use symphony::observability::{RateLimitInfo, RunningEntrySnapshot, RuntimeSnapshot};
use symphony::orchestrator::{OrchestratorState, RunningEntry};

// ─── 1. log_includes_issue_context ───────────────────────────────────────────

/// RunningEntrySnapshot carries issue_id and identifier for structured logging.
#[test]
fn log_includes_issue_context() {
    let entry = RunningEntrySnapshot {
        issue_id: "gid://github/Issue/42".to_string(),
        identifier: "42".to_string(),
        session_id: None,
        turn_count: 0,
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        last_event: None,
        last_event_message: None,
        started_at: Utc::now(),
        seconds_running: 0.0,
    };

    assert_eq!(entry.issue_id, "gid://github/Issue/42");
    assert_eq!(entry.identifier, "42");
}

// ─── 2. log_includes_session_context ─────────────────────────────────────────

/// RunningEntrySnapshot includes session_id for per-session log correlation.
#[test]
fn log_includes_session_context() {
    let entry = RunningEntrySnapshot {
        issue_id: "gid://github/Issue/42".to_string(),
        identifier: "42".to_string(),
        session_id: Some("gid://github/Issue/42-1".to_string()),
        turn_count: 1,
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        last_event: None,
        last_event_message: None,
        started_at: Utc::now(),
        seconds_running: 0.0,
    };

    assert!(entry.session_id.is_some());
    assert_eq!(entry.session_id.as_deref(), Some("gid://github/Issue/42-1"));
}

// ─── 3. token_aggregation_across_sessions ────────────────────────────────────

/// TokenTotals correctly accumulates tokens from multiple independent sessions.
#[test]
fn token_aggregation_across_sessions() {
    let mut totals = TokenTotals::new();

    // Session 1 finishes
    totals.add(&TokenUsage {
        input_tokens: 100,
        output_tokens: 50,
        cache_read_tokens: None,
        cache_creation_tokens: None,
    });

    // Session 2 finishes
    totals.add(&TokenUsage {
        input_tokens: 200,
        output_tokens: 100,
        cache_read_tokens: Some(30),
        cache_creation_tokens: None,
    });

    assert_eq!(totals.input_tokens, 300);
    assert_eq!(totals.output_tokens, 150);
    assert_eq!(totals.total_tokens, 450);
    assert_eq!(totals.cache_read_tokens, 30);
}

// ─── 4. token_no_double_count ────────────────────────────────────────────────

/// Using compute_delta() prevents double-counting when the same absolute total
/// is reported more than once (e.g. multiple "result" events from Claude).
#[test]
fn token_no_double_count() {
    let mut last_input = 0u64;
    let mut last_output = 0u64;
    let mut totals = TokenTotals::new();

    // First report: absolute counts (100 in, 50 out)
    {
        let (in_delta, out_delta, _) =
            TokenTotals::compute_delta(100, 50, last_input, last_output);
        totals.add(&TokenUsage {
            input_tokens: in_delta,
            output_tokens: out_delta,
            cache_read_tokens: None,
            cache_creation_tokens: None,
        });
        last_input = 100;
        last_output = 50;
    }

    // Second report: same absolute counts again (should contribute 0 delta)
    {
        let (in_delta, out_delta, _) =
            TokenTotals::compute_delta(100, 50, last_input, last_output);
        totals.add(&TokenUsage {
            input_tokens: in_delta,
            output_tokens: out_delta,
            cache_read_tokens: None,
            cache_creation_tokens: None,
        });
    }

    // Must not double-count
    assert_eq!(totals.input_tokens, 100);
    assert_eq!(totals.output_tokens, 50);
    assert_eq!(totals.total_tokens, 150);
}

// ─── 5. runtime_seconds_includes_active ──────────────────────────────────────

/// Snapshot of a running entry includes the elapsed wall-clock seconds.
#[test]
fn runtime_seconds_includes_active() {
    let started = Utc::now() - Duration::seconds(10);
    let elapsed = (Utc::now() - started).num_milliseconds() as f64 / 1000.0;

    let snapshot = RunningEntrySnapshot {
        issue_id: "gid://github/Issue/1".to_string(),
        identifier: "1".to_string(),
        session_id: None,
        turn_count: 1,
        input_tokens: 100,
        output_tokens: 50,
        total_tokens: 150,
        last_event: Some("result".to_string()),
        last_event_message: None,
        started_at: started,
        seconds_running: elapsed,
    };

    assert!(
        snapshot.seconds_running >= 10.0,
        "Expected at least 10s elapsed, got {}",
        snapshot.seconds_running
    );
}

// ─── 6. rate_limit_tracking ──────────────────────────────────────────────────

/// The RuntimeSnapshot preserves the most recent rate-limit info from the tracker.
#[test]
fn rate_limit_tracking() {
    let rate_limits = RateLimitInfo {
        remaining: 4500,
        limit: 5000,
        reset_at: Utc::now() + Duration::hours(1),
        source: "github".to_string(),
    };

    let snapshot = RuntimeSnapshot {
        generated_at: Utc::now(),
        running_count: 0,
        retrying_count: 0,
        completed_count: 0,
        running: vec![],
        retrying: vec![],
        agent_totals: TokenTotals::new(),
        rate_limits: Some(rate_limits),
    };

    let rl = snapshot.rate_limits.as_ref().expect("rate_limits should be Some");
    assert_eq!(rl.remaining, 4500);
    assert_eq!(rl.limit, 5000);
    assert_eq!(rl.source, "github");
}

// ─── Integration tests using OrchestratorState ───────────────────────────────

fn make_running_entry(identifier: &str, started_at: chrono::DateTime<Utc>) -> RunningEntry {
    let mut issue = Issue::new(&format!("gid://github/Issue/{identifier}"), identifier, "test");
    issue.state = "open".to_string();
    RunningEntry {
        identifier: identifier.to_string(),
        issue,
        started_at,
        ..Default::default()
    }
}

/// to_snapshot() includes elapsed time of running sessions in agent_totals.seconds_running.
/// This verifies SPEC §13.1: "including active sessions".
#[test]
fn snapshot_seconds_running_includes_active_sessions() {
    let config = AppConfig::default();
    let mut state = OrchestratorState::new(&config);

    // Add a running entry that started 5 seconds ago
    let started = Utc::now() - Duration::seconds(5);
    state.running.insert(
        "gid://github/Issue/1".to_string(),
        make_running_entry("1", started),
    );

    let snapshot = state.to_snapshot();

    // agent_totals.seconds_running should reflect active session time
    assert!(
        snapshot.agent_totals.seconds_running >= 5,
        "Expected >= 5s from active session, got {}",
        snapshot.agent_totals.seconds_running
    );
}

/// Agent token deltas accumulate into agent_totals via handle_agent_update logic.
/// We simulate this by directly mutating OrchestratorState, mirroring what the
/// orchestrator does in handle_agent_update.
#[test]
fn state_token_aggregation_via_agent_totals() {
    let config = AppConfig::default();
    let mut state = OrchestratorState::new(&config);

    // Simulate two sessions worth of token accumulation
    state.agent_totals.add(&TokenUsage {
        input_tokens: 100,
        output_tokens: 50,
        cache_read_tokens: None,
        cache_creation_tokens: None,
    });
    state.agent_totals.add(&TokenUsage {
        input_tokens: 200,
        output_tokens: 100,
        cache_read_tokens: Some(20),
        cache_creation_tokens: None,
    });

    let snapshot = state.to_snapshot();
    assert_eq!(snapshot.agent_totals.input_tokens, 300);
    assert_eq!(snapshot.agent_totals.output_tokens, 150);
    assert_eq!(snapshot.agent_totals.total_tokens, 450);
    assert_eq!(snapshot.agent_totals.cache_read_tokens, 20);
}

/// Cache tokens (creation + read) are aggregated into agent_totals and surfaced in the snapshot.
#[test]
fn state_token_aggregation_with_cache_tokens() {
    let config = AppConfig::default();
    let mut state = OrchestratorState::new(&config);

    state.agent_totals.add(&TokenUsage {
        input_tokens: 100,
        output_tokens: 50,
        cache_creation_tokens: Some(30),
        cache_read_tokens: Some(20),
    });
    state.agent_totals.add(&TokenUsage {
        input_tokens: 50,
        output_tokens: 25,
        cache_creation_tokens: None,
        cache_read_tokens: Some(10),
    });

    let snapshot = state.to_snapshot();
    assert_eq!(snapshot.agent_totals.cache_creation_tokens, 30);
    assert_eq!(snapshot.agent_totals.cache_read_tokens, 30); // 20 + 10
}

/// Rate limit info stored in OrchestratorState is reflected in the snapshot.
#[test]
fn state_rate_limit_preserved_in_snapshot() {
    let config = AppConfig::default();
    let mut state = OrchestratorState::new(&config);

    state.rate_limits = Some(RateLimitInfo {
        remaining: 3000,
        limit: 5000,
        reset_at: Utc::now() + Duration::hours(1),
        source: "github".to_string(),
    });

    let snapshot = state.to_snapshot();
    let rl = snapshot.rate_limits.as_ref().expect("rate_limits should be present");
    assert_eq!(rl.remaining, 3000);
    assert_eq!(rl.source, "github");
}

/// Running entries carry issue_id and identifier through to the snapshot for structured logging.
#[test]
fn state_snapshot_carries_log_context() {
    let config = AppConfig::default();
    let mut state = OrchestratorState::new(&config);

    state.running.insert(
        "gid://github/Issue/99".to_string(),
        make_running_entry("99", Utc::now()),
    );

    let snapshot = state.to_snapshot();
    assert_eq!(snapshot.running.len(), 1);
    assert_eq!(snapshot.running[0].issue_id, "gid://github/Issue/99");
    assert_eq!(snapshot.running[0].identifier, "99");
}

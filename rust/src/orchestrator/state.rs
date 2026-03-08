//! Orchestrator state management

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use chrono::{DateTime, Utc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::AppConfig;
use crate::domain::{Issue, RetryEntry};
use crate::observability::RateLimitInfo;

/// Running entry in the orchestrator
#[derive(Debug)]
pub struct RunningEntry {
    /// Task handle (for cancellation)
    pub task_handle: Option<JoinHandle<()>>,
    /// Cancellation token
    pub cancel_token: CancellationToken,
    /// Issue identifier (for logging)
    pub identifier: String,
    /// Issue being worked on
    pub issue: Issue,
    /// Session ID (if any)
    pub session_id: Option<String>,
    /// Agent PID (if any)
    pub agent_pid: Option<u32>,
    /// Last event type
    pub last_event: Option<String>,
    /// Last event timestamp
    pub last_event_timestamp: Option<DateTime<Utc>>,
    /// Last event message (truncated to 200 chars)
    pub last_event_message: Option<String>,
    /// Input tokens accumulated
    pub input_tokens: u64,
    /// Output tokens accumulated
    pub output_tokens: u64,
    /// Total tokens
    pub total_tokens: u64,
    /// Last reported input tokens (for delta computation)
    pub last_reported_input: u64,
    /// Last reported output tokens
    pub last_reported_output: u64,
    /// Last reported total tokens
    pub last_reported_total: u64,
    /// Turn count
    pub turn_count: u32,
    /// Consecutive failure count (survives dispatch → used for backoff calculation)
    pub consecutive_failures: u32,
    /// When this entry was started
    pub started_at: DateTime<Utc>,
    /// Workspace path for this issue (set after WorkspaceReady message, used for cleanup)
    pub workspace_path: Option<PathBuf>,
}

impl Default for RunningEntry {
    fn default() -> Self {
        Self {
            task_handle: None,
            cancel_token: CancellationToken::new(),
            identifier: String::new(),
            issue: Issue::new("", "", ""),
            session_id: None,
            agent_pid: None,
            last_event: None,
            last_event_timestamp: None,
            last_event_message: None,
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            last_reported_input: 0,
            last_reported_output: 0,
            last_reported_total: 0,
            turn_count: 0,
            consecutive_failures: 0,
            started_at: Utc::now(),
            workspace_path: None,
        }
    }
}

/// Orchestrator runtime state
pub struct OrchestratorState {
    /// Poll interval in milliseconds
    pub poll_interval_ms: u64,
    /// Maximum concurrent agents
    pub max_concurrent_agents: usize,
    /// Currently running entries
    pub running: HashMap<String, RunningEntry>,
    /// Claimed issue IDs (running + retrying)
    pub claimed: HashSet<String>,
    /// Retry queue
    pub retry_attempts: HashMap<String, RetryEntry>,
    /// Count of successfully completed agent runs (monotonically increasing)
    pub completed_count: u64,
    /// Aggregate token totals
    pub agent_totals: crate::domain::TokenTotals,
    /// Rate limit info (if any)
    pub rate_limits: Option<RateLimitInfo>,
    /// Consecutive tracker poll failures (reset on success)
    pub consecutive_tracker_failures: u32,
    /// Instant until which ticks should be skipped (tracker backoff)
    pub skip_ticks_until: Option<tokio::time::Instant>,
}

impl OrchestratorState {
    /// Create new state from config
    pub fn new(config: &AppConfig) -> Self {
        Self {
            poll_interval_ms: config.polling.interval_ms,
            max_concurrent_agents: config.agent.max_concurrent_agents,
            running: HashMap::new(),
            claimed: HashSet::new(),
            retry_attempts: HashMap::new(),
            completed_count: 0,
            agent_totals: crate::domain::TokenTotals::new(),
            rate_limits: None,
            consecutive_tracker_failures: 0,
            skip_ticks_until: None,
        }
    }

    /// Convert to a snapshot for observability
    pub fn to_snapshot(&self) -> crate::observability::RuntimeSnapshot {
        let running: Vec<crate::observability::RunningEntrySnapshot> = self.running.iter()
            .map(|(id, entry)| crate::observability::RunningEntrySnapshot {
                issue_id: id.clone(),
                identifier: entry.identifier.clone(),
                session_id: entry.session_id.clone(),
                turn_count: entry.turn_count,
                input_tokens: entry.input_tokens,
                output_tokens: entry.output_tokens,
                total_tokens: entry.total_tokens,
                last_event: entry.last_event.clone(),
                last_event_message: entry.last_event_message.clone(),
                started_at: entry.started_at,
                seconds_running: (Utc::now() - entry.started_at).num_milliseconds().max(0) as f64 / 1000.0,
            })
            .collect();

        let retrying: Vec<crate::observability::RetryingEntrySnapshot> = self.retry_attempts.iter()
            .map(|(id, entry)| crate::observability::RetryingEntrySnapshot {
                issue_id: id.clone(),
                attempt: entry.attempt,
                error: entry.error.clone(),
            })
            .collect();

        // Include active-session elapsed time so that agent_totals.seconds_running
        // reflects "aggregate runtime as of snapshot time, including active sessions"
        // as required by SPEC §13.1.
        let mut agent_totals = self.agent_totals.clone();
        let active_secs: u64 = self.running.values()
            .map(|e| (Utc::now() - e.started_at).num_milliseconds().max(0) as u64 / 1000)
            .sum();
        agent_totals.add_seconds(active_secs);

        crate::observability::RuntimeSnapshot {
            generated_at: Utc::now(),
            running_count: running.len(),
            retrying_count: retrying.len(),
            completed_count: self.completed_count as usize,
            running,
            retrying,
            agent_totals,
            rate_limits: self.rate_limits.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orchestrator_state_new() {
        let config = AppConfig::default();
        let state = OrchestratorState::new(&config);

        assert_eq!(state.poll_interval_ms, 30000);
        assert_eq!(state.max_concurrent_agents, 10);
        assert!(state.running.is_empty());
        assert!(state.claimed.is_empty());
        assert!(state.retry_attempts.is_empty());
    }

    #[test]
    fn orchestrator_state_to_snapshot() {
        let config = AppConfig::default();
        let state = OrchestratorState::new(&config);

        let snapshot = state.to_snapshot();

        assert_eq!(snapshot.running_count, 0);
        assert_eq!(snapshot.retrying_count, 0);
        assert!(snapshot.running.is_empty());
    }

    #[test]
    fn consecutive_tracker_failures_initialized_to_zero() {
        let config = AppConfig::default();
        let state = OrchestratorState::new(&config);
        assert_eq!(state.consecutive_tracker_failures, 0);
    }

    #[test]
    fn completed_count_is_u64_not_hashset() {
        let config = AppConfig::default();
        let mut state = OrchestratorState::new(&config);

        assert_eq!(state.completed_count, 0);
        state.completed_count += 1;
        assert_eq!(state.completed_count, 1);
        state.completed_count += 1;
        assert_eq!(state.completed_count, 2);

        let snapshot = state.to_snapshot();
        assert_eq!(snapshot.completed_count, 2);
    }
}

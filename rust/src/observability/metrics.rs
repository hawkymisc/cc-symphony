//! Runtime metrics and snapshots

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::TokenTotals;

/// Runtime snapshot for observability
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    /// When this snapshot was generated
    pub generated_at: DateTime<Utc>,
    /// Number of running agents
    pub running_count: usize,
    /// Number of issues in retry queue
    pub retrying_count: usize,
    /// Number of completed issues
    pub completed_count: usize,
    /// Running entries
    pub running: Vec<RunningEntrySnapshot>,
    /// Retry queue entries (for observability and testing)
    pub retrying: Vec<RetryingEntrySnapshot>,
    /// Aggregate token totals
    pub agent_totals: TokenTotals,
    /// Rate limit info
    pub rate_limits: Option<RateLimitInfo>,
}

/// Snapshot of a retrying entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryingEntrySnapshot {
    /// Issue ID
    pub issue_id: String,
    /// Current consecutive failure count (1-indexed)
    pub attempt: u32,
    /// Error that caused the retry
    pub error: Option<String>,
}

/// Snapshot of a running entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningEntrySnapshot {
    /// Issue ID
    pub issue_id: String,
    /// Issue identifier
    pub identifier: String,
    /// Session ID
    pub session_id: Option<String>,
    /// Turn count
    pub turn_count: u32,
    /// Input tokens
    pub input_tokens: u64,
    /// Output tokens
    pub output_tokens: u64,
    /// Total tokens
    pub total_tokens: u64,
    /// Last event type
    pub last_event: Option<String>,
    /// Last event message
    pub last_event_message: Option<String>,
    /// When this entry was started
    pub started_at: DateTime<Utc>,
    /// Elapsed wall-clock seconds since the entry started
    pub seconds_running: f64,
}

impl Default for RuntimeSnapshot {
    fn default() -> Self {
        Self {
            generated_at: Utc::now(),
            running_count: 0,
            retrying_count: 0,
            completed_count: 0,
            running: vec![],
            retrying: vec![],
            agent_totals: TokenTotals::new(),
            rate_limits: None,
        }
    }
}

/// Rate limit information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitInfo {
    /// Remaining requests
    pub remaining: u32,
    /// Total limit
    pub limit: u32,
    /// When the limit resets
    pub reset_at: DateTime<Utc>,
    /// Source of the limit
    pub source: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_snapshot_serialization() {
        let snapshot = RuntimeSnapshot {
            generated_at: Utc::now(),
            running_count: 2,
            retrying_count: 1,
            completed_count: 5,
            retrying: vec![RetryingEntrySnapshot {
                issue_id: "gid://github/Issue/10".to_string(),
                attempt: 2,
                error: Some("mock error".to_string()),
            }],
            running: vec![RunningEntrySnapshot {
                issue_id: "gid://github/Issue/42".to_string(),
                identifier: "42".to_string(),
                session_id: Some("session-1".to_string()),
                turn_count: 3,
                input_tokens: 1000,
                output_tokens: 500,
                total_tokens: 1500,
                last_event: Some("result".to_string()),
                last_event_message: Some("Done".to_string()),
                started_at: Utc::now(),
                seconds_running: 0.0,
            }],
            agent_totals: TokenTotals::new(),
            rate_limits: Some(RateLimitInfo {
                remaining: 4500,
                limit: 5000,
                reset_at: Utc::now() + chrono::Duration::hours(1),
                source: "github".to_string(),
            }),
        };

        let json = serde_json::to_string(&snapshot).unwrap();
        let deserialized: RuntimeSnapshot = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.running_count, 2);
        assert_eq!(deserialized.running.len(), 1);
        assert_eq!(deserialized.running[0].identifier, "42");
    }
}

//! Retry queue entry (SPEC §4.1.7)

use serde::{Deserialize, Serialize};
use tokio::task::JoinHandle;
use std::path::PathBuf;
use std::time::Instant;

/// Entry in the retry queue
#[derive(Debug)]
pub struct RetryEntry {
    /// Current retry attempt number (1-indexed)
    pub attempt: u32,
    /// When this retry should be executed
    pub due_at: Instant,
    /// Handle to the timer task (for cancellation)
    pub timer_handle: JoinHandle<()>,
    /// Issue identifier for logging
    pub identifier: Option<String>,
    /// Error that caused the retry (for logging)
    pub error: Option<String>,
    /// Workspace path for this issue (used for cleanup when issue is abandoned)
    pub workspace_path: Option<PathBuf>,
}

impl RetryEntry {
    /// Check if this retry is due
    pub fn is_due(&self) -> bool {
        Instant::now() >= self.due_at
    }
}

/// Serialized form of RetryEntry for snapshots (without timer handle)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryEntrySnapshot {
    pub attempt: u32,
    pub identifier: Option<String>,
    pub error: Option<String>,
    pub due_in_seconds: u64,
}

impl RetryEntry {
    /// Create a snapshot of this entry (for observability)
    pub fn to_snapshot(&self) -> RetryEntrySnapshot {
        let now = Instant::now();
        let due_in_seconds = if self.due_at > now {
            (self.due_at - now).as_secs()
        } else {
            0
        };

        RetryEntrySnapshot {
            attempt: self.attempt,
            identifier: self.identifier.clone(),
            error: self.error.clone(),
            due_in_seconds,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::runtime::Runtime;

    #[test]
    fn retry_entry_is_due() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let handle = tokio::spawn(async {});

            let past_entry = RetryEntry {
                attempt: 1,
                due_at: Instant::now() - Duration::from_secs(10),
                timer_handle: handle,
                identifier: Some("42".to_string()),
                error: None,
                workspace_path: None,
            };
            assert!(past_entry.is_due());

            let handle2 = tokio::spawn(async {});
            let future_entry = RetryEntry {
                attempt: 1,
                due_at: Instant::now() + Duration::from_secs(10),
                timer_handle: handle2,
                identifier: Some("42".to_string()),
                error: None,
                workspace_path: None,
            };
            assert!(!future_entry.is_due());
        });
    }

    #[test]
    fn retry_entry_snapshot() {
        let rt = Runtime::new().unwrap();
        rt.block_on(async {
            let handle = tokio::spawn(async {});

            let entry = RetryEntry {
                attempt: 3,
                due_at: Instant::now() + Duration::from_secs(30),
                timer_handle: handle,
                identifier: Some("42".to_string()),
                error: Some("timeout".to_string()),
                workspace_path: None,
            };

            let snapshot = entry.to_snapshot();
            assert_eq!(snapshot.attempt, 3);
            assert_eq!(snapshot.identifier, Some("42".to_string()));
            assert_eq!(snapshot.error, Some("timeout".to_string()));
            // Due time should be approximately 30 seconds
            assert!(snapshot.due_in_seconds <= 31);
            assert!(snapshot.due_in_seconds >= 29);
        });
    }
}

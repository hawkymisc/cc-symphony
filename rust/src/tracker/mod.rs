//! Tracker trait and implementations (SPEC §11)

mod memory;

pub use memory::MemoryTracker;

use async_trait::async_trait;
use thiserror::Error;

use crate::domain::Issue;

/// Errors that can occur during tracker operations
#[derive(Debug, Error)]
pub enum TrackerError {
    #[error("API request failed: {0}")]
    ApiRequest(String),

    #[error("API returned error status: {0}")]
    ApiStatus(u16),

    #[error("Rate limited")]
    RateLimited { retry_after_seconds: u64 },

    #[error("GraphQL errors: {0}")]
    GraphqlErrors(String),

    #[error("Missing end cursor in pagination")]
    MissingEndCursor,

    #[error("Unknown payload structure")]
    UnknownPayload,
}

/// Tracker trait for fetching issues from issue tracking systems
#[async_trait]
pub trait Tracker: Send + Sync {
    /// Fetch all candidate issues in active states
    async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError>;

    /// Fetch issues by their IDs (for reconciliation)
    async fn fetch_issues_by_ids(&self, ids: &[String]) -> Result<Vec<Issue>, TrackerError>;

    /// Fetch issues by their states
    async fn fetch_issues_by_states(&self, states: &[String]) -> Result<Vec<Issue>, TrackerError>;
}

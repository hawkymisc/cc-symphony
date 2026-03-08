//! Agent runner trait and Claude Code integration (SPEC §10)

mod claude;

pub use claude::ClaudeRunner;

use async_trait::async_trait;
use thiserror::Error;

use crate::config::AppConfig;
use crate::domain::Issue;

/// Errors that can occur during agent operations
#[derive(Debug, Error)]
pub enum AgentError {
    #[error("Claude CLI not found")]
    ClaudeNotFound,

    #[error("Invalid workspace directory")]
    InvalidWorkspaceCwd,

    #[error("Failed to spawn agent: {0}")]
    SpawnFailed(String),

    #[error("Turn timed out")]
    TurnTimeout,

    #[error("Turn stalled (no activity)")]
    TurnStalled,

    #[error("Agent process exited with code {0}")]
    ProcessExit(i32),

    #[error("Turn failed: {0}")]
    TurnFailed(String),

    #[error("Prompt render error: {0}")]
    PromptRenderError(#[from] crate::prompt::PromptError),
}

/// Update from an agent
#[derive(Debug, Clone)]
pub enum AgentUpdate {
    /// Agent started
    Started { session_id: String },
    /// Agent produced an event
    Event {
        event_type: String,
        message: Option<String>,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
    },
    /// Agent turn completed
    TurnComplete {
        success: bool,
        final_message: Option<String>,
    },
    /// Agent error
    Error { message: String },
}

/// Agent runner trait
#[async_trait]
pub trait AgentRunner: Send + Sync {
    /// Run an agent for an issue
    ///
    /// # Arguments
    /// * `issue` - The issue to work on
    /// * `attempt` - The attempt number (None for first run)
    /// * `config` - The application configuration
    /// * `update_tx` - Channel to send updates to the orchestrator
    /// * `cancel` - Cancellation token
    async fn run(
        &self,
        issue: &Issue,
        attempt: Option<u32>,
        config: &AppConfig,
        update_tx: tokio::sync::mpsc::UnboundedSender<(String, AgentUpdate)>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<(), AgentError>;
}

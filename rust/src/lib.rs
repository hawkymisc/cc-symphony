//! Symphony - Issue tracker to coding agent orchestrator
//!
//! This is the GitHub + Claude Code variant of OpenAI's Symphony.
//! It connects GitHub Issues with Claude Code CLI to automate coding tasks.

pub mod domain;
pub mod workflow;
pub mod config;
pub mod prompt;
pub mod tracker;
pub mod workspace;
pub mod agent;
pub mod orchestrator;
pub mod observability;
pub mod http_server;

pub use domain::Issue;
pub use workflow::{load_workflow, LoadedWorkflow, WorkflowError};
pub use config::{AppConfig, ConfigError};
pub use prompt::{render_prompt, PromptError};

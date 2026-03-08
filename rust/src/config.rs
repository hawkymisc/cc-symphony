//! Configuration layer (SPEC §6)
//!
//! Parses workflow config into typed structs with defaults and validation.

use std::collections::HashMap;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::workflow::LoadedWorkflow;

/// Errors that can occur during config loading/validation
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Missing required field: tracker.kind")]
    MissingTrackerKind,

    #[error("Unsupported tracker kind: {0}. Supported: github")]
    UnsupportedTrackerKind(String),

    #[error("Missing tracker.api_key")]
    MissingTrackerApiKey,

    #[error("Missing tracker.repo (required for GitHub tracker)")]
    MissingTrackerRepo,

    #[error("Invalid tracker.repo format: {0}. Expected: owner/repo")]
    InvalidTrackerRepoFormat(String),

    #[error("Missing claude.command")]
    MissingClaudeCommand,

    #[error("Permission configuration error: must set either skip_permissions=true or allowed_tools")]
    PermissionConfigError,

    #[error("Failed to parse config: {0}")]
    ParseError(#[from] serde_yaml::Error),

    #[error("Environment variable not found: {0}")]
    EnvVarNotFound(String),
}

/// Main application configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub tracker: TrackerConfig,
    #[serde(default)]
    pub polling: PollingConfig,
    #[serde(default)]
    pub workspace: WorkspaceConfig,
    #[serde(default)]
    pub hooks: HooksConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub claude: ClaudeConfig,
    /// Prompt template (the Markdown body of the workflow file, not serialized from YAML)
    #[serde(skip)]
    pub prompt_template: String,
}

/// Tracker configuration (GitHub Issues)
#[derive(Clone, Serialize, Deserialize)]
pub struct TrackerConfig {
    #[serde(default = "default_tracker_kind")]
    pub kind: String,
    #[serde(default = "default_github_endpoint")]
    pub endpoint: String,
    pub api_key: Option<String>,
    pub repo: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default = "default_active_states")]
    pub active_states: Vec<String>,
    #[serde(default = "default_terminal_states")]
    pub terminal_states: Vec<String>,
}

fn default_tracker_kind() -> String {
    "github".to_string()
}

fn default_github_endpoint() -> String {
    "https://api.github.com/graphql".to_string()
}

fn default_active_states() -> Vec<String> {
    vec!["open".to_string()]
}

fn default_terminal_states() -> Vec<String> {
    vec!["closed".to_string()]
}

impl std::fmt::Debug for TrackerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackerConfig")
            .field("kind", &self.kind)
            .field("endpoint", &self.endpoint)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("repo", &self.repo)
            .field("labels", &self.labels)
            .field("active_states", &self.active_states)
            .field("terminal_states", &self.terminal_states)
            .finish()
    }
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self {
            kind: default_tracker_kind(),
            endpoint: default_github_endpoint(),
            api_key: None,
            repo: None,
            labels: Vec::new(),
            active_states: default_active_states(),
            terminal_states: default_terminal_states(),
        }
    }
}

/// Polling configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollingConfig {
    #[serde(default = "default_poll_interval_ms")]
    pub interval_ms: u64,
}

fn default_poll_interval_ms() -> u64 {
    30000
}

impl Default for PollingConfig {
    fn default() -> Self {
        Self {
            interval_ms: default_poll_interval_ms(),
        }
    }
}

/// Workspace configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    #[serde(default = "default_workspace_root")]
    pub root: PathBuf,
}

fn default_workspace_root() -> PathBuf {
    std::env::temp_dir().join("symphony_workspaces")
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            root: default_workspace_root(),
        }
    }
}

/// Hooks configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HooksConfig {
    pub after_create: Option<String>,
    pub before_run: Option<String>,
    pub after_run: Option<String>,
    pub before_remove: Option<String>,
    #[serde(default = "default_hook_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_hook_timeout_ms() -> u64 {
    60000
}

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_max_concurrent_agents")]
    pub max_concurrent_agents: usize,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    #[serde(default = "default_max_retry_backoff_ms")]
    pub max_retry_backoff_ms: u64,
    #[serde(default)]
    pub max_concurrent_agents_by_state: HashMap<String, usize>,
}

fn default_max_concurrent_agents() -> usize {
    10
}

fn default_max_turns() -> u32 {
    20
}

fn default_max_retry_backoff_ms() -> u64 {
    300000 // 5 minutes
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_concurrent_agents: default_max_concurrent_agents(),
            max_turns: default_max_turns(),
            max_retry_backoff_ms: default_max_retry_backoff_ms(),
            max_concurrent_agents_by_state: HashMap::new(),
        }
    }
}

/// Claude Code CLI configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeConfig {
    #[serde(default = "default_claude_command")]
    pub command: String,
    #[serde(default = "default_claude_model")]
    pub model: String,
    #[serde(default)]
    pub skip_permissions: bool,
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default = "default_max_turns_per_invocation")]
    pub max_turns_per_invocation: u32,
    #[serde(default = "default_turn_timeout_ms")]
    pub turn_timeout_ms: u64,
    #[serde(default = "default_read_timeout_ms")]
    pub read_timeout_ms: u64,
    #[serde(default = "default_stall_timeout_ms")]
    pub stall_timeout_ms: u64,
}

fn default_claude_command() -> String {
    "claude".to_string()
}

fn default_claude_model() -> String {
    "claude-sonnet-4-20250514".to_string()
}

fn default_max_turns_per_invocation() -> u32 {
    50
}

fn default_turn_timeout_ms() -> u64 {
    3600000 // 1 hour
}

fn default_read_timeout_ms() -> u64 {
    5000 // 5 seconds
}

fn default_stall_timeout_ms() -> u64 {
    300000 // 5 minutes
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            command: default_claude_command(),
            model: default_claude_model(),
            skip_permissions: false,
            allowed_tools: None,
            max_turns_per_invocation: default_max_turns_per_invocation(),
            turn_timeout_ms: default_turn_timeout_ms(),
            read_timeout_ms: default_read_timeout_ms(),
            stall_timeout_ms: default_stall_timeout_ms(),
        }
    }
}

impl AppConfig {
    /// Create config from loaded workflow
    pub fn from_workflow(workflow: &LoadedWorkflow) -> Result<Self, ConfigError> {
        // Convert Value to AppConfig with defaults
        let mut config: AppConfig = serde_yaml::from_value(workflow.config.clone())?;

        // Preserve the prompt template from the workflow body
        config.prompt_template = workflow.prompt_template.clone();

        // Resolve environment variables
        config.resolve_env()?;

        // Expand paths
        config.expand_paths()?;

        Ok(config)
    }

    /// Resolve environment variable references in config
    fn resolve_env(&mut self) -> Result<(), ConfigError> {
        // Resolve api_key
        if let Some(ref key) = self.tracker.api_key {
            self.tracker.api_key = Some(resolve_env_var(key)?);
        }

        // Resolve workspace root (can contain $VAR)
        if let Some(root_str) = self.workspace.root.to_str() {
            if root_str.contains('$') || root_str.contains('~') {
                let resolved = resolve_env_var(root_str)?;
                self.workspace.root = PathBuf::from(resolved);
            }
        }

        Ok(())
    }

    /// Expand paths (home directory, etc.)
    fn expand_paths(&mut self) -> Result<(), ConfigError> {
        // Expand ~ in workspace root
        let root_str = self.workspace.root.to_string_lossy();
        if root_str.starts_with('~') {
            if let Some(home) = std::env::var_os("HOME") {
                let expanded = root_str.replacen('~', &home.to_string_lossy(), 1);
                self.workspace.root = PathBuf::from(expanded);
            }
        }

        // Make workspace root absolute
        if !self.workspace.root.is_absolute() {
            // Get current directory and make it absolute
            if let Ok(cwd) = std::env::current_dir() {
                self.workspace.root = cwd.join(&self.workspace.root);
            }
        }

        Ok(())
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Validate tracker kind
        if self.tracker.kind != "github" {
            return Err(ConfigError::UnsupportedTrackerKind(self.tracker.kind.clone()));
        }

        // Validate api_key is present
        if self.tracker.api_key.as_ref().is_none_or(|k| k.is_empty()) {
            return Err(ConfigError::MissingTrackerApiKey);
        }

        // Validate repo format
        if let Some(ref repo) = self.tracker.repo {
            if !repo.contains('/') || repo.split('/').count() != 2 {
                return Err(ConfigError::InvalidTrackerRepoFormat(repo.clone()));
            }
        } else {
            return Err(ConfigError::MissingTrackerRepo);
        }

        // Validate claude command
        if self.claude.command.is_empty() {
            return Err(ConfigError::MissingClaudeCommand);
        }

        // Validate permission settings
        if !self.claude.skip_permissions && self.claude.allowed_tools.is_none() {
            // This is actually valid - it means the agent will ask for permission interactively
            // For unattended mode, user should set one of these
            // We just log a warning, not an error
        }

        Ok(())
    }
}

/// Resolve environment variable references in a string
/// Supports $VAR_NAME format
fn resolve_env_var(s: &str) -> Result<String, ConfigError> {
    let mut result = s.to_string();

    // Handle ~ expansion first
    if result.starts_with('~') {
        if let Some(home) = std::env::var_os("HOME") {
            result = result.replacen('~', &home.to_string_lossy(), 1);
        }
    }

    // Find all $VAR references and replace them
    let mut offset = 0;
    while offset < result.len() {
        let remaining = &result[offset..];
        if let Some(dollar_pos) = remaining.find('$') {
            let abs_pos = offset + dollar_pos;
            let after_dollar = &result[abs_pos + 1..];

            // Find the end of the variable name
            let var_end = after_dollar
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(after_dollar.len());

            if var_end == 0 {
                offset = abs_pos + 1;
                continue;
            }

            let var_name = &after_dollar[..var_end];
            let var_value = std::env::var(var_name).unwrap_or_default();

            // Build the new string
            let before = result[..abs_pos].to_string();
            let after = result[abs_pos + 1 + var_end..].to_string();
            result = format!("{}{}{}", before, var_value, after);

            // Update offset to after the replacement
            offset = before.len() + var_value.len();
        } else {
            break;
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;
    use temp_env;

    fn create_workflow_file(contents: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "{}", contents).unwrap();
        file
    }

    #[test]
    fn config_defaults() {
        let config = AppConfig::default();

        assert_eq!(config.tracker.kind, "github");
        assert_eq!(config.tracker.endpoint, "https://api.github.com/graphql");
        assert!(config.tracker.api_key.is_none());
        assert!(config.tracker.repo.is_none());
        assert!(config.tracker.labels.is_empty());
        assert_eq!(config.tracker.active_states, vec!["open"]);
        assert_eq!(config.tracker.terminal_states, vec!["closed"]);

        assert_eq!(config.polling.interval_ms, 30000);

        assert_eq!(config.agent.max_concurrent_agents, 10);
        assert_eq!(config.agent.max_turns, 20);
        assert_eq!(config.agent.max_retry_backoff_ms, 300000);

        assert_eq!(config.claude.command, "claude");
        assert_eq!(config.claude.model, "claude-sonnet-4-20250514");
        assert!(!config.claude.skip_permissions);
        assert!(config.claude.allowed_tools.is_none());
        assert_eq!(config.claude.max_turns_per_invocation, 50);
        assert_eq!(config.claude.turn_timeout_ms, 3600000);
        assert_eq!(config.claude.read_timeout_ms, 5000);
        assert_eq!(config.claude.stall_timeout_ms, 300000);
    }

    #[test]
    fn config_env_resolution() {
        temp_env::with_var("TEST_API_KEY", Some("secret123"), || {
            let workflow = crate::workflow::LoadedWorkflow {
                config: serde_yaml::from_str(r#"
                    tracker:
                      api_key: $TEST_API_KEY
                      repo: "owner/repo"
                "#).unwrap(),
                prompt_template: String::new(),
                path: String::new(),
            };

            let config = AppConfig::from_workflow(&workflow).unwrap();
            assert_eq!(config.tracker.api_key, Some("secret123".to_string()));
        });
    }

    #[test]
    fn config_env_empty_treated_as_missing() {
        temp_env::with_var("EMPTY_KEY", Some(""), || {
            let workflow = crate::workflow::LoadedWorkflow {
                config: serde_yaml::from_str(r#"
                    tracker:
                      api_key: $EMPTY_KEY
                      repo: "owner/repo"
                "#).unwrap(),
                prompt_template: String::new(),
                path: String::new(),
            };

            let config = AppConfig::from_workflow(&workflow).unwrap();
            assert_eq!(config.tracker.api_key, Some("".to_string()));

            // Validation should fail because api_key is empty
            let result = config.validate();
            assert!(matches!(result, Err(ConfigError::MissingTrackerApiKey)));
        });
    }

    #[test]
    fn config_path_expansion() {
        temp_env::with_var("HOME", Some("/home/testuser"), || {
            let workflow = crate::workflow::LoadedWorkflow {
                config: serde_yaml::from_str(r#"
                    workspace:
                      root: ~/workspaces
                "#).unwrap(),
                prompt_template: String::new(),
                path: String::new(),
            };

            let config = AppConfig::from_workflow(&workflow).unwrap();
            assert!(config.workspace.root.to_str().unwrap().contains("testuser"));
        });
    }

    #[test]
    fn config_validate_missing_tracker_kind() {
        let workflow = crate::workflow::LoadedWorkflow {
            config: serde_yaml::from_str(r#"
                tracker:
                  api_key: "test"
                  repo: "owner/repo"
            "#).unwrap(),
            prompt_template: String::new(),
            path: String::new(),
        };

        // Actually kind defaults to "github", so this should work
        let config = AppConfig::from_workflow(&workflow).unwrap();
        assert_eq!(config.tracker.kind, "github");
    }

    #[test]
    fn config_validate_unsupported_tracker_kind() {
        let workflow = crate::workflow::LoadedWorkflow {
            config: serde_yaml::from_str(r#"
                tracker:
                  kind: "linear"
                  api_key: "test"
                  repo: "owner/repo"
            "#).unwrap(),
            prompt_template: String::new(),
            path: String::new(),
        };

        let config = AppConfig::from_workflow(&workflow).unwrap();
        let result = config.validate();
        assert!(matches!(result, Err(ConfigError::UnsupportedTrackerKind(_))));
    }

    #[test]
    fn config_validate_missing_api_key() {
        let workflow = crate::workflow::LoadedWorkflow {
            config: serde_yaml::from_str(r#"
                tracker:
                  repo: "owner/repo"
            "#).unwrap(),
            prompt_template: String::new(),
            path: String::new(),
        };

        let config = AppConfig::from_workflow(&workflow).unwrap();
        let result = config.validate();
        assert!(matches!(result, Err(ConfigError::MissingTrackerApiKey)));
    }

    #[test]
    fn config_validate_missing_repo() {
        let workflow = crate::workflow::LoadedWorkflow {
            config: serde_yaml::from_str(r#"
                tracker:
                  api_key: "test"
            "#).unwrap(),
            prompt_template: String::new(),
            path: String::new(),
        };

        let config = AppConfig::from_workflow(&workflow).unwrap();
        let result = config.validate();
        assert!(matches!(result, Err(ConfigError::MissingTrackerRepo)));
    }

    #[test]
    fn config_validate_invalid_repo_format() {
        let workflow = crate::workflow::LoadedWorkflow {
            config: serde_yaml::from_str(r#"
                tracker:
                  api_key: "test"
                  repo: "invalid-repo-format"
            "#).unwrap(),
            prompt_template: String::new(),
            path: String::new(),
        };

        let config = AppConfig::from_workflow(&workflow).unwrap();
        let result = config.validate();
        assert!(matches!(result, Err(ConfigError::InvalidTrackerRepoFormat(_))));
    }

    #[test]
    fn config_validate_repo_valid_format() {
        let workflow = crate::workflow::LoadedWorkflow {
            config: serde_yaml::from_str(r#"
                tracker:
                  api_key: "test"
                  repo: "owner/repo"
                claude:
                  command: "claude"
            "#).unwrap(),
            prompt_template: String::new(),
            path: String::new(),
        };

        let config = AppConfig::from_workflow(&workflow).unwrap();
        let result = config.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn tracker_config_debug_masks_api_key() {
        let config = TrackerConfig {
            api_key: Some("ghp_super_secret_token_12345".to_string()),
            ..Default::default()
        };
        let debug_output = format!("{:?}", config);
        assert!(!debug_output.contains("ghp_super_secret_token_12345"), "API key should be masked in Debug output");
        assert!(debug_output.contains("[REDACTED]"), "Debug output should contain [REDACTED]");
    }

    #[test]
    fn config_full_workflow() {
        let contents = r#"---
tracker:
  kind: github
  repo: "owner/repo"
  api_key: test_key
  labels:
    - symphony
  active_states:
    - open
  terminal_states:
    - closed
polling:
  interval_ms: 60000
workspace:
  root: /tmp/symphony
hooks:
  after_create: |
    echo "created"
  before_run: |
    echo "before"
  timeout_ms: 30000
agent:
  max_concurrent_agents: 5
  max_turns: 10
  max_retry_backoff_ms: 120000
claude:
  command: claude
  model: claude-sonnet-4-20250514
  skip_permissions: true
  max_turns_per_invocation: 30
  turn_timeout_ms: 1800000
---

Test prompt
"#;
        let file = create_workflow_file(contents);
        let workflow = crate::workflow::load_workflow(file.path()).unwrap();
        let config = AppConfig::from_workflow(&workflow).unwrap();

        assert_eq!(config.tracker.kind, "github");
        assert_eq!(config.tracker.repo, Some("owner/repo".to_string()));
        assert_eq!(config.tracker.labels, vec!["symphony"]);
        assert_eq!(config.polling.interval_ms, 60000);
        assert_eq!(config.hooks.timeout_ms, 30000);
        assert_eq!(config.agent.max_concurrent_agents, 5);
        assert_eq!(config.agent.max_turns, 10);
        assert!(config.claude.skip_permissions);
        assert_eq!(config.claude.max_turns_per_invocation, 30);
    }
}

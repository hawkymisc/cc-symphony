//! Workspace manager (SPEC §9)
//!
//! Manages per-issue workspace directories and lifecycle hooks.

mod hooks;

pub use hooks::{run_hook, HookError, HookType};

use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::warn;

use crate::config::{HooksConfig, WorkspaceConfig};
use crate::domain::Issue;

/// Errors that can occur during workspace operations
#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("Path escapes workspace root")]
    OutsideRoot,

    #[error("Path equals root")]
    EqualsRoot,

    #[error("Symlink escapes workspace")]
    SymlinkEscape,

    #[error("Hook failed: {0}")]
    HookFailed(#[from] HookError),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Result of workspace preparation
#[derive(Debug)]
pub struct PreparedWorkspace {
    /// Absolute path to the workspace directory
    pub path: PathBuf,
    /// Whether the workspace was newly created
    pub created_now: bool,
}

/// Prepare a workspace for an issue.
///
/// Creates the workspace directory if it doesn't exist.
/// Runs the `after_create` hook only on newly created workspaces.
/// If `after_create` fails, removes the directory and returns an error.
pub async fn prepare_workspace(
    config: &WorkspaceConfig,
    hooks: &HooksConfig,
    issue: &Issue,
) -> Result<PreparedWorkspace, WorkspaceError> {
    let workspace_key = issue.sanitized_identifier();
    let workspace_path = config.root.join(&workspace_key);

    // Ensure root directory exists
    if !config.root.exists() {
        std::fs::create_dir_all(&config.root)?;
    }

    // Validate path containment
    validate_path_containment(&config.root, &workspace_path)?;

    // Check if directory already exists
    let created_now = if workspace_path.exists() {
        false
    } else {
        std::fs::create_dir_all(&workspace_path)?;
        true
    };

    // Run after_create hook only on new workspaces (fatal on failure)
    if created_now {
        if let Some(ref script) = hooks.after_create {
            if let Err(e) = run_hook(HookType::AfterCreate, script, &workspace_path, hooks.timeout_ms).await {
                // Fatal: remove the newly-created directory and propagate error
                let _ = std::fs::remove_dir_all(&workspace_path);
                return Err(WorkspaceError::HookFailed(e));
            }
        }
    }

    Ok(PreparedWorkspace {
        path: workspace_path,
        created_now,
    })
}

/// Run the before_run hook. Fatal on failure (aborts the current attempt).
pub async fn run_before_run_hook(path: &Path, hooks: &HooksConfig) -> Result<(), WorkspaceError> {
    if let Some(ref script) = hooks.before_run {
        run_hook(HookType::BeforeRun, script, path, hooks.timeout_ms).await?;
    }
    Ok(())
}

/// Run the after_run hook. Non-fatal: logs warning but does not fail.
pub async fn run_after_run_hook(path: &Path, hooks: &HooksConfig) {
    if let Some(ref script) = hooks.after_run {
        if let Err(e) = run_hook(HookType::AfterRun, script, path, hooks.timeout_ms).await {
            warn!("after_run hook failed (non-fatal): {}", e);
        }
    }
}

/// Clean up a workspace directory. Calls before_remove hook (non-fatal) first.
pub async fn cleanup_workspace(path: &Path, hooks: &HooksConfig) -> Result<(), WorkspaceError> {
    if path.exists() {
        if let Some(ref script) = hooks.before_remove {
            if let Err(e) = run_hook(HookType::BeforeRemove, script, path, hooks.timeout_ms).await {
                warn!("before_remove hook failed (non-fatal): {}", e);
            }
        }
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}

/// Validate that a path is contained within the root
pub fn validate_path_containment(root: &Path, path: &Path) -> Result<(), WorkspaceError> {
    // Canonicalize both paths to resolve symlinks and relative components
    let canonical_root = root.canonicalize().map_err(|_| {
        if !root.exists() {
            std::io::Error::new(std::io::ErrorKind::NotFound, "Root does not exist")
        } else {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Cannot canonicalize root")
        }
    })?;

    // For the target path, we can't canonicalize if it doesn't exist yet
    // So we use canonicalize on the parent if the path doesn't exist
    let canonical_path = if path.exists() {
        path.canonicalize()?
    } else {
        // Get parent directory and join the final component
        let parent = path.parent().ok_or(WorkspaceError::EqualsRoot)?;
        let file_name = path.file_name().ok_or(WorkspaceError::EqualsRoot)?;

        if parent.exists() {
            parent.canonicalize()?.join(file_name)
        } else {
            // Parent doesn't exist, this will be created
            path.to_path_buf()
        }
    };

    // Check for symlink escape
    if canonical_path.is_symlink() {
        let target = std::fs::read_link(&canonical_path)?;
        if !target.starts_with(&canonical_root) {
            return Err(WorkspaceError::SymlinkEscape);
        }
    }

    // Check containment
    if !canonical_path.starts_with(&canonical_root) {
        return Err(WorkspaceError::OutsideRoot);
    }

    // Check for root equality
    if canonical_path == canonical_root {
        return Err(WorkspaceError::EqualsRoot);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HooksConfig;
    use tempfile::TempDir;

    fn create_test_config(temp_dir: &TempDir) -> WorkspaceConfig {
        WorkspaceConfig {
            root: temp_dir.path().to_path_buf(),
        }
    }

    fn no_hooks() -> HooksConfig {
        HooksConfig::default()
    }

    #[tokio::test]
    async fn workspace_create_new() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        let issue = Issue::new("1", "42", "Test");

        let result = prepare_workspace(&config, &no_hooks(), &issue).await.unwrap();

        assert!(result.path.exists());
        assert!(result.created_now);
        assert!(result.path.to_str().unwrap().ends_with("42"));
    }

    #[tokio::test]
    async fn workspace_reuse_existing() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        let issue = Issue::new("1", "42", "Test");

        let result1 = prepare_workspace(&config, &no_hooks(), &issue).await.unwrap();
        assert!(result1.created_now);

        let result2 = prepare_workspace(&config, &no_hooks(), &issue).await.unwrap();
        assert!(!result2.created_now);
        assert_eq!(result1.path, result2.path);
    }

    #[tokio::test]
    async fn workspace_path_deterministic() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        let issue = Issue::new("1", "42", "Test");

        let result1 = prepare_workspace(&config, &no_hooks(), &issue).await.unwrap();
        let result2 = prepare_workspace(&config, &no_hooks(), &issue).await.unwrap();

        assert_eq!(result1.path, result2.path);
    }

    #[tokio::test]
    async fn workspace_path_sanitized() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);

        let issue = Issue::new("1", "foo/bar", "Test");
        let result = prepare_workspace(&config, &no_hooks(), &issue).await.unwrap();

        assert!(result.path.to_str().unwrap().contains("foo_bar"));
    }

    #[tokio::test]
    async fn workspace_cleanup() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        let issue = Issue::new("1", "42", "Test");

        let result = prepare_workspace(&config, &no_hooks(), &issue).await.unwrap();
        assert!(result.path.exists());

        cleanup_workspace(&result.path, &no_hooks()).await.unwrap();
        assert!(!result.path.exists());
    }

    // --- Hook integration tests ---

    #[tokio::test]
    async fn hook_after_create_runs_on_new_workspace() {
        let temp_dir = TempDir::new().unwrap();
        let flag_file = temp_dir.path().join("hook_ran");
        let config = WorkspaceConfig {
            root: temp_dir.path().join("workspaces"),
        };
        let hooks = HooksConfig {
            after_create: Some(format!("touch {}", flag_file.display())),
            timeout_ms: 5000,
            ..Default::default()
        };
        let issue = Issue::new("1", "42", "Test");

        prepare_workspace(&config, &hooks, &issue).await.unwrap();

        assert!(flag_file.exists(), "after_create hook should have run");
    }

    #[tokio::test]
    async fn hook_after_create_not_run_on_reuse() {
        let temp_dir = TempDir::new().unwrap();
        let counter_file = temp_dir.path().join("counter");
        let config = WorkspaceConfig {
            root: temp_dir.path().join("workspaces"),
        };
        let hooks = HooksConfig {
            after_create: Some(format!(
                "if [ -f {f} ]; then echo two > {f}; else echo one > {f}; fi",
                f = counter_file.display()
            )),
            timeout_ms: 5000,
            ..Default::default()
        };
        let issue = Issue::new("1", "42", "Test");

        // First call: new workspace, hook should run
        prepare_workspace(&config, &hooks, &issue).await.unwrap();
        assert_eq!(std::fs::read_to_string(&counter_file).unwrap().trim(), "one");

        // Second call: reuse, hook should NOT run again
        prepare_workspace(&config, &hooks, &issue).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&counter_file).unwrap().trim(),
            "one",
            "after_create should not run on workspace reuse"
        );
    }

    #[tokio::test]
    async fn hook_after_create_failure_removes_dir() {
        let temp_dir = TempDir::new().unwrap();
        let config = WorkspaceConfig {
            root: temp_dir.path().join("workspaces"),
        };
        let hooks = HooksConfig {
            after_create: Some("exit 1".to_string()),
            timeout_ms: 5000,
            ..Default::default()
        };
        let issue = Issue::new("1", "42", "Test");

        let result = prepare_workspace(&config, &hooks, &issue).await;

        assert!(result.is_err(), "should fail when after_create hook fails");
        // Workspace directory should have been removed
        let workspace_path = config.root.join("42");
        assert!(
            !workspace_path.exists(),
            "workspace dir should be removed after failed after_create"
        );
    }

    #[tokio::test]
    async fn hook_before_run_runs() {
        let temp_dir = TempDir::new().unwrap();
        let flag_file = temp_dir.path().join("before_run_ran");
        let hooks = HooksConfig {
            before_run: Some(format!("touch {}", flag_file.display())),
            timeout_ms: 5000,
            ..Default::default()
        };

        run_before_run_hook(temp_dir.path(), &hooks).await.unwrap();

        assert!(flag_file.exists(), "before_run hook should have run");
    }

    #[tokio::test]
    async fn hook_before_run_failure_aborts() {
        let temp_dir = TempDir::new().unwrap();
        let hooks = HooksConfig {
            before_run: Some("exit 42".to_string()),
            timeout_ms: 5000,
            ..Default::default()
        };

        let result = run_before_run_hook(temp_dir.path(), &hooks).await;

        assert!(result.is_err(), "before_run failure should be fatal");
    }

    #[tokio::test]
    async fn hook_after_run_failure_is_not_fatal() {
        let temp_dir = TempDir::new().unwrap();
        let hooks = HooksConfig {
            after_run: Some("exit 1".to_string()),
            timeout_ms: 5000,
            ..Default::default()
        };

        // Should not panic or propagate an error
        run_after_run_hook(temp_dir.path(), &hooks).await;
    }

    #[tokio::test]
    async fn hook_before_remove_called_on_cleanup() {
        let temp_dir = TempDir::new().unwrap();
        let flag_file = temp_dir.path().join("before_remove_ran");
        let workspace_dir = temp_dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_dir).unwrap();

        let hooks = HooksConfig {
            before_remove: Some(format!("touch {}", flag_file.display())),
            timeout_ms: 5000,
            ..Default::default()
        };

        cleanup_workspace(&workspace_dir, &hooks).await.unwrap();

        assert!(flag_file.exists(), "before_remove hook should have run");
        assert!(!workspace_dir.exists(), "workspace dir should be removed");
    }

    #[tokio::test]
    async fn hook_before_remove_failure_is_not_fatal() {
        let temp_dir = TempDir::new().unwrap();
        let workspace_dir = temp_dir.path().join("workspace");
        std::fs::create_dir_all(&workspace_dir).unwrap();

        let hooks = HooksConfig {
            before_remove: Some("exit 1".to_string()),
            timeout_ms: 5000,
            ..Default::default()
        };

        // Should still remove the directory even if before_remove fails
        let result = cleanup_workspace(&workspace_dir, &hooks).await;
        assert!(result.is_ok());
        assert!(!workspace_dir.exists());
    }
}

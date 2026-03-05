//! Workspace manager (SPEC §9)
//!
//! Manages per-issue workspace directories and lifecycle hooks.

mod hooks;

pub use hooks::{run_hook, HookError, HookType};

use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::config::WorkspaceConfig;
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

/// Prepare a workspace for an issue
///
/// Creates the workspace directory if it doesn't exist.
/// Returns the path and whether it was newly created.
pub fn prepare_workspace(
    config: &WorkspaceConfig,
    issue: &Issue,
) -> Result<PreparedWorkspace, WorkspaceError> {
    let workspace_key = issue.sanitized_identifier();
    let workspace_path = config.root.join(&workspace_key);

    // Validate path containment
    validate_path_containment(&config.root, &workspace_path)?;

    // Check if directory already exists
    let created_now = if workspace_path.exists() {
        false
    } else {
        std::fs::create_dir_all(&workspace_path)?;
        true
    };

    Ok(PreparedWorkspace {
        path: workspace_path,
        created_now,
    })
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

/// Clean up a workspace directory
pub fn cleanup_workspace(path: &Path) -> Result<(), WorkspaceError> {
    if path.exists() {
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_config(temp_dir: &TempDir) -> WorkspaceConfig {
        WorkspaceConfig {
            root: temp_dir.path().to_path_buf(),
        }
    }

    #[test]
    fn workspace_create_new() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        let issue = Issue::new("1", "42", "Test");

        let result = prepare_workspace(&config, &issue).unwrap();

        assert!(result.path.exists());
        assert!(result.created_now);
        assert!(result.path.to_str().unwrap().ends_with("42"));
    }

    #[test]
    fn workspace_reuse_existing() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        let issue = Issue::new("1", "42", "Test");

        // First call creates the workspace
        let result1 = prepare_workspace(&config, &issue).unwrap();
        assert!(result1.created_now);

        // Second call reuses it
        let result2 = prepare_workspace(&config, &issue).unwrap();
        assert!(!result2.created_now);
        assert_eq!(result1.path, result2.path);
    }

    #[test]
    fn workspace_path_deterministic() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        let issue = Issue::new("1", "42", "Test");

        let result1 = prepare_workspace(&config, &issue).unwrap();
        let result2 = prepare_workspace(&config, &issue).unwrap();

        assert_eq!(result1.path, result2.path);
    }

    #[test]
    fn workspace_path_sanitized() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);

        let issue = Issue::new("1", "foo/bar", "Test");
        let result = prepare_workspace(&config, &issue).unwrap();

        assert!(result.path.to_str().unwrap().contains("foo_bar"));
    }

    #[test]
    fn workspace_cleanup() {
        let temp_dir = TempDir::new().unwrap();
        let config = create_test_config(&temp_dir);
        let issue = Issue::new("1", "42", "Test");

        let result = prepare_workspace(&config, &issue).unwrap();
        assert!(result.path.exists());

        cleanup_workspace(&result.path).unwrap();
        assert!(!result.path.exists());
    }
}

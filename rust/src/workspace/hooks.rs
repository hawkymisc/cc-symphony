//! Hook execution with timeout

use std::path::Path;
use std::time::Duration;
use thiserror::Error;
use tokio::process::Command;
use tokio::time::timeout;

/// Types of workspace hooks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookType {
    AfterCreate,
    BeforeRun,
    AfterRun,
    BeforeRemove,
}

/// Errors that can occur during hook execution
#[derive(Debug, Error)]
pub enum HookError {
    #[error("Hook '{hook:?}' failed with exit code {code}")]
    Failed { hook: HookType, code: i32 },

    #[error("Hook '{hook:?}' timed out after {timeout_ms}ms")]
    Timeout { hook: HookType, timeout_ms: u64 },

    #[error("Hook '{hook:?}' could not start: {message}")]
    CouldNotStart { hook: HookType, message: String },
}

/// Run a hook script
///
/// # Arguments
/// * `hook_type` - The type of hook being run
/// * `script` - The shell script to execute
/// * `workspace_path` - The path to the workspace directory
/// * `timeout_ms` - Timeout in milliseconds
///
/// # Returns
/// Ok(()) if the hook succeeded, Err if it failed or timed out
pub async fn run_hook(
    hook_type: HookType,
    script: &str,
    workspace_path: &Path,
    timeout_ms: u64,
) -> Result<(), HookError> {
    if script.trim().is_empty() {
        return Ok(());
    }

    let timeout_duration = Duration::from_millis(timeout_ms);

    let result = timeout(timeout_duration, async {
        let mut child = Command::new("sh")
            .arg("-lc")
            .arg(script)
            .current_dir(workspace_path)
            .spawn()
            .map_err(|e| HookError::CouldNotStart {
                hook: hook_type,
                message: e.to_string(),
            })?;

        let status = child.wait().await.map_err(|e| HookError::CouldNotStart {
            hook: hook_type,
            message: e.to_string(),
        })?;

        if status.success() {
            Ok(())
        } else {
            Err(HookError::Failed {
                hook: hook_type,
                code: status.code().unwrap_or(-1),
            })
        }
    })
    .await;

    match result {
        Ok(inner) => inner,
        Err(_) => Err(HookError::Timeout {
            hook: hook_type,
            timeout_ms,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn hook_success() {
        let temp_dir = TempDir::new().unwrap();
        let script = "echo 'hello'";

        let result = run_hook(HookType::BeforeRun, script, temp_dir.path(), 5000).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn hook_failure() {
        let temp_dir = TempDir::new().unwrap();
        let script = "exit 1";

        let result = run_hook(HookType::BeforeRun, script, temp_dir.path(), 5000).await;
        assert!(matches!(result, Err(HookError::Failed { hook: HookType::BeforeRun, code: 1 })));
    }

    #[tokio::test]
    async fn hook_timeout() {
        let temp_dir = TempDir::new().unwrap();
        let script = "sleep 10";

        let result = run_hook(HookType::BeforeRun, script, temp_dir.path(), 100).await;
        assert!(matches!(result, Err(HookError::Timeout { .. })));
    }

    #[tokio::test]
    async fn hook_empty_script() {
        let temp_dir = TempDir::new().unwrap();

        let result = run_hook(HookType::AfterCreate, "", temp_dir.path(), 5000).await;
        assert!(result.is_ok());

        let result = run_hook(HookType::AfterCreate, "   ", temp_dir.path(), 5000).await;
        assert!(result.is_ok());
    }
}

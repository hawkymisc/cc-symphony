//! Claude Code CLI integration

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use serde::Deserialize;

use crate::domain::Issue;
use super::{AgentRunner, AgentError, AgentUpdate, AgentRunConfig};

/// Claude Code CLI runner
pub struct ClaudeRunner;

/// Stream event from Claude Code CLI
#[derive(Debug, Deserialize)]
struct ClaudeStreamEvent {
    #[serde(rename = "type")]
    event_type: String,
    message: Option<serde_json::Value>,
    result: Option<String>,
    usage: Option<ClaudeUsage>,
    error: Option<serde_json::Value>,
    tool: Option<String>,
    #[allow(dead_code)]
    input: Option<serde_json::Value>,
    output: Option<String>,
}

/// Token usage from Claude
#[derive(Debug, Deserialize)]
struct ClaudeUsage {
    input_tokens: u64,
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

#[async_trait]
impl AgentRunner for ClaudeRunner {
    async fn run(
        &self,
        issue: &Issue,
        attempt: Option<u32>,
        config: &AgentRunConfig,
        update_tx: tokio::sync::mpsc::UnboundedSender<(String, AgentUpdate)>,
        cancel: CancellationToken,
    ) -> Result<(), AgentError> {
        let workspace_path = config.workspace_root.join(issue.sanitized_identifier());

        // Prepare workspace
        if !workspace_path.exists() {
            std::fs::create_dir_all(&workspace_path)
                .map_err(|e| AgentError::SpawnFailed(e.to_string()))?;
        }

        // Render prompt from the template stored in config (parsed from WORKFLOW.md at startup)
        let repo = config.repo.as_str();
        let template = if config.prompt_template.is_empty() {
            crate::prompt::DEFAULT_PROMPT_TEMPLATE.to_string()
        } else {
            config.prompt_template.clone()
        };
        let prompt = crate::prompt::render_prompt(&template, issue, attempt, repo)?;

        // Track tokens
        let mut last_reported_input: u64 = 0;
        let mut last_reported_output: u64 = 0;

        // Build command
        let mut cmd = Command::new(&config.claude.command);
        cmd.arg("--print")
            .arg("--output-format").arg("stream-json")
            .arg("--model").arg(&config.claude.model)
            .arg("--max-turns").arg(config.claude.max_turns_per_invocation.to_string())
            .arg("-p").arg(&prompt)
            .current_dir(&workspace_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        if config.claude.skip_permissions {
            cmd.arg("--dangerously-skip-permissions");
        }

        if let Some(ref tools) = config.claude.allowed_tools {
            cmd.arg("--allowedTools").arg(tools.join(","));
        }

        // Spawn process
        let mut child = cmd.spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    AgentError::ClaudeNotFound
                } else {
                    AgentError::SpawnFailed(e.to_string())
                }
            })?;

        let session_id = format!("{}-1", issue.id);
        let _ = update_tx.send((issue.id.clone(), AgentUpdate::Started {
            session_id: session_id.clone(),
        }));

        // Read stdout
        let stdout = child.stdout.take().ok_or_else(|| {
            AgentError::SpawnFailed("Failed to capture stdout".to_string())
        })?;

        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();

        let read_timeout = Duration::from_millis(config.claude.read_timeout_ms);
        let turn_timeout = Duration::from_millis(config.claude.turn_timeout_ms);

        let start_time = std::time::Instant::now();

        loop {
            // Check cancellation
            if cancel.is_cancelled() {
                let _ = child.kill().await;
                return Ok(());
            }

            // Check overall timeout
            if start_time.elapsed() > turn_timeout {
                let _ = child.kill().await;
                return Err(AgentError::TurnTimeout);
            }

            // Read next line with timeout
            let line_result = timeout(read_timeout, lines.next_line()).await;

            match line_result {
                Ok(Ok(Some(line))) => {
                    // Parse JSON event
                    if let Ok(event) = serde_json::from_str::<ClaudeStreamEvent>(&line) {
                        match event.event_type.as_str() {
                            "result" => {
                                // Extract usage
                                if let Some(usage) = event.usage {
                                    // Compute delta from last reported values
                                    let input_delta = usage.input_tokens.saturating_sub(last_reported_input);
                                    let output_delta = usage.output_tokens.saturating_sub(last_reported_output);

                                    last_reported_input = usage.input_tokens;
                                    last_reported_output = usage.output_tokens;

                                    let _ = update_tx.send((issue.id.clone(), AgentUpdate::Event {
                                        event_type: "result".to_string(),
                                        message: event.result.clone(),
                                        input_tokens: input_delta,
                                        output_tokens: output_delta,
                                        cache_creation_tokens: usage.cache_creation_input_tokens,
                                        cache_read_tokens: usage.cache_read_input_tokens,
                                    }));
                                }

                                let _ = update_tx.send((issue.id.clone(), AgentUpdate::TurnComplete {
                                    success: true,
                                    final_message: event.result,
                                }));
                            }
                            "error" => {
                                let error_msg = event.error
                                    .and_then(|e| e.get("message").and_then(|m| m.as_str().map(|s| s.to_string())))
                                    .unwrap_or_else(|| "Unknown error".to_string());

                                let _ = update_tx.send((issue.id.clone(), AgentUpdate::Error {
                                    message: error_msg.clone(),
                                }));

                                return Err(AgentError::TurnFailed(error_msg));
                            }
                            "assistant" | "tool_use" | "tool_result" => {
                                let msg = if event.event_type == "tool_use" {
                                    event.tool.clone()
                                } else if event.event_type == "tool_result" {
                                    event.output.clone()
                                } else {
                                    event.message.as_ref().and_then(|m| {
                                        m.get("content").and_then(|c| c.as_str().map(|s| {
                                            if s.len() > 200 {
                                                format!("{}...", &s[..200])
                                            } else {
                                                s.to_string()
                                            }
                                        }))
                                    })
                                };

                                let _ = update_tx.send((issue.id.clone(), AgentUpdate::Event {
                                    event_type: event.event_type.clone(),
                                    message: msg,
                                    input_tokens: 0,
                                    output_tokens: 0,
                                    cache_creation_tokens: 0,
                                    cache_read_tokens: 0,
                                }));
                            }
                            _ => {
                                // Unknown event type, ignore
                            }
                        }
                    }
                    // Malformed JSON is ignored (logged elsewhere)
                }
                Ok(Ok(None)) => {
                    // EOF reached
                    break;
                }
                Ok(Err(e)) => {
                    // Read error
                    tracing::warn!("Read error from Claude CLI: {}", e);
                    break;
                }
                Err(_) => {
                    // Read timeout - check if process is still running
                    // Continue loop to check cancellation and overall timeout
                }
            }
        }

        // Wait for process to complete
        let exit_status = child.wait().await.map_err(|e| {
            AgentError::SpawnFailed(e.to_string())
        })?;

        if !exit_status.success() {
            return Err(AgentError::ProcessExit(
                exit_status.code().unwrap_or(-1)
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_usage_deserialize() {
        let json = r#"{"input_tokens": 100, "output_tokens": 50}"#;
        let usage: ClaudeUsage = serde_json::from_str(json).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
    }

    #[test]
    fn claude_usage_with_cache() {
        let json = r#"{
            "input_tokens": 100,
            "output_tokens": 50,
            "cache_creation_input_tokens": 10,
            "cache_read_input_tokens": 20
        }"#;
        let usage: ClaudeUsage = serde_json::from_str(json).unwrap();
        assert_eq!(usage.cache_creation_input_tokens, 10);
        assert_eq!(usage.cache_read_input_tokens, 20);
    }
}

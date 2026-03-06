//! Phase 5: ClaudeRunner integration tests using mock `claude` scripts
//!
//! Each test configures `config.claude.command` to point to a shell script
//! in fixtures/claude_mocks/ that emits canned stream-json output.

use std::path::PathBuf;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use symphony::agent::{AgentRunner, AgentUpdate, ClaudeRunner};
use symphony::config::AppConfig;
use symphony::domain::Issue;

// ─── helpers ─────────────────────────────────────────────────────────────────

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/claude_mocks")
}

fn make_config(mock_script: &str) -> AppConfig {
    let mut config = AppConfig::default();
    config.claude.command = fixtures_dir().join(mock_script).to_string_lossy().to_string();
    // Workspace root: unique temp dir per test to avoid cross-test contamination
    config.workspace.root = std::env::temp_dir().join(format!(
        "symphony_agent_test_{}",
        uuid::Uuid::new_v4()
    ));
    config
}

fn make_issue(id: &str, identifier: &str) -> Issue {
    Issue::new(id, identifier, "Test issue for agent runner")
}

async fn collect_updates(
    config: AppConfig,
    issue: Issue,
) -> (Result<(), symphony::agent::AgentError>, Vec<AgentUpdate>) {
    let runner = ClaudeRunner;
    let (tx, mut rx) = mpsc::unbounded_channel::<(String, AgentUpdate)>();
    let cancel = CancellationToken::new();

    let result = runner.run(&issue, None, &config, tx, cancel).await;

    let mut updates = Vec::new();
    while let Ok((_, upd)) = rx.try_recv() {
        updates.push(upd);
    }

    (result, updates)
}

// ─── success path ─────────────────────────────────────────────────────────────

/// ClaudeRunner succeeds when the mock script exits 0 and emits a `result` event.
#[tokio::test]
async fn run_success_emits_turn_complete() {
    let config = make_config("success.sh");
    let issue = make_issue("I_1", "1");

    let (result, updates) = collect_updates(config, issue).await;

    assert!(result.is_ok(), "Expected Ok(()) but got: {:?}", result);

    let turn_complete = updates.iter().find(|u| matches!(u, AgentUpdate::TurnComplete { .. }));
    assert!(turn_complete.is_some(), "Expected TurnComplete update; got: {:?}", updates);
}

/// ClaudeRunner accumulates token counts from the `result` event.
#[tokio::test]
async fn run_success_reports_tokens() {
    let config = make_config("success.sh");
    let issue = make_issue("I_2", "2");

    let (_result, updates) = collect_updates(config, issue).await;

    let token_event = updates.iter().find(|u| {
        matches!(u, AgentUpdate::Event { input_tokens, .. } if *input_tokens > 0)
    });
    assert!(token_event.is_some(), "Expected Event with input_tokens > 0; got: {:?}", updates);
}

/// ClaudeRunner emits a Started update at the beginning of a run.
#[tokio::test]
async fn run_emits_started_update() {
    let config = make_config("success.sh");
    let issue = make_issue("I_3", "3");

    let (_result, updates) = collect_updates(config, issue).await;

    let started = updates.iter().find(|u| matches!(u, AgentUpdate::Started { .. }));
    assert!(started.is_some(), "Expected Started update; got: {:?}", updates);
}

/// ClaudeRunner forwards tool_use events from the stream.
#[tokio::test]
async fn run_success_forwards_tool_use_events() {
    let config = make_config("success.sh");
    let issue = make_issue("I_4", "4");

    let (_result, updates) = collect_updates(config, issue).await;

    let tool_event = updates.iter().find(|u| {
        matches!(u, AgentUpdate::Event { event_type, .. } if event_type == "tool_use")
    });
    assert!(tool_event.is_some(), "Expected tool_use Event; got: {:?}", updates);
}

// ─── error path ───────────────────────────────────────────────────────────────

/// ClaudeRunner returns Err when the stream contains an `error` event.
#[tokio::test]
async fn run_error_event_returns_err() {
    let config = make_config("error.sh");
    let issue = make_issue("I_5", "5");

    let (result, updates) = collect_updates(config, issue).await;

    assert!(result.is_err(), "Expected Err but got Ok");

    // An Error update should also have been sent before the Err return
    let error_update = updates.iter().find(|u| matches!(u, AgentUpdate::Error { .. }));
    assert!(error_update.is_some(), "Expected Error update; got: {:?}", updates);
}

/// ClaudeRunner returns Err(ProcessExit) when the process exits with nonzero status.
#[tokio::test]
async fn run_nonzero_exit_returns_process_exit_err() {
    let config = make_config("nonzero_exit.sh");
    let issue = make_issue("I_6", "6");

    let (result, _updates) = collect_updates(config, issue).await;

    assert!(
        matches!(result, Err(symphony::agent::AgentError::ProcessExit(_))),
        "Expected ProcessExit error; got: {:?}",
        result
    );
}

// ─── cancellation ─────────────────────────────────────────────────────────────

/// ClaudeRunner returns Ok when cancelled mid-run and does not emit TurnComplete.
#[tokio::test]
async fn run_mid_run_cancellation_returns_ok() {
    let mut config = make_config("heartbeat.sh");
    // Use short timeouts to avoid long waits if cancellation fails
    config.claude.read_timeout_ms = 200;
    config.claude.turn_timeout_ms = 1000;

    let issue = make_issue("I_7", "7");

    let runner = ClaudeRunner;
    let (tx, mut rx) = mpsc::unbounded_channel::<(String, AgentUpdate)>();
    let cancel = CancellationToken::new();

    // Spawn the runner in a separate task
    let cancel_clone = cancel.clone();
    let handle = tokio::spawn(async move {
        runner.run(&issue, None, &config, tx, cancel_clone).await
    });

    // Give the process time to spawn and emit at least one heartbeat
    tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

    // Cancel mid-run
    cancel.cancel();

    // Wait for the runner to complete
    let result = handle.await.expect("task should not panic");

    // Cancelled mid-run should return Ok(())
    assert!(result.is_ok(), "Cancelled run should return Ok; got: {:?}", result);

    // Collect all updates and verify TurnComplete was NOT emitted
    let mut updates = Vec::new();
    while let Ok((_, upd)) = rx.try_recv() {
        updates.push(upd);
    }

    let turn_complete = updates.iter().find(|u| matches!(u, AgentUpdate::TurnComplete { .. }));
    assert!(
        turn_complete.is_none(),
        "Cancelled run should NOT emit TurnComplete; got: {:?}",
        updates
    );
}

// ─── command not found ────────────────────────────────────────────────────────

/// ClaudeRunner returns ClaudeNotFound when the command does not exist.
#[tokio::test]
async fn run_missing_command_returns_claude_not_found() {
    let mut config = AppConfig::default();
    config.claude.command = "/nonexistent/path/to/claude".to_string();
    config.workspace.root = std::env::temp_dir().join(format!(
        "symphony_agent_test_{}",
        uuid::Uuid::new_v4()
    ));

    let issue = make_issue("I_8", "8");
    let runner = ClaudeRunner;
    let (tx, _rx) = mpsc::unbounded_channel::<(String, AgentUpdate)>();
    let cancel = CancellationToken::new();

    let result = runner.run(&issue, None, &config, tx, cancel).await;
    assert!(
        matches!(result, Err(symphony::agent::AgentError::ClaudeNotFound)),
        "Expected ClaudeNotFound; got: {:?}",
        result
    );
}

// ─── retry attempt in prompt ──────────────────────────────────────────────────

/// ClaudeRunner handles different retry attempt values without error.
#[tokio::test]
async fn run_with_different_retry_attempts_succeeds() {
    // Test with attempt = Some(1) - first retry
    let config = make_config("success.sh");
    let issue = make_issue("I_9", "9");

    let runner = ClaudeRunner;
    let (tx, mut rx) = mpsc::unbounded_channel::<(String, AgentUpdate)>();
    let cancel = CancellationToken::new();

    let result = runner.run(&issue, Some(1), &config, tx, cancel).await;
    assert!(result.is_ok(), "Run with attempt=1 should succeed; got: {:?}", result);

    let mut updates = Vec::new();
    while let Ok((_, upd)) = rx.try_recv() {
        updates.push(upd);
    }
    assert!(updates.iter().any(|u| matches!(u, AgentUpdate::TurnComplete { .. })));

    // Test with attempt = Some(3) - third retry
    let config = make_config("success.sh");
    let issue = make_issue("I_10", "10");

    let (tx, mut rx) = mpsc::unbounded_channel::<(String, AgentUpdate)>();
    let cancel = CancellationToken::new();

    let result = runner.run(&issue, Some(3), &config, tx, cancel).await;
    assert!(result.is_ok(), "Run with attempt=3 should succeed; got: {:?}", result);

    let mut updates = Vec::new();
    while let Ok((_, upd)) = rx.try_recv() {
        updates.push(upd);
    }
    assert!(updates.iter().any(|u| matches!(u, AgentUpdate::TurnComplete { .. })));
}

/// ClaudeRunner embeds the attempt value in the prompt.
/// The mock (verify_attempt.sh) fails if "continuation attempt" is not in the prompt.
#[tokio::test]
async fn run_retry_attempt_embedded_in_prompt() {
    let config = make_config("verify_attempt.sh");
    let issue = make_issue("I_11", "11");

    let runner = ClaudeRunner;
    let (tx, mut rx) = mpsc::unbounded_channel::<(String, AgentUpdate)>();
    let cancel = CancellationToken::new();

    // Run with attempt=2 - the prompt should contain "continuation attempt"
    let result = runner.run(&issue, Some(2), &config, tx, cancel).await;

    // The mock succeeds only if prompt contains "continuation attempt"
    assert!(
        result.is_ok(),
        "Run with attempt should succeed (prompt should contain attempt); got: {:?}",
        result
    );

    let mut updates = Vec::new();
    while let Ok((_, upd)) = rx.try_recv() {
        updates.push(upd);
    }
    assert!(updates.iter().any(|u| matches!(u, AgentUpdate::TurnComplete { .. })));
}

/// Verify that verify_attempt.sh fails when attempt is None (no continuation message).
#[tokio::test]
async fn run_without_attempt_fails_verify_attempt_mock() {
    let config = make_config("verify_attempt.sh");
    let issue = make_issue("I_12", "12");

    let runner = ClaudeRunner;
    let (tx, _rx) = mpsc::unbounded_channel::<(String, AgentUpdate)>();
    let cancel = CancellationToken::new();

    // Run with attempt=None - the prompt should NOT contain "continuation attempt"
    // The verify_attempt.sh mock should fail
    let result = runner.run(&issue, None, &config, tx, cancel).await;

    // The mock should fail because prompt doesn't contain "continuation attempt"
    assert!(
        result.is_err(),
        "Run without attempt should fail with verify_attempt.sh; got: {:?}",
        result
    );
}

//! Phase 8: CLI tests (PLAN.md §Phase 8)
//!
//! Tests the `symphony` binary CLI using assert_cmd.
//! Covers exit codes, flag parsing, dry-run, and graceful SIGTERM shutdown.

use std::time::Duration;

use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Minimal valid WORKFLOW.md content for CLI tests
// (api_key is a literal so it doesn't require GITHUB_TOKEN env var)
// ---------------------------------------------------------------------------

const MINIMAL_WORKFLOW: &str = r#"---
tracker:
  kind: github
  repo: "test/repo"
  api_key: "ghp_test_token_12345"
---
Test prompt
"#;

fn symphony() -> Command {
    Command::cargo_bin("symphony").unwrap()
}

// ---------------------------------------------------------------------------
// cli_missing_workflow_exits_3
// ---------------------------------------------------------------------------

/// Missing WORKFLOW.md → exit code 3
#[test]
fn cli_missing_workflow_exits_3() {
    symphony()
        .arg("./nonexistent-WORKFLOW.md")
        .assert()
        .failure()
        .code(3);
}

// ---------------------------------------------------------------------------
// cli_default_workflow_path
// ---------------------------------------------------------------------------

/// When no path is given, the binary looks for ./WORKFLOW.md in CWD.
/// If CWD has no WORKFLOW.md, it exits 3.
#[test]
fn cli_default_workflow_path_searches_cwd() {
    let dir = TempDir::new().unwrap();

    // Run from a directory that has no WORKFLOW.md → should fail with code 3
    symphony()
        .current_dir(dir.path())
        .assert()
        .failure()
        .code(3);
}

/// When CWD contains a valid WORKFLOW.md, --dry-run succeeds with exit 0.
#[test]
fn cli_default_workflow_path_uses_cwd_workflow() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("WORKFLOW.md"), MINIMAL_WORKFLOW).unwrap();

    symphony()
        .current_dir(dir.path())
        .arg("--dry-run")
        .assert()
        .success()
        .code(0);
}

// ---------------------------------------------------------------------------
// cli_explicit_path
// ---------------------------------------------------------------------------

/// Explicit path argument is used instead of the default ./WORKFLOW.md.
#[test]
fn cli_explicit_path_uses_provided_file() {
    let dir = TempDir::new().unwrap();
    let custom = dir.path().join("custom.md");
    std::fs::write(&custom, MINIMAL_WORKFLOW).unwrap();

    symphony()
        .arg(&custom)
        .arg("--dry-run")
        .assert()
        .success()
        .code(0);
}

/// Explicit path that does not exist → exit code 3.
#[test]
fn cli_explicit_path_missing_exits_3() {
    symphony()
        .arg("/tmp/symphony_test_nonexistent_path_xyz.md")
        .assert()
        .failure()
        .code(3);
}

// ---------------------------------------------------------------------------
// cli_dry_run_validates_and_exits
// ---------------------------------------------------------------------------

/// --dry-run validates config and exits 0 with a success message.
#[test]
fn cli_dry_run_validates_and_exits() {
    let dir = TempDir::new().unwrap();
    let workflow = dir.path().join("WORKFLOW.md");
    std::fs::write(&workflow, MINIMAL_WORKFLOW).unwrap();

    symphony()
        .arg(&workflow)
        .arg("--dry-run")
        .assert()
        .success()
        .code(0)
        .stdout(predicate::str::contains("Config validated successfully"));
}

/// --dry-run output includes tracker repo and agent info.
#[test]
fn cli_dry_run_shows_config_summary() {
    let dir = TempDir::new().unwrap();
    let workflow = dir.path().join("WORKFLOW.md");
    std::fs::write(&workflow, MINIMAL_WORKFLOW).unwrap();

    symphony()
        .arg(&workflow)
        .arg("--dry-run")
        .assert()
        .success()
        .stdout(predicate::str::contains("test/repo"))
        .stdout(predicate::str::contains("Max concurrent agents"));
}

/// --dry-run with an invalid repo format → exit code 1 (config error).
#[test]
fn cli_dry_run_invalid_config_exits_1() {
    let dir = TempDir::new().unwrap();
    let workflow = dir.path().join("WORKFLOW.md");
    std::fs::write(
        &workflow,
        "---\ntracker:\n  kind: github\n  repo: \"not-valid\"\n  api_key: \"tok\"\n---\nTest\n",
    )
    .unwrap();

    symphony()
        .arg(&workflow)
        .arg("--dry-run")
        .assert()
        .failure()
        .code(1);
}

// ---------------------------------------------------------------------------
// cli_graceful_shutdown  (UNIX only — requires SIGTERM)
// ---------------------------------------------------------------------------

/// SIGTERM causes the binary to exit cleanly with exit code 0.
///
/// Uses a TCP listener that accepts connections but never sends a response,
/// so the orchestrator's first poll blocks inside `reqwest` waiting for HTTP data.
/// When SIGTERM fires, the cancel-safe `select!` drops the in-flight future
/// (cancelling the reqwest call) and the process exits in < 2 s.
#[cfg(unix)]
#[test]
fn cli_graceful_shutdown_on_sigterm() {
    use std::net::TcpListener;
    use std::process::Stdio;

    // Bind a local server that accepts TCP connections but never sends a byte.
    // reqwest will connect successfully and then block waiting for an HTTP response,
    // which exercises the cancel-safe select! in the orchestrator.
    let hanging_server = TcpListener::bind("127.0.0.1:0").unwrap();
    let hanging_port = hanging_server.local_addr().unwrap().port();

    // Background thread: accept connections and hold them open indefinitely.
    std::thread::spawn(move || {
        loop {
            // accept() blocks until a client connects; returned socket is held
            // open until the thread exits (process teardown).
            if hanging_server.accept().is_err() {
                break;
            }
        }
    });

    let workflow_content = format!(
        "---\ntracker:\n  kind: github\n  repo: \"test/repo\"\n  api_key: \"ghp_test_token_12345\"\n  endpoint: \"http://127.0.0.1:{}/graphql\"\n---\nTest prompt\n",
        hanging_port
    );

    let dir = TempDir::new().unwrap();
    let workflow = dir.path().join("WORKFLOW.md");
    std::fs::write(&workflow, &workflow_content).unwrap();

    let mut child = std::process::Command::new(
        assert_cmd::cargo::cargo_bin("symphony"),
    )
    .arg(&workflow)
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .spawn()
    .unwrap();

    // Give the process enough time to initialise and start the first poll,
    // so reqwest is actually blocking on the hanging server.
    std::thread::sleep(Duration::from_millis(500));

    // Send SIGTERM
    // SAFETY: pid is a valid child process pid; kill(2) is async-signal-safe
    let rc = unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    assert_eq!(rc, 0, "kill(SIGTERM) should succeed");

    // Wait for the process to exit (with a generous timeout)
    let start = std::time::Instant::now();
    loop {
        match child.try_wait().unwrap() {
            Some(status) => {
                assert_eq!(status.code(), Some(0), "expected exit 0 after SIGTERM");
                return;
            }
            None => {
                if start.elapsed() > Duration::from_secs(5) {
                    child.kill().ok();
                    panic!("process did not exit within 5 s after SIGTERM");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

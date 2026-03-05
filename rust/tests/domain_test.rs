//! Domain module integration tests

//!
//! These tests verify the domain module functions correctly.

use symphony::domain::{Issue, BlockerRef, TokenTotals, TokenUsage, RetryEntry};
use symphony::workflow::load_workflow;
use symphony::config::AppConfig;
use symphony::prompt::render_prompt;

use tempfile::NamedTempFile;
use std::io::Write;
use std::time::Duration;

fn create_temp_file(contents: &str) -> NamedTempFile {
    let mut file = NamedTempFile::new().unwrap();
    write!(file, "{}", contents).unwrap();
    file
}

#[test]
fn test_issue_creation() {
    let issue = Issue::new("gid://github/Issue/42", "42", "Test Issue");
    assert_eq!(issue.id, "gid://github/Issue/42");
    assert_eq!(issue.identifier, "42");
    assert_eq!(issue.title, "Test Issue");
    assert!(issue.is_active());
}

#[test]
fn test_workflow_parsing() {
    let contents = r#"---
tracker:
  kind: github
  repo: "owner/repo"
  api_key: test_key
---

You are working on Issue #{{ issue.identifier }}.
"#;
    let file = create_temp_file(contents);
    let workflow = load_workflow(file.path()).unwrap();
    assert!(workflow.config.is_mapping());
}

#[test]
fn test_config_defaults() {
    let config = AppConfig::default();
    assert_eq!(config.tracker.kind, "github");
    assert_eq!(config.polling.interval_ms, 30000);
    assert_eq!(config.agent.max_concurrent_agents, 10);
}

#[test]
fn test_prompt_rendering() {
    let mut issue = Issue::new("gid://github/Issue/1", "1", "Test");
    issue.description = Some("Test description".to_string());

    let template = "Issue #{{ issue.identifier }}: {{ issue.title }}";
    let result = render_prompt(template, &issue, None, "owner/repo").unwrap();
    assert_eq!(result, "Issue #1: Test");
}

#[tokio::test]
async fn test_token_totals() {
    let mut totals = TokenTotals::new();
    totals.add(&TokenUsage {
        input_tokens: 100,
        output_tokens: 50,
        cache_read_tokens: None,
        cache_creation_tokens: None,
    });
    assert_eq!(totals.input_tokens, 100);
    assert_eq!(totals.output_tokens, 50);
    assert_eq!(totals.total_tokens, 150);
}

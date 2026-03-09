//! Prompt template rendering (SPEC §12)
//!
//! Renders Liquid templates with issue context.

use liquid::{ParserBuilder, Object};
use liquid::model::Value;
use thiserror::Error;

use crate::domain::Issue;

/// Errors that can occur during prompt rendering
#[derive(Debug, Error)]
pub enum PromptError {
    #[error("Failed to parse template: {0}")]
    ParseError(String),

    #[error("Failed to render template: {0}")]
    RenderError(String),
}

impl From<liquid::Error> for PromptError {
    fn from(e: liquid::Error) -> Self {
        // Try to determine if it's a parse or render error
        let msg = e.to_string();
        if msg.contains("parse") || msg.contains("syntax") {
            PromptError::ParseError(msg)
        } else {
            PromptError::RenderError(msg)
        }
    }
}

/// Render a prompt template with the given issue context
///
/// # Arguments
/// * `template` - The Liquid template string
/// * `issue` - The issue to render the template for
/// * `attempt` - The attempt number (None for first run, Some(n) for retries)
/// * `repo` - The repository name (owner/repo format)
///
/// # Available Variables
/// - `issue.id` - Issue GraphQL node ID
/// - `issue.identifier` - Issue number as string
/// - `issue.title` - Issue title
/// - `issue.description` - Issue body (or "No description provided.")
/// - `issue.state` - Issue state ("open" or "closed")
/// - `issue.labels` - Comma-separated list of labels
/// - `issue.url` - Issue URL
/// - `attempt` - Current attempt number (1-indexed, or None)
/// - `repo` - Repository name
pub fn render_prompt(
    template: &str,
    issue: &Issue,
    attempt: Option<u32>,
    repo: &str,
) -> Result<String, PromptError> {
    // Build parser with strict mode (unknown variables cause errors)
    let parser = ParserBuilder::with_stdlib()
        .build()?;

    // Parse template
    let tmpl = parser.parse(template)?;

    // Build context
    let mut context = Object::new();

    // Issue object
    let mut issue_obj = Object::new();
    issue_obj.insert("id".into(), Value::scalar(issue.id.clone()));
    issue_obj.insert("identifier".into(), Value::scalar(issue.identifier.clone()));
    issue_obj.insert("title".into(), Value::scalar(issue.title.clone()));
    issue_obj.insert("description".into(), Value::scalar(
        issue.description.clone().unwrap_or_else(|| "No description provided.".to_string())
    ));
    issue_obj.insert("state".into(), Value::scalar(issue.state.clone()));
    issue_obj.insert("labels".into(), Value::scalar(issue.labels.join(", ")));
    issue_obj.insert("url".into(), Value::scalar(
        issue.url.clone().unwrap_or_default()
    ));

    context.insert("issue".into(), Value::Object(issue_obj));

    // Attempt
    context.insert("attempt".into(), match attempt {
        Some(n) => Value::scalar(n as i64),
        None => Value::Nil,
    });

    // Repo
    context.insert("repo".into(), Value::scalar(repo.to_string()));

    // Render
    let result = tmpl.render(&context)?;

    Ok(result)
}

/// Default prompt template when none is provided in WORKFLOW.md
pub const DEFAULT_PROMPT_TEMPLATE: &str = r#"You are working on GitHub Issue #{{ issue.identifier }}

{% if attempt %}
This is continuation attempt #{{ attempt }}. Resume from current workspace state.
{% endif %}

Issue: #{{ issue.identifier }} - {{ issue.title }}
State: {{ issue.state }}
Labels: {{ issue.labels }}
URL: {{ issue.url }}

Description:
{{ issue.description }}

Instructions:
1. This is an unattended session. Do not ask for human input.
2. Work only in the provided workspace directory.
3. Create a feature branch from main, implement the changes, and push.
4. Create a Pull Request using `gh pr create`.
5. If blocked, add a comment to the issue explaining the blocker.
6. Only stop if you encounter a true blocker (missing auth, permissions, secrets).
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_issue() -> Issue {
        let mut issue = Issue::new("gid://github/Issue/42", "42", "Fix the bug");
        issue.description = Some("There is a bug in the code.".to_string());
        issue.state = "open".to_string();
        issue.labels = vec!["bug".to_string(), "symphony".to_string()];
        issue.url = Some("https://github.com/owner/repo/issues/42".to_string());
        issue
    }

    #[test]
    fn prompt_render_basic() {
        let issue = create_test_issue();
        let template = "Issue #{{ issue.identifier }}: {{ issue.title }}";

        let result = render_prompt(template, &issue, None, "owner/repo").unwrap();

        assert_eq!(result, "Issue #42: Fix the bug");
    }

    #[test]
    fn prompt_render_all_issue_fields() {
        let issue = create_test_issue();
        let template = r#"ID: {{ issue.id }}
Identifier: {{ issue.identifier }}
Title: {{ issue.title }}
Description: {{ issue.description }}
State: {{ issue.state }}
Labels: {{ issue.labels }}
URL: {{ issue.url }}"#;

        let result = render_prompt(template, &issue, None, "owner/repo").unwrap();

        assert!(result.contains("ID: gid://github/Issue/42"));
        assert!(result.contains("Identifier: 42"));
        assert!(result.contains("Title: Fix the bug"));
        assert!(result.contains("Description: There is a bug in the code."));
        assert!(result.contains("State: open"));
        assert!(result.contains("Labels: bug, symphony"));
        assert!(result.contains("URL: https://github.com/owner/repo/issues/42"));
    }

    #[test]
    fn prompt_render_attempt_variable() {
        let issue = create_test_issue();

        // First attempt (None)
        let template = "{% if attempt %}Attempt #{{ attempt }}{% else %}First run{% endif %}";
        let result = render_prompt(template, &issue, None, "owner/repo").unwrap();
        assert_eq!(result, "First run");

        // Retry attempt
        let result = render_prompt(template, &issue, Some(2), "owner/repo").unwrap();
        assert_eq!(result, "Attempt #2");
    }

    #[test]
    fn prompt_render_repo_variable() {
        let issue = create_test_issue();
        let template = "Repo: {{ repo }}";

        let result = render_prompt(template, &issue, None, "myorg/myproject").unwrap();

        assert_eq!(result, "Repo: myorg/myproject");
    }

    #[test]
    fn prompt_render_empty_description() {
        let mut issue = create_test_issue();
        issue.description = None;

        let template = "Description: {{ issue.description }}";
        let result = render_prompt(template, &issue, None, "owner/repo").unwrap();

        assert_eq!(result, "Description: No description provided.");
    }

    #[test]
    fn prompt_render_strict_mode_unknown_variable() {
        let issue = create_test_issue();
        let template = "Unknown: {{ unknown_variable }}";

        let result = render_prompt(template, &issue, None, "owner/repo");

        // Liquid in strict mode should error on unknown variables
        assert!(result.is_err());
    }

    #[test]
    fn prompt_render_conditionals() {
        let issue = create_test_issue();
        let template = r#"{% if issue.state == "open" %}
The issue is open.
{% else %}
The issue is closed.
{% endif %}"#;

        let result = render_prompt(template, &issue, None, "owner/repo").unwrap();
        assert!(result.contains("The issue is open."));

        let mut closed_issue = issue.clone();
        closed_issue.state = "closed".to_string();
        let result = render_prompt(template, &closed_issue, None, "owner/repo").unwrap();
        assert!(result.contains("The issue is closed."));
    }

    #[test]
    fn prompt_render_loops() {
        let issue = create_test_issue();
        // Note: labels is rendered as a string, not an array in this implementation
        let template = "Labels: {{ issue.labels }}";

        let result = render_prompt(template, &issue, None, "owner/repo").unwrap();

        assert!(result.contains("bug, symphony"));
    }

    #[test]
    fn prompt_render_default_template() {
        let issue = create_test_issue();
        let result = render_prompt(DEFAULT_PROMPT_TEMPLATE, &issue, None, "owner/repo").unwrap();

        assert!(result.contains("Issue #42"));
        assert!(result.contains("Fix the bug"));
        assert!(result.contains("bug, symphony"));
        assert!(result.contains("https://github.com/owner/repo/issues/42"));
        assert!(result.contains("owner/repo"));
    }

    #[test]
    fn prompt_render_multiline() {
        let issue = create_test_issue();
        let template = r#"Working on Issue #{{ issue.identifier }}

Title: {{ issue.title }}
Description:
{{ issue.description }}

Good luck!"#;

        let result = render_prompt(template, &issue, None, "owner/repo").unwrap();

        assert!(result.starts_with("Working on Issue #42"));
        assert!(result.contains("Title: Fix the bug"));
        assert!(result.contains("There is a bug in the code."));
        assert!(result.ends_with("Good luck!"));
    }

    #[test]
    fn prompt_render_attempt_continuation_message() {
        let issue = create_test_issue();
        let template = r#"{% if attempt %}
This is continuation attempt #{{ attempt }}. Resume from current workspace state.
{% endif %}
Starting work on Issue #{{ issue.identifier }}."#;

        // First run - no continuation message
        let result = render_prompt(template, &issue, None, "owner/repo").unwrap();
        assert!(!result.contains("continuation attempt"));
        assert!(result.contains("Starting work on Issue #42."));

        // Retry - with continuation message
        let result = render_prompt(template, &issue, Some(3), "owner/repo").unwrap();
        assert!(result.contains("continuation attempt #3"));
        assert!(result.contains("Starting work on Issue #42."));
    }
}

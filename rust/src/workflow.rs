//! WORKFLOW.md loader and parser (SPEC §5)
//!
//! Parses YAML front matter followed by a Liquid template body.

use std::path::Path;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors that can occur during workflow loading
#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error("Workflow file not found: {0}")]
    MissingFile(String),

    #[error("Failed to read workflow file: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Failed to parse YAML front matter: {0}")]
    ParseError(#[from] serde_yaml::Error),

    #[error("Front matter must be a map, got {0}")]
    FrontMatterNotAMap(String),
}

/// Raw workflow configuration from YAML front matter
pub type WorkflowConfig = serde_yaml::Value;

/// Loaded workflow with config and prompt template
#[derive(Debug, Clone)]
pub struct LoadedWorkflow {
    /// Parsed YAML front matter (raw value, to be processed by config module)
    pub config: WorkflowConfig,
    /// Prompt template body (Liquid format)
    pub prompt_template: String,
    /// Path to the workflow file
    pub path: String,
}

/// Load a workflow file
///
/// The file format is:
/// ```yaml
/// ---
/// tracker:
///   kind: github
///   ...
/// ---
///
/// Prompt template body here...
/// ```
pub fn load_workflow<P: AsRef<Path>>(path: P) -> Result<LoadedWorkflow, WorkflowError> {
    let path_ref = path.as_ref();
    let path_str = path_ref.display().to_string();

    // Check file exists
    if !path_ref.exists() {
        return Err(WorkflowError::MissingFile(path_str));
    }

    // Read file contents
    let contents = std::fs::read_to_string(path_ref)?;

    // Parse front matter and body
    let (config, prompt_template) = parse_front_matter(&contents, &path_str)?;

    Ok(LoadedWorkflow {
        config,
        prompt_template,
        path: path_str,
    })
}

/// Parse front matter and body from file contents
fn parse_front_matter(contents: &str, _path: &str) -> Result<(WorkflowConfig, String), WorkflowError> {
    // Check for YAML front matter (starts with ---)
    let trimmed = contents.trim_start();

    if !trimmed.starts_with("---") {
        // No front matter - entire file is the prompt template
        return Ok((WorkflowConfig::Mapping(serde_yaml::Mapping::new()), contents.to_string()));
    }

    // Find the end of front matter (closing ---)
    // Look for a line that is just "---" (possibly with whitespace)
    let after_first_dash = &trimmed[3..]; // Skip first ---
    let remaining = after_first_dash.trim_start_matches(|c| c == '\n' || c == '\r');

    // Find the closing --- on its own line
    let mut end_marker_pos = None;
    for (i, line) in remaining.lines().enumerate() {
        if line.trim() == "---" {
            // Found closing marker
            let byte_offset = remaining.lines().take(i).map(|l| l.len() + 1).sum::<usize>();
            end_marker_pos = Some(byte_offset);
            break;
        }
    }

    match end_marker_pos {
        Some(pos) => {
            let front_matter_str = &remaining[..pos.saturating_sub(1)]; // Exclude trailing newline before ---
            let body_start = pos + 4; // Skip the closing --- and newline

            // Parse YAML - handle empty front matter
            let config: WorkflowConfig = if front_matter_str.trim().is_empty() {
                // Empty front matter -> empty map
                WorkflowConfig::Mapping(serde_yaml::Mapping::new())
            } else {
                serde_yaml::from_str(front_matter_str)?
            };

            // Validate it's a map
            if !config.is_mapping() {
                let type_name = match config {
                    WorkflowConfig::Null => "null",
                    WorkflowConfig::Bool(_) => "boolean",
                    WorkflowConfig::Number(_) => "number",
                    WorkflowConfig::String(_) => "string",
                    WorkflowConfig::Sequence(_) => "sequence",
                    WorkflowConfig::Mapping(_) => "mapping",
                    WorkflowConfig::Tagged(_) => "tagged",
                };
                return Err(WorkflowError::FrontMatterNotAMap(type_name.to_string()));
            }

            let prompt_template = if body_start < remaining.len() {
                remaining[body_start..].trim_start().to_string()
            } else {
                String::new()
            };
            Ok((config, prompt_template))
        }
        None => {
            // Front matter not properly closed - treat entire file as template
            Ok((WorkflowConfig::Mapping(serde_yaml::Mapping::new()), contents.to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    fn create_temp_file(contents: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "{}", contents).unwrap();
        file
    }

    #[test]
    fn load_workflow_valid_yaml_front_matter() {
        let contents = r#"---
tracker:
  kind: github
  repo: "owner/repo"
polling:
  interval_ms: 30000
---

You are working on Issue #{{ issue.identifier }}.
Title: {{ issue.title }}
"#;
        let file = create_temp_file(contents);
        let result = load_workflow(file.path());

        assert!(result.is_ok());
        let workflow = result.unwrap();

        // Check config
        assert!(workflow.config.is_mapping());
        let mapping = workflow.config.as_mapping().unwrap();
        assert!(mapping.contains_key(&serde_yaml::Value::String("tracker".to_string())));
        assert!(mapping.contains_key(&serde_yaml::Value::String("polling".to_string())));

        // Check prompt template
        assert!(workflow.prompt_template.starts_with("You are working"));
        assert!(workflow.prompt_template.contains("{{ issue.identifier }}"));
    }

    #[test]
    fn load_workflow_no_front_matter() {
        let contents = "Just a prompt template with {{ variable }}";
        let file = create_temp_file(contents);
        let result = load_workflow(file.path());

        assert!(result.is_ok());
        let workflow = result.unwrap();

        // Config should be empty map
        assert!(workflow.config.is_mapping());
        assert!(workflow.config.as_mapping().unwrap().is_empty());

        // Template should be entire file
        assert_eq!(workflow.prompt_template, contents);
    }

    #[test]
    fn load_workflow_missing_file() {
        let result = load_workflow("/nonexistent/path/WORKFLOW.md");
        assert!(matches!(result, Err(WorkflowError::MissingFile(_))));
    }

    #[test]
    fn load_workflow_invalid_yaml() {
        let contents = "---\ninvalid: yaml: colon: :\n---\nPrompt";
        let file = create_temp_file(contents);
        let result = load_workflow(file.path());

        assert!(matches!(result, Err(WorkflowError::ParseError(_))));
    }

    #[test]
    fn load_workflow_front_matter_not_a_map() {
        let contents = "---\n- item1\n- item2\n---\nPrompt";
        let file = create_temp_file(contents);
        let result = load_workflow(file.path());

        assert!(matches!(result, Err(WorkflowError::FrontMatterNotAMap(_))));
        if let Err(WorkflowError::FrontMatterNotAMap(type_name)) = result {
            assert_eq!(type_name, "sequence");
        }
    }

    #[test]
    fn load_workflow_empty_front_matter() {
        let contents = "---\n---\nPrompt template here";
        let file = create_temp_file(contents);
        let result = load_workflow(file.path());

        assert!(result.is_ok());
        let workflow = result.unwrap();

        // Empty map
        assert!(workflow.config.is_mapping());
        assert!(workflow.config.as_mapping().unwrap().is_empty());
        assert_eq!(workflow.prompt_template, "Prompt template here");
    }

    #[test]
    fn load_workflow_unclosed_front_matter() {
        // No closing ---, entire file should be template
        let contents = "---\ntracker:\n  kind: github\n\nThis is the prompt";
        let file = create_temp_file(contents);
        let result = load_workflow(file.path());

        assert!(result.is_ok());
        let workflow = result.unwrap();

        // Should treat entire file as template
        assert!(workflow.config.as_mapping().unwrap().is_empty());
        assert_eq!(workflow.prompt_template, contents);
    }

    #[test]
    fn load_workflow_multiline_prompt() {
        let contents = "---\nkey: value\n---\n\nLine 1\nLine 2\nLine 3\n";
        let file = create_temp_file(contents);
        let result = load_workflow(file.path());

        assert!(result.is_ok());
        let workflow = result.unwrap();

        assert!(workflow.prompt_template.starts_with("Line 1"));
        assert!(workflow.prompt_template.contains("Line 2"));
        assert!(workflow.prompt_template.contains("Line 3"));
    }
}

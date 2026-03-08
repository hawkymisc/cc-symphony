//! Issue data model (SPEC §4.1.1)

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Reference to a blocking issue
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockerRef {
    /// The identifier of the blocking issue (e.g., "42" for GitHub Issue #42)
    pub identifier: String,
    /// Whether the blocker is currently active
    pub is_active: bool,
}

/// Represents an issue from the tracker (GitHub Issues)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    /// GraphQL node ID (unique identifier)
    pub id: String,
    /// Human-readable identifier (Issue number as string, e.g., "42")
    pub identifier: String,
    /// Issue title
    pub title: String,
    /// Issue body/description
    pub description: Option<String>,
    /// Priority (lower = higher priority); None means no priority set (sorts last)
    pub priority: Option<i32>,
    /// Current state ("open" or "closed" for GitHub)
    pub state: String,
    /// Associated branch name (always None for MVP)
    pub branch_name: Option<String>,
    /// URL to the issue
    pub url: Option<String>,
    /// Labels attached to the issue (lowercase)
    pub labels: Vec<String>,
    /// Issues blocking this one (empty for GitHub MVP)
    pub blocked_by: Vec<BlockerRef>,
    /// Creation timestamp
    pub created_at: Option<DateTime<Utc>>,
    /// Last update timestamp
    pub updated_at: Option<DateTime<Utc>>,
}

impl Issue {
    /// Create a new issue with minimal required fields
    pub fn new(id: impl Into<String>, identifier: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            identifier: identifier.into(),
            title: title.into(),
            description: None,
            priority: None,
            state: "open".to_string(),
            branch_name: None,
            url: None,
            labels: Vec::new(),
            blocked_by: Vec::new(),
            created_at: None,
            updated_at: None,
        }
    }

    /// Check if this issue is in an active state
    pub fn is_active(&self) -> bool {
        self.state.to_lowercase() == "open"
    }

    /// Check if this issue is in a terminal state
    pub fn is_terminal(&self) -> bool {
        self.state.to_lowercase() == "closed"
    }

    /// Check if this issue is blocked
    pub fn is_blocked(&self) -> bool {
        self.blocked_by.iter().any(|b| b.is_active)
    }

    /// Sanitize the identifier for use in paths
    /// Replaces `/` and other unsafe characters with `_`
    pub fn sanitized_identifier(&self) -> String {
        self.identifier
            .chars()
            .map(|c| match c {
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | ' ' => '_',
                _ => c,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_new_creates_minimal_issue() {
        let issue = Issue::new("gid://github/Issue/42", "42", "Test Issue");

        assert_eq!(issue.id, "gid://github/Issue/42");
        assert_eq!(issue.identifier, "42");
        assert_eq!(issue.title, "Test Issue");
        assert_eq!(issue.description, None);
        assert_eq!(issue.priority, None);
        assert_eq!(issue.state, "open");
        assert!(issue.is_active());
        assert!(!issue.is_terminal());
        assert!(issue.labels.is_empty());
        assert!(issue.blocked_by.is_empty());
    }

    #[test]
    fn issue_is_active_checks_state() {
        let mut issue = Issue::new("1", "1", "Test");
        issue.state = "open".to_string();
        assert!(issue.is_active());

        issue.state = "OPEN".to_string(); // case insensitive
        assert!(issue.is_active());

        issue.state = "closed".to_string();
        assert!(!issue.is_active());
    }

    #[test]
    fn issue_is_terminal_checks_state() {
        let mut issue = Issue::new("1", "1", "Test");
        issue.state = "closed".to_string();
        assert!(issue.is_terminal());

        issue.state = "CLOSED".to_string(); // case insensitive
        assert!(issue.is_terminal());

        issue.state = "open".to_string();
        assert!(!issue.is_terminal());
    }

    #[test]
    fn issue_is_blocked_checks_blockers() {
        let mut issue = Issue::new("1", "1", "Test");

        // No blockers
        assert!(!issue.is_blocked());

        // Inactive blocker
        issue.blocked_by.push(BlockerRef {
            identifier: "2".to_string(),
            is_active: false,
        });
        assert!(!issue.is_blocked());

        // Active blocker
        issue.blocked_by.push(BlockerRef {
            identifier: "3".to_string(),
            is_active: true,
        });
        assert!(issue.is_blocked());
    }

    #[test]
    fn is_blocked_with_multiple_active_blockers() {
        let mut issue = Issue::new("1", "1", "Test");
        issue.blocked_by = vec![
            BlockerRef {
                identifier: "2".to_string(),
                is_active: true,
            },
            BlockerRef {
                identifier: "3".to_string(),
                is_active: true,
            },
            BlockerRef {
                identifier: "4".to_string(),
                is_active: true,
            },
        ];
        assert!(issue.is_blocked());
    }

    #[test]
    fn is_blocked_with_all_inactive_blockers() {
        let mut issue = Issue::new("1", "1", "Test");
        issue.blocked_by = vec![
            BlockerRef {
                identifier: "2".to_string(),
                is_active: false,
            },
            BlockerRef {
                identifier: "3".to_string(),
                is_active: false,
            },
            BlockerRef {
                identifier: "4".to_string(),
                is_active: false,
            },
            BlockerRef {
                identifier: "5".to_string(),
                is_active: false,
            },
            BlockerRef {
                identifier: "6".to_string(),
                is_active: false,
            },
        ];
        assert!(!issue.is_blocked());
    }

    #[test]
    fn is_blocked_with_one_active_among_many_inactive() {
        let mut issue = Issue::new("1", "1", "Test");
        // 9 inactive + 1 active = blocked
        let mut blockers: Vec<BlockerRef> = (2..=10)
            .map(|i| BlockerRef {
                identifier: i.to_string(),
                is_active: false,
            })
            .collect();
        blockers.push(BlockerRef {
            identifier: "11".to_string(),
            is_active: true,
        });
        issue.blocked_by = blockers;
        assert!(issue.is_blocked());
    }

    #[test]
    fn is_blocked_empty_blockers_explicit() {
        let mut issue = Issue::new("1", "1", "Test");
        issue.blocked_by = vec![];
        assert!(!issue.is_blocked());
    }

    #[test]
    fn is_blocked_combined_with_terminal_state() {
        // A closed issue can still have active blockers
        let mut issue = Issue::new("1", "1", "Test");
        issue.state = "closed".to_string();
        issue.blocked_by = vec![BlockerRef {
            identifier: "2".to_string(),
            is_active: true,
        }];
        // is_blocked checks blockers regardless of state
        assert!(issue.is_blocked());
        // but the issue is terminal
        assert!(issue.is_terminal());
        assert!(!issue.is_active());
    }

    #[test]
    fn issue_sanitized_identifier_replaces_unsafe_chars() {
        let issue = Issue::new("1", "ABC-123", "Test");
        assert_eq!(issue.sanitized_identifier(), "ABC-123");

        let issue = Issue::new("1", "foo/bar", "Test");
        assert_eq!(issue.sanitized_identifier(), "foo_bar");

        let issue = Issue::new("1", "hello world", "Test");
        assert_eq!(issue.sanitized_identifier(), "hello_world");

        let issue = Issue::new("1", "a:b*c?d\"e<f>g|h", "Test");
        assert_eq!(issue.sanitized_identifier(), "a_b_c_d_e_f_g_h");
    }

    #[test]
    fn issue_serialization_roundtrip() {
        let mut issue = Issue::new("gid://github/Issue/42", "42", "Test Issue");
        issue.description = Some("This is a test".to_string());
        issue.priority = Some(1);
        issue.labels = vec!["bug".to_string(), "symphony".to_string()];

        let json = serde_json::to_string(&issue).unwrap();
        let deserialized: Issue = serde_json::from_str(&json).unwrap();

        assert_eq!(issue, deserialized);
    }
}

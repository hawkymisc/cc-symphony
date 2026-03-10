//! In-memory tracker for testing

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::domain::Issue;
use super::{Tracker, TrackerError};

/// In-memory tracker for testing purposes
#[derive(Clone)]
pub struct MemoryTracker {
    issues: Arc<RwLock<Vec<Issue>>>,
}

impl MemoryTracker {
    /// Create a new empty memory tracker
    pub fn new() -> Self {
        Self {
            issues: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Create a tracker with initial issues
    pub fn with_issues(issues: Vec<Issue>) -> Self {
        Self {
            issues: Arc::new(RwLock::new(issues)),
        }
    }

    /// Add an issue to the tracker
    pub async fn add_issue(&self, issue: Issue) {
        let mut issues = self.issues.write().await;
        issues.push(issue);
    }

    /// Update an issue's state
    pub async fn update_state(&self, id: &str, new_state: &str) {
        let mut issues = self.issues.write().await;
        if let Some(issue) = issues.iter_mut().find(|i| i.id == id) {
            issue.state = new_state.to_string();
        }
    }

    /// Remove an issue from the tracker
    pub async fn remove_issue(&self, id: &str) {
        let mut issues = self.issues.write().await;
        issues.retain(|i| i.id != id);
    }

    /// Get all issues (for testing)
    pub async fn get_all(&self) -> Vec<Issue> {
        self.issues.read().await.clone()
    }
}

impl Default for MemoryTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Tracker for MemoryTracker {
    async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError> {
        let issues = self.issues.read().await;
        // Return only issues in "open" state
        Ok(issues.iter().filter(|i| i.is_active()).cloned().collect())
    }

    async fn fetch_issues_by_ids(&self, ids: &[String]) -> Result<Vec<Issue>, TrackerError> {
        let issues = self.issues.read().await;
        Ok(issues
            .iter()
            .filter(|i| ids.contains(&i.id))
            .cloned()
            .collect())
    }

    async fn fetch_issues_by_states(&self, states: &[String]) -> Result<Vec<Issue>, TrackerError> {
        let states_lower: Vec<String> = states.iter().map(|s| s.to_lowercase()).collect();
        let issues = self.issues.read().await;
        Ok(issues
            .iter()
            .filter(|i| states_lower.contains(&i.state.to_lowercase()))
            .cloned()
            .collect())
    }

    async fn add_label(&self, issue_identifier: &str, label: &str) -> Result<(), TrackerError> {
        let mut issues = self.issues.write().await;
        if let Some(issue) = issues.iter_mut().find(|i| i.identifier == issue_identifier) {
            let label_lower = label.to_lowercase();
            if !issue.labels.contains(&label_lower) {
                issue.labels.push(label_lower);
            }
        }
        Ok(())
    }

    async fn remove_label(&self, issue_identifier: &str, label: &str) -> Result<(), TrackerError> {
        let mut issues = self.issues.write().await;
        if let Some(issue) = issues.iter_mut().find(|i| i.identifier == issue_identifier) {
            let label_lower = label.to_lowercase();
            issue.labels.retain(|l| l != &label_lower);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_tracker_add_and_fetch() {
        let tracker = MemoryTracker::new();

        let issue = Issue::new("1", "42", "Test Issue");
        tracker.add_issue(issue).await;

        let candidates = tracker.fetch_candidate_issues().await.unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].identifier, "42");
    }

    #[tokio::test]
    async fn memory_tracker_filter_by_state() {
        let tracker = MemoryTracker::new();

        let mut open_issue = Issue::new("1", "1", "Open");
        open_issue.state = "open".to_string();

        let mut closed_issue = Issue::new("2", "2", "Closed");
        closed_issue.state = "closed".to_string();

        tracker.add_issue(open_issue).await;
        tracker.add_issue(closed_issue).await;

        let candidates = tracker.fetch_candidate_issues().await.unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].identifier, "1");
    }

    #[tokio::test]
    async fn memory_tracker_update_state() {
        let tracker = MemoryTracker::new();

        let issue = Issue::new("1", "42", "Test");
        tracker.add_issue(issue).await;

        tracker.update_state("1", "closed").await;

        let candidates = tracker.fetch_candidate_issues().await.unwrap();
        assert!(candidates.is_empty());
    }

    #[tokio::test]
    async fn memory_tracker_add_label() {
        let tracker = MemoryTracker::new();
        tracker.add_issue(Issue::new("1", "42", "Test")).await;

        tracker.add_label("42", "symphony-doing").await.unwrap();

        let issues = tracker.get_all().await;
        assert!(issues[0].labels.contains(&"symphony-doing".to_string()));
    }

    #[tokio::test]
    async fn memory_tracker_add_label_idempotent() {
        let tracker = MemoryTracker::new();
        tracker.add_issue(Issue::new("1", "42", "Test")).await;

        tracker.add_label("42", "symphony-doing").await.unwrap();
        tracker.add_label("42", "symphony-doing").await.unwrap();

        let issues = tracker.get_all().await;
        assert_eq!(issues[0].labels.iter().filter(|l| *l == "symphony-doing").count(), 1);
    }

    #[tokio::test]
    async fn memory_tracker_remove_label() {
        let tracker = MemoryTracker::new();
        let mut issue = Issue::new("1", "42", "Test");
        issue.labels = vec!["symphony-doing".to_string(), "bug".to_string()];
        tracker.add_issue(issue).await;

        tracker.remove_label("42", "symphony-doing").await.unwrap();

        let issues = tracker.get_all().await;
        assert!(!issues[0].labels.contains(&"symphony-doing".to_string()));
        assert!(issues[0].labels.contains(&"bug".to_string()));
    }

    #[tokio::test]
    async fn memory_tracker_remove_label_not_present() {
        let tracker = MemoryTracker::new();
        tracker.add_issue(Issue::new("1", "42", "Test")).await;

        // Removing a label that doesn't exist should succeed
        tracker.remove_label("42", "symphony-doing").await.unwrap();

        let issues = tracker.get_all().await;
        assert!(issues[0].labels.is_empty());
    }

    #[tokio::test]
    async fn memory_tracker_fetch_by_ids() {
        let tracker = MemoryTracker::new();

        tracker.add_issue(Issue::new("1", "1", "A")).await;
        tracker.add_issue(Issue::new("2", "2", "B")).await;
        tracker.add_issue(Issue::new("3", "3", "C")).await;

        let result = tracker.fetch_issues_by_ids(&["1".to_string(), "3".to_string()]).await.unwrap();
        assert_eq!(result.len(), 2);
    }
}

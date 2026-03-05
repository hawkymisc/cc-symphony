//! Dispatch logic for selecting candidates

use std::collections::{HashMap, HashSet};

use crate::domain::{Issue, RetryEntry};
use crate::orchestrator::RunningEntry;

/// Select issues to dispatch from candidates
///
/// # Arguments
/// * `candidates` - All candidate issues
/// * `running` - Currently running entries
/// * `claimed` - Claimed issue IDs
/// * `retry_attempts` - Retry queue
/// * `max_concurrent` - Maximum concurrent agents
///
/// # Returns
/// List of issues to dispatch, sorted by priority
pub fn select_candidates(
    candidates: &[Issue],
    running: &HashMap<String, RunningEntry>,
    claimed: &HashSet<String>,
    retry_attempts: &HashMap<String, RetryEntry>,
    max_concurrent: usize,
) -> Vec<Issue> {
    let available_slots = max_concurrent.saturating_sub(running.len());

    if available_slots == 0 {
        return Vec::new();
    }

    // Filter eligible issues
    let mut eligible: Vec<&Issue> = candidates
        .iter()
        .filter(|issue| {
            // Not already claimed
            !claimed.contains(&issue.id) &&
            // Not in retry queue
            !retry_attempts.contains_key(&issue.id) &&
            // Is in active state
            issue.is_active() &&
            // Not blocked
            !issue.is_blocked()
        })
        .collect();

    // Sort by priority (lower number = higher priority), then by creation date
    eligible.sort_by(|a, b| {
        // First compare priority (None sorts last)
        let priority_cmp = match (a.priority, b.priority) {
            (Some(p1), Some(p2)) => p1.cmp(&p2),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        };

        if priority_cmp != std::cmp::Ordering::Equal {
            return priority_cmp;
        }

        // Then compare creation date (older = higher priority)
        match (a.created_at, b.created_at) {
            (Some(t1), Some(t2)) => t1.cmp(&t2),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });

    // Take up to available slots
    eligible
        .into_iter()
        .take(available_slots)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn create_test_issue(id: &str, identifier: &str, priority: Option<i32>) -> Issue {
        let mut issue = Issue::new(id, identifier, "Test");
        issue.priority = priority;
        issue.created_at = Some(Utc::now());
        issue
    }

    #[test]
    fn dispatch_priority_sort() {
        let mut candidates = vec![
            create_test_issue("3", "3", Some(3)),
            create_test_issue("1", "1", Some(1)),
            create_test_issue("2", "2", Some(2)),
        ];

        // Set different creation times
        candidates[0].created_at = Some(Utc.with_ymd_and_hms(2024, 1, 3, 0, 0, 0).unwrap());
        candidates[1].created_at = Some(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap());
        candidates[2].created_at = Some(Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap());

        let selected = select_candidates(&candidates, &HashMap::new(), &HashSet::new(), &HashMap::new(), 10);

        assert_eq!(selected.len(), 3);
        // Priority 1 should be first
        assert_eq!(selected[0].identifier, "1");
        assert_eq!(selected[1].identifier, "2");
        assert_eq!(selected[2].identifier, "3");
    }

    #[test]
    fn dispatch_respects_global_concurrency() {
        let candidates = vec![
            create_test_issue("1", "1", Some(1)),
            create_test_issue("2", "2", Some(2)),
            create_test_issue("3", "3", Some(3)),
        ];

        // With 1 slot available
        let selected = select_candidates(&candidates, &HashMap::new(), &HashSet::new(), &HashMap::new(), 1);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].identifier, "1");
    }

    #[test]
    fn dispatch_skips_claimed() {
        let candidates = vec![
            create_test_issue("1", "1", Some(1)),
            create_test_issue("2", "2", Some(2)),
        ];

        let mut claimed = HashSet::new();
        claimed.insert("1".to_string());

        let selected = select_candidates(&candidates, &HashMap::new(), &claimed, &HashMap::new(), 10);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].identifier, "2");
    }

    #[test]
    fn dispatch_slot_exhaustion() {
        let candidates = vec![
            create_test_issue("1", "1", Some(1)),
            create_test_issue("2", "2", Some(2)),
        ];

        // With 0 slots available (max_concurrent already reached)
        let selected = select_candidates(&candidates, &HashMap::new(), &HashSet::new(), &HashMap::new(), 0);

        assert!(selected.is_empty());
    }

    #[test]
    fn dispatch_none_priority_sorts_last() {
        let mut candidates = vec![
            create_test_issue("1", "1", Some(1)),
            create_test_issue("2", "2", None),  // No priority
            create_test_issue("3", "3", Some(2)),
        ];

        // Set same creation times for simplicity
        let now = Utc::now();
        for c in &mut candidates {
            c.created_at = Some(now);
        }

        let selected = select_candidates(&candidates, &HashMap::new(), &HashSet::new(), &HashMap::new(), 10);

        assert_eq!(selected.len(), 3);
        // None priority should be last
        assert_eq!(selected[0].identifier, "1");
        assert_eq!(selected[1].identifier, "3");
        assert_eq!(selected[2].identifier, "2");
    }
}

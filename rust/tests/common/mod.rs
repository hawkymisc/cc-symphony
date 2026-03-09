//! Shared test helpers for integration tests.

use symphony::config::AppConfig;
use symphony::domain::Issue;

/// Create a default AppConfig with fast polling for tests.
pub fn make_app_config() -> AppConfig {
    let mut config = AppConfig::default();
    config.polling.interval_ms = 50;
    config
}

/// Create a default AppConfig with a custom concurrency limit and fast polling.
pub fn make_app_config_with_concurrency(max: usize) -> AppConfig {
    let mut config = make_app_config();
    config.agent.max_concurrent_agents = max;
    config
}

/// Create a test Issue with state set to "open".
pub fn make_open_issue(id: &str, identifier: &str) -> Issue {
    let mut issue = Issue::new(id, identifier, "Test issue");
    issue.state = "open".to_string();
    issue
}

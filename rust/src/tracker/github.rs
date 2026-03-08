//! GitHub Issues tracker implementation (SPEC §11)
//!
//! Uses GitHub GraphQL API v4 to fetch issues.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::{Tracker, TrackerError};
use crate::domain::Issue;

/// GitHub GraphQL API endpoint (used in tests and as the default value)
#[cfg(test)]
const GITHUB_GRAPHQL_ENDPOINT: &str = "https://api.github.com/graphql";

/// Default page size for GraphQL queries
const DEFAULT_PAGE_SIZE: usize = 50;

/// Maximum pages per fetch to prevent runaway queries
const MAX_PAGES: usize = 10;

/// Rate limit warning threshold
const RATE_LIMIT_WARNING_THRESHOLD: u32 = 100;

/// GitHub tracker configuration
#[derive(Clone)]
pub struct GitHubConfig {
    /// API endpoint (default: https://api.github.com/graphql)
    pub endpoint: String,
    /// Personal access token
    pub api_key: String,
    /// Repository in owner/repo format
    pub repo: String,
    /// Label filter (optional)
    pub labels: Vec<String>,
    /// Active states
    pub active_states: Vec<String>,
    /// Terminal states
    pub terminal_states: Vec<String>,
}

impl std::fmt::Debug for GitHubConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubConfig")
            .field("endpoint", &self.endpoint)
            .field("api_key", &"[REDACTED]")
            .field("repo", &self.repo)
            .field("labels", &self.labels)
            .field("active_states", &self.active_states)
            .field("terminal_states", &self.terminal_states)
            .finish()
    }
}

/// GitHub Issues tracker
pub struct GitHubTracker {
    client: Client,
    config: GitHubConfig,
    rate_limit: Arc<RwLock<RateLimitInfo>>,
}

#[derive(Debug, Clone, Default)]
struct RateLimitInfo {
    remaining: u32,
    #[allow(dead_code)]
    limit: u32,
    #[allow(dead_code)]
    reset_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// GraphQL response for repository issues query
#[derive(Debug, Deserialize)]
struct GraphQLResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQLError {
    message: String,
    #[serde(default)]
    #[allow(dead_code)]
    locations: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RepositoryData {
    repository: Option<Repository>,
}

#[derive(Debug, Deserialize)]
struct Repository {
    issues: IssuesConnection,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssuesConnection {
    nodes: Vec<GitHubIssue>,
    page_info: PageInfo,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GitHubIssue {
    id: String,
    number: i32,
    title: String,
    #[serde(default)]
    body: Option<String>,
    state: String,
    #[serde(default)]
    labels: Option<LabelConnection>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    url: String,
}

#[derive(Debug, Deserialize)]
struct LabelConnection {
    nodes: Vec<Label>,
}

#[derive(Debug, Deserialize)]
struct Label {
    name: String,
}

impl GitHubTracker {
    /// Create a new GitHub tracker
    pub fn new(config: GitHubConfig) -> Result<Self, TrackerError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| TrackerError::ApiRequest(e.to_string()))?;

        Ok(Self {
            client,
            config,
            rate_limit: Arc::new(RwLock::new(RateLimitInfo::default())),
        })
    }

    /// Execute a GraphQL query
    async fn query<T: for<'de> Deserialize<'de>>(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<T, TrackerError> {
        let body = serde_json::json!({
            "query": query,
            "variables": variables,
        });

        let response = self.client
            .post(&self.config.endpoint)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("User-Agent", "Symphony/0.1.0")
            .json(&body)
            .send()
            .await
            .map_err(|e| TrackerError::ApiRequest(e.to_string()))?;

        // Check rate limit headers
        if let Some(remaining) = response.headers().get("x-ratelimit-remaining") {
            if let Ok(s) = remaining.to_str() {
                if let Ok(remaining) = s.parse::<u32>() {
                    let mut rate_limit = self.rate_limit.write().await;
                    rate_limit.remaining = remaining;

                    if remaining < RATE_LIMIT_WARNING_THRESHOLD {
                        warn!("GitHub API rate limit low: {} remaining", remaining);
                    }
                }
            }
        }

        let status = response.status();
        if status.as_u16() == 403 {
            // Rate limited
            if let Some(reset_time) = response.headers().get("x-ratelimit-reset") {
                if let Ok(s) = reset_time.to_str() {
                    if let Ok(timestamp) = s.parse::<i64>() {
                        let now = chrono::Utc::now().timestamp();
                        let retry_after = (timestamp - now).max(0) as u64;
                        return Err(TrackerError::RateLimited {
                            retry_after_seconds: retry_after,
                        });
                    }
                }
            }
            return Err(TrackerError::RateLimited {
                retry_after_seconds: 60,
            });
        }

        if !status.is_success() {
            return Err(TrackerError::ApiStatus(status.as_u16()));
        }

        let graphql_response: GraphQLResponse<T> = response
            .json()
            .await
            .map_err(|e| TrackerError::ApiRequest(e.to_string()))?;

        if let Some(errors) = graphql_response.errors {
            let messages: Vec<String> = errors.iter().map(|e| e.message.clone()).collect();
            return Err(TrackerError::GraphqlErrors(messages.join(", ")));
        }

        graphql_response.data.ok_or(TrackerError::UnknownPayload)
    }

    /// Fetch issues with pagination
    async fn fetch_issues_paginated(&self, states: &[String], labels: &[String])
        -> Result<Vec<Issue>, TrackerError>
    {
        let (owner, repo) = self.parse_repo()?;
        let mut all_issues = Vec::new();
        let mut cursor: Option<String> = None;
        let mut pages = 0;

        // GitHub's IssueState enum requires UPPERCASE (OPEN, CLOSED).
        // Normalize here so config can use lowercase for readability.
        let states_upper: Vec<String> = states.iter().map(|s| s.to_uppercase()).collect();

        loop {
            pages += 1;
            if pages > MAX_PAGES {
                warn!("Reached maximum pages ({}) during fetch", MAX_PAGES);
                break;
            }

            let query = r#"
                query($owner: String!, $repo: String!, $states: [IssueState!], $labels: [String!], $first: Int!, $cursor: String) {
                    repository(owner: $owner, name: $repo) {
                        issues(
                            states: $states
                            labels: $labels
                            first: $first
                            after: $cursor
                            orderBy: {field: CREATED_AT, direction: ASC}
                        ) {
                            nodes {
                                id
                                number
                                title
                                body
                                state
                                labels(first: 20) {
                                    nodes { name }
                                }
                                createdAt
                                updatedAt
                                url
                            }
                            pageInfo {
                                hasNextPage
                                endCursor
                            }
                        }
                    }
                }
            "#;

            let variables = serde_json::json!({
                "owner": owner,
                "repo": repo,
                "states": &states_upper,
                "labels": if labels.is_empty() { serde_json::Value::Null } else { serde_json::json!(labels) },
                "first": DEFAULT_PAGE_SIZE,
                "cursor": cursor,
            });

            let data: RepositoryData = self.query(query, variables).await?;

            let repository = data.repository.ok_or(TrackerError::UnknownPayload)?;
            let issues_conn = repository.issues;

            // Convert GitHub issues to domain issues
            for gh_issue in issues_conn.nodes {
                all_issues.push(self.normalize_issue(gh_issue));
            }

            // Check pagination
            if !issues_conn.page_info.has_next_page {
                break;
            }

            cursor = issues_conn.page_info.end_cursor;
            if cursor.is_none() {
                debug!("No end cursor, stopping pagination");
                break;
            }
        }

        info!("Fetched {} issues from GitHub", all_issues.len());
        Ok(all_issues)
    }

    /// Parse owner/repo format
    fn parse_repo(&self) -> Result<(&str, &str), TrackerError> {
        let parts: Vec<&str> = self.config.repo.split('/').collect();
        if parts.len() != 2 {
            return Err(TrackerError::ApiRequest(format!(
                "Invalid repo format: {}. Expected owner/repo",
                self.config.repo
            )));
        }
        Ok((parts[0], parts[1]))
    }

    /// Normalize GitHub issue to domain Issue
    fn normalize_issue(&self, gh: GitHubIssue) -> Issue {
        let labels: Vec<String> = gh.labels
            .map(|l| l.nodes.into_iter().map(|n| n.name.to_lowercase()).collect())
            .unwrap_or_default();

        Issue {
            id: gh.id,
            identifier: gh.number.to_string(),
            title: gh.title,
            description: gh.body,
            priority: None, // GitHub Issues doesn't have native priority
            state: gh.state.to_lowercase(),
            branch_name: None,
            url: Some(gh.url),
            labels,
            blocked_by: Vec::new(), // Not supported in MVP
            created_at: Some(gh.created_at),
            updated_at: Some(gh.updated_at),
        }
    }
}

#[async_trait]
impl Tracker for GitHubTracker {
    async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError> {
        self.fetch_issues_paginated(
            &self.config.active_states,
            &self.config.labels,
        ).await
    }

    async fn fetch_issues_by_ids(&self, ids: &[String]) -> Result<Vec<Issue>, TrackerError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        // parse_repo validates the config format even though owner/repo aren't used in this query
        let _ = self.parse_repo()?;

        // GitHub GraphQL doesn't support fetching by node IDs directly for issues
        // We need to use the node query
        let query = r#"
            query($ids: [ID!]!) {
                nodes(ids: $ids) {
                    ... on Issue {
                        id
                        number
                        title
                        body
                        state
                        labels(first: 20) {
                            nodes { name }
                        }
                        createdAt
                        updatedAt
                        url
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "ids": ids,
        });

        #[derive(Debug, Deserialize)]
        struct NodesData {
            nodes: Vec<Option<GitHubIssue>>,
        }

        let data: NodesData = self.query(query, variables).await?;

        let issues: Vec<Issue> = data.nodes
            .into_iter()
            .flatten()
            .map(|gh| self.normalize_issue(gh))
            .collect();

        Ok(issues)
    }

    async fn fetch_issues_by_states(&self, states: &[String]) -> Result<Vec<Issue>, TrackerError> {
        if states.is_empty() {
            return Ok(Vec::new());
        }
        self.fetch_issues_paginated(states, &self.config.labels).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_repo() {
        let config = GitHubConfig {
            endpoint: GITHUB_GRAPHQL_ENDPOINT.to_string(),
            api_key: "test".to_string(),
            repo: "owner/repo".to_string(),
            labels: vec![],
            active_states: vec!["open".to_string()],
            terminal_states: vec!["closed".to_string()],
        };

        let tracker = GitHubTracker::new(config).unwrap();
        let (owner, repo) = tracker.parse_repo().unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn github_config_debug_masks_api_key() {
        let config = GitHubConfig {
            endpoint: GITHUB_GRAPHQL_ENDPOINT.to_string(),
            api_key: "ghp_super_secret_token_12345".to_string(),
            repo: "owner/repo".to_string(),
            labels: vec![],
            active_states: vec!["open".to_string()],
            terminal_states: vec!["closed".to_string()],
        };
        let debug_output = format!("{:?}", config);
        assert!(!debug_output.contains("ghp_super_secret_token_12345"), "API key should be masked in Debug output");
        assert!(debug_output.contains("[REDACTED]"), "Debug output should contain [REDACTED]");
    }

    #[test]
    fn test_parse_repo_invalid() {
        let config = GitHubConfig {
            endpoint: GITHUB_GRAPHQL_ENDPOINT.to_string(),
            api_key: "test".to_string(),
            repo: "invalid".to_string(),
            labels: vec![],
            active_states: vec!["open".to_string()],
            terminal_states: vec!["closed".to_string()],
        };

        let tracker = GitHubTracker::new(config).unwrap();
        assert!(tracker.parse_repo().is_err());
    }
}

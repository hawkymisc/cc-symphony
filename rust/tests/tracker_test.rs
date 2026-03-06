//! Phase 3: GitHub Tracker tests using wiremock (PLAN.md §Phase 3)

use wiremock::matchers::{body_json_schema, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use serde_json::json;

use symphony::tracker::{GitHubConfig, GitHubTracker, Tracker};

/// Helper to build a GitHubConfig pointing at the mock server
fn make_config(server_uri: &str, labels: Vec<String>) -> GitHubConfig {
    GitHubConfig {
        endpoint: format!("{}/graphql", server_uri),
        api_key: "test-token".to_string(),
        repo: "owner/repo".to_string(),
        labels,
        active_states: vec!["OPEN".to_string()],
        terminal_states: vec!["CLOSED".to_string()],
    }
}

/// A minimal valid GitHub Issues GraphQL response with one issue
fn single_issue_response() -> serde_json::Value {
    json!({
        "data": {
            "repository": {
                "issues": {
                    "nodes": [{
                        "id": "I_abc123",
                        "number": 42,
                        "title": "Fix the bug",
                        "body": "Description here",
                        "state": "OPEN",
                        "labels": {
                            "nodes": [{"name": "bug"}, {"name": "symphony"}]
                        },
                        "createdAt": "2026-01-01T00:00:00Z",
                        "updatedAt": "2026-01-02T00:00:00Z",
                        "url": "https://github.com/owner/repo/issues/42"
                    }],
                    "pageInfo": {
                        "hasNextPage": false,
                        "endCursor": null
                    }
                }
            }
        }
    })
}

/// An empty issues response
fn empty_issues_response() -> serde_json::Value {
    json!({
        "data": {
            "repository": {
                "issues": {
                    "nodes": [],
                    "pageInfo": {
                        "hasNextPage": false,
                        "endCursor": null
                    }
                }
            }
        }
    })
}

// ─── fetch_candidates_success ─────────────────────────────────────────────────

#[tokio::test]
async fn fetch_candidates_success() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(header("Authorization", "Bearer test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(single_issue_response()))
        .expect(1)
        .mount(&server)
        .await;

    let tracker = GitHubTracker::new(make_config(&server.uri(), vec![])).unwrap();
    let issues = tracker.fetch_candidate_issues().await.unwrap();

    assert_eq!(issues.len(), 1);
    let issue = &issues[0];
    assert_eq!(issue.id, "I_abc123");
    assert_eq!(issue.identifier, "42");
    assert_eq!(issue.title, "Fix the bug");
    assert_eq!(issue.description.as_deref(), Some("Description here"));
    assert_eq!(issue.state, "open"); // normalized to lowercase
    assert_eq!(issue.labels, vec!["bug", "symphony"]);
    assert_eq!(
        issue.url.as_deref(),
        Some("https://github.com/owner/repo/issues/42")
    );
}

// ─── fetch_candidates_empty ──────────────────────────────────────────────────

#[tokio::test]
async fn fetch_candidates_empty() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(empty_issues_response()))
        .mount(&server)
        .await;

    let tracker = GitHubTracker::new(make_config(&server.uri(), vec![])).unwrap();
    let issues = tracker.fetch_candidate_issues().await.unwrap();

    assert!(issues.is_empty());
}

// ─── fetch_candidates_pagination ─────────────────────────────────────────────

#[tokio::test]
async fn fetch_candidates_pagination() {
    let server = MockServer::start().await;

    let page1 = json!({
        "data": {
            "repository": {
                "issues": {
                    "nodes": [{
                        "id": "I_001",
                        "number": 1,
                        "title": "Issue 1",
                        "body": null,
                        "state": "OPEN",
                        "labels": {"nodes": []},
                        "createdAt": "2026-01-01T00:00:00Z",
                        "updatedAt": "2026-01-01T00:00:00Z",
                        "url": "https://github.com/owner/repo/issues/1"
                    }],
                    "pageInfo": {
                        "hasNextPage": true,
                        "endCursor": "cursor_abc"
                    }
                }
            }
        }
    });

    let page2 = json!({
        "data": {
            "repository": {
                "issues": {
                    "nodes": [{
                        "id": "I_002",
                        "number": 2,
                        "title": "Issue 2",
                        "body": null,
                        "state": "OPEN",
                        "labels": {"nodes": []},
                        "createdAt": "2026-01-02T00:00:00Z",
                        "updatedAt": "2026-01-02T00:00:00Z",
                        "url": "https://github.com/owner/repo/issues/2"
                    }],
                    "pageInfo": {
                        "hasNextPage": false,
                        "endCursor": null
                    }
                }
            }
        }
    });

    // Serve page1 first, then page2
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page1))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(page2))
        .mount(&server)
        .await;

    let tracker = GitHubTracker::new(make_config(&server.uri(), vec![])).unwrap();
    let issues = tracker.fetch_candidate_issues().await.unwrap();

    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0].identifier, "1");
    assert_eq!(issues[1].identifier, "2");
}

// ─── normalize_labels_lowercase ──────────────────────────────────────────────

#[tokio::test]
async fn normalize_labels_lowercase() {
    let server = MockServer::start().await;

    let response = json!({
        "data": {
            "repository": {
                "issues": {
                    "nodes": [{
                        "id": "I_xyz",
                        "number": 10,
                        "title": "Test",
                        "body": null,
                        "state": "OPEN",
                        "labels": {
                            "nodes": [
                                {"name": "BUG"},
                                {"name": "In-Progress"},
                                {"name": "SYMPHONY"}
                            ]
                        },
                        "createdAt": "2026-01-01T00:00:00Z",
                        "updatedAt": "2026-01-01T00:00:00Z",
                        "url": "https://github.com/owner/repo/issues/10"
                    }],
                    "pageInfo": {"hasNextPage": false, "endCursor": null}
                }
            }
        }
    });

    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .mount(&server)
        .await;

    let tracker = GitHubTracker::new(make_config(&server.uri(), vec![])).unwrap();
    let issues = tracker.fetch_candidate_issues().await.unwrap();

    assert_eq!(issues[0].labels, vec!["bug", "in-progress", "symphony"]);
}

// ─── error_auth_401 ──────────────────────────────────────────────────────────

#[tokio::test]
async fn error_auth_401() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
        .mount(&server)
        .await;

    let tracker = GitHubTracker::new(make_config(&server.uri(), vec![])).unwrap();
    let result = tracker.fetch_candidate_issues().await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    // Should be an ApiStatus error with code 401
    assert!(
        matches!(err, symphony::tracker::TrackerError::ApiStatus(401)),
        "Expected ApiStatus(401), got {:?}", err
    );
}

// ─── error_rate_limit_403 ────────────────────────────────────────────────────

#[tokio::test]
async fn error_rate_limit_403() {
    let server = MockServer::start().await;

    // GitHub returns 403 with rate limit headers when rate-limited
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(
            ResponseTemplate::new(403)
                .append_header("x-ratelimit-remaining", "0")
                .append_header("x-ratelimit-reset", "9999999999")
                .set_body_string("rate limit exceeded"),
        )
        .mount(&server)
        .await;

    let tracker = GitHubTracker::new(make_config(&server.uri(), vec![])).unwrap();
    let result = tracker.fetch_candidate_issues().await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, symphony::tracker::TrackerError::RateLimited { .. }),
        "Expected RateLimited, got {:?}", err
    );
}

// ─── error_graphql ───────────────────────────────────────────────────────────

#[tokio::test]
async fn error_graphql() {
    let server = MockServer::start().await;

    let response = json!({
        "data": null,
        "errors": [
            {"message": "Could not resolve to a Repository with the name 'owner/nonexistent'."}
        ]
    });

    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .mount(&server)
        .await;

    let tracker = GitHubTracker::new(make_config(&server.uri(), vec![])).unwrap();
    let result = tracker.fetch_candidate_issues().await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        matches!(err, symphony::tracker::TrackerError::GraphqlErrors(_)),
        "Expected GraphqlErrors, got {:?}", err
    );
    if let symphony::tracker::TrackerError::GraphqlErrors(msg) = err {
        assert!(msg.contains("Could not resolve"));
    }
}

// ─── error_network ───────────────────────────────────────────────────────────

#[tokio::test]
async fn error_network() {
    // Point at a port where nothing is listening
    let config = GitHubConfig {
        endpoint: "http://127.0.0.1:1".to_string(), // definitely not listening
        api_key: "test-token".to_string(),
        repo: "owner/repo".to_string(),
        labels: vec![],
        active_states: vec!["OPEN".to_string()],
        terminal_states: vec!["CLOSED".to_string()],
    };

    let tracker = GitHubTracker::new(config).unwrap();
    let result = tracker.fetch_candidate_issues().await;

    assert!(result.is_err());
    assert!(
        matches!(result.unwrap_err(), symphony::tracker::TrackerError::ApiRequest(_)),
        "Expected ApiRequest (network error)"
    );
}

// ─── fetch_states_by_ids_success ─────────────────────────────────────────────

#[tokio::test]
async fn fetch_states_by_ids_success() {
    let server = MockServer::start().await;

    let response = json!({
        "data": {
            "nodes": [
                {
                    "id": "I_abc123",
                    "number": 42,
                    "title": "Fix the bug",
                    "body": null,
                    "state": "CLOSED",
                    "labels": {"nodes": []},
                    "createdAt": "2026-01-01T00:00:00Z",
                    "updatedAt": "2026-01-03T00:00:00Z",
                    "url": "https://github.com/owner/repo/issues/42"
                }
            ]
        }
    });

    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .mount(&server)
        .await;

    let tracker = GitHubTracker::new(make_config(&server.uri(), vec![])).unwrap();
    let issues = tracker
        .fetch_issues_by_ids(&["I_abc123".to_string()])
        .await
        .unwrap();

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].id, "I_abc123");
    assert_eq!(issues[0].state, "closed"); // normalized
}

// ─── fetch_states_by_ids_empty_input ─────────────────────────────────────────

#[tokio::test]
async fn fetch_states_by_ids_empty_input() {
    let server = MockServer::start().await;
    // No mock registered — no HTTP call should be made
    let tracker = GitHubTracker::new(make_config(&server.uri(), vec![])).unwrap();

    let issues = tracker.fetch_issues_by_ids(&[]).await.unwrap();

    assert!(issues.is_empty());
    // wiremock will verify 0 requests were made (no expect() registered)
}

// ─── fetch_states_partial ────────────────────────────────────────────────────

#[tokio::test]
async fn fetch_states_partial() {
    let server = MockServer::start().await;

    // GitHub returns null for node IDs it can't find
    let response = json!({
        "data": {
            "nodes": [
                {
                    "id": "I_abc123",
                    "number": 42,
                    "title": "Found",
                    "body": null,
                    "state": "OPEN",
                    "labels": {"nodes": []},
                    "createdAt": "2026-01-01T00:00:00Z",
                    "updatedAt": "2026-01-01T00:00:00Z",
                    "url": "https://github.com/owner/repo/issues/42"
                },
                null  // ID not found
            ]
        }
    });

    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .mount(&server)
        .await;

    let tracker = GitHubTracker::new(make_config(&server.uri(), vec![])).unwrap();
    let issues = tracker
        .fetch_issues_by_ids(&["I_abc123".to_string(), "I_notfound".to_string()])
        .await
        .unwrap();

    // Only the found issue should be returned
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].id, "I_abc123");
}

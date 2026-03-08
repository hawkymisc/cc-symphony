# Symphony Rust Implementation Plan

CODEX_REVIEWED

## Overview

OpenAI Symphony の仕様を基に、以下の変更を加えた Rust 実装を構築する:

1. **Issue Tracker**: Linear -> **GitHub Issues** (MVP), GitHub Projects v2 (future extension)
2. **Coding Agent**: Codex app-server -> **Claude Code CLI**
3. **Implementation Language**: Elixir -> **Rust**

## Scope

### MVP (v0.1): GitHub Issues + Claude Code

- `tracker.kind: github` (GitHub Issues のみ)
- Claude Code CLI subprocess per turn
- 単一リポジトリ対象
- Linux only (hooks は `sh -lc`)
- HTTP server なし

### v0.2 Extensions (後回し)

- `tracker.kind: github-project` (GitHub Projects v2)
- HTTP server + dashboard
- macOS 対応 (hooks portability)
- `linear_graphql` 相当の `github_graphql` tool extension

## Architecture

```
rust/
├── Cargo.toml
├── src/
│   ├── main.rs                  # CLI entrypoint
│   ├── lib.rs                   # Library root
│   ├── config.rs                # Typed config layer (Section 6)
│   ├── workflow.rs              # WORKFLOW.md loader + YAML parser (Section 5)
│   ├── prompt.rs                # Liquid template rendering (Section 12)
│   ├── domain/
│   │   ├── mod.rs
│   │   ├── issue.rs             # Issue model (Section 4.1.1)
│   │   ├── run_attempt.rs       # Run attempt lifecycle (Section 4.1.5)
│   │   ├── session.rs           # Live session metadata (Section 4.1.6)
│   │   └── retry.rs             # Retry entry (Section 4.1.7)
│   ├── tracker/
│   │   ├── mod.rs               # Tracker trait (Section 11.1)
│   │   ├── github.rs            # GitHub Issues adapter (MVP)
│   │   └── memory.rs            # In-memory adapter for tests
│   ├── orchestrator/
│   │   ├── mod.rs               # Orchestrator state machine (Section 7)
│   │   ├── state.rs             # Runtime state (Section 4.1.8)
│   │   ├── dispatch.rs          # Dispatch logic (Section 8.2-8.3)
│   │   ├── reconcile.rs         # Reconciliation (Section 8.5)
│   │   └── retry.rs             # Retry queue + backoff (Section 8.4)
│   ├── workspace/
│   │   ├── mod.rs               # Workspace manager (Section 9)
│   │   └── hooks.rs             # Hook execution with timeout
│   ├── agent/
│   │   ├── mod.rs               # Agent runner trait (Section 10)
│   │   └── claude.rs            # Claude Code CLI integration
│   └── observability/
│       ├── mod.rs               # Logging setup
│       └── metrics.rs           # Token accounting + runtime metrics
├── tests/
│   ├── workflow_test.rs
│   ├── config_test.rs
│   ├── orchestrator_test.rs
│   ├── workspace_test.rs
│   ├── tracker_test.rs
│   └── integration_test.rs
└── WORKFLOW.md                  # Example workflow file
```

## SPEC.md Modification Plan

SPEC.md を直接書き換えるのではなく、`SPEC_GITHUB.md` として fork し、
オリジナルとの差分を明確にする。主な変更箇所:

### 1. Tracker: Linear -> GitHub Issues

| SPEC Section | Original | Modified |
|---|---|---|
| 5.3.1 `tracker.kind` | `linear` | `github` |
| 5.3.1 `tracker.endpoint` | `https://api.linear.app/graphql` | `https://api.github.com/graphql` |
| 5.3.1 `tracker.api_key` | `$LINEAR_API_KEY` | `$GITHUB_TOKEN` |
| 5.3.1 `tracker.project_slug` | Linear slugId | **Removed**; replaced by `tracker.repo` |
| 5.3.1 (new) `tracker.repo` | N/A | `owner/repo` format, required |
| 5.3.1 (new) `tracker.labels` | N/A | Optional label filter list |
| 5.3.1 `active_states` | `Todo, In Progress` | `open` |
| 5.3.1 `terminal_states` | `Closed, Cancelled, ...` | `closed` |
| 11.2 Query Semantics | Linear GraphQL | GitHub GraphQL v4 |
| 11.3 Normalization | `slugId`, relations | Issue `number` as identifier, no `blocked_by` in MVP |

### 2. Agent: Codex -> Claude Code

| SPEC Section | Original | Modified |
|---|---|---|
| 5.3.6 key name | `codex` | `claude` |
| 5.3.6 `command` | `codex app-server` | `claude` |
| 5.3.6 (new) `model` | N/A | Model ID (default: `claude-sonnet-4-20250514`) |
| 5.3.6 (new) `skip_permissions` | N/A | `bool`, default `false` |
| 5.3.6 `approval_policy` | Codex AskForApproval | **Removed**; replaced by `skip_permissions` + `allowed_tools` |
| 5.3.6 `thread_sandbox` / `turn_sandbox_policy` | Codex sandbox | **Removed**; Claude Code doesn't have sandbox modes |
| 10.1 Launch | `bash -lc <codex.command>` | `claude --print --output-format stream-json -p <prompt>` |
| 10.2 Handshake | JSON-RPC initialize/thread/turn | **Removed**; no handshake, direct CLI invocation |
| 10.3 Streaming | JSON-RPC line-delimited | Claude Code stream-json events (newline-delimited) |
| 10.4 Events | Codex-specific event types | Claude Code event types (see below) |
| 10.5 Approval | Codex approval protocol | `--dangerously-skip-permissions` or `--allowedTools` |

### 3. Fields Removed (Linear-specific)

- `tracker.project_slug` (Linear only)
- `codex.approval_policy`, `codex.thread_sandbox`, `codex.turn_sandbox_policy`
- `linear_graphql` client-side tool extension -> `github_graphql` (future)
- `blocked_by` relation normalization (GitHub doesn't have native blocker relations)

### 4. Fields Added (GitHub-specific)

- `tracker.repo` (string, `owner/repo`, required)
- `tracker.labels` (list of strings, optional filter)
- `claude.model` (string, model ID)
- `claude.skip_permissions` (bool)
- `claude.allowed_tools` (list of strings, optional)
- `claude.max_turns_per_invocation` (integer, default 50; Claude Code internal turn limit)

## Security & Secrets Management

### Authentication

| Secret | Source | Validation |
|---|---|---|
| `GITHUB_TOKEN` | env var or `$VAR` in YAML | Validate non-empty at startup; never log |
| `ANTHROPIC_API_KEY` | env var (used by Claude Code internally) | Validate Claude Code can start; Symphony doesn't read this directly |

### GitHub Token Requirements

- Scope: `repo` (full access to private repos) or `public_repo` (public only)
- For Projects v2: additional `project` scope
- Rate limit: 5,000 req/hour (authenticated), tracked via `X-RateLimit-*` headers

### Token Refresh Strategy

MVP: static token from env var. No refresh needed for personal access tokens.
Future: GitHub App installation tokens with auto-refresh (expires every hour).

### Safety

- `--dangerously-skip-permissions` must be explicitly opted in via config
- Document the security implications prominently in README
- Workspace path containment is enforced (Section 9.5)
- Hook scripts are trusted config (same as original spec)

## GitHub API Integration Detail

### HTTP Client Configuration

```rust
struct GitHubClient {
    http: reqwest::Client,        // with default headers
    token: String,                // from config, never logged
    endpoint: String,             // default: https://api.github.com/graphql
    rate_limit_remaining: AtomicU32,
    rate_limit_reset: AtomicU64,  // unix timestamp
}
```

### Rate Limit Handling

1. Parse `X-RateLimit-Remaining` and `X-RateLimit-Reset` from every response
2. If `remaining < 100`, log warning
3. If `remaining == 0`, sleep until `reset` timestamp (with jitter)
4. On 403 with `rate limit exceeded`, apply exponential backoff: `min(1s * 2^attempt, 60s)`
5. On 5xx or network error, retry up to 3 times with backoff: `1s, 2s, 4s`

### Pagination Strategy

- GitHub GraphQL: use `pageInfo.hasNextPage` + `endCursor`
- Default page size: 50 (same as Linear spec)
- Maximum total pages per fetch: 10 (500 issues cap, prevents runaway)

### GraphQL Queries

**Fetch candidate issues:**
```graphql
query($owner: String!, $repo: String!, $states: [IssueState!], $labels: [String!], $cursor: String) {
  repository(owner: $owner, name: $repo) {
    issues(
      states: $states
      labels: $labels
      first: 50
      after: $cursor
      orderBy: {field: CREATED_AT, direction: ASC}
    ) {
      nodes {
        id
        number
        title
        body
        state
        labels(first: 20) { nodes { name } }
        createdAt
        updatedAt
        url
      }
      pageInfo { hasNextPage endCursor }
    }
  }
}
```

**Fetch issue states by IDs (reconciliation):**
```graphql
query($ids: [ID!]!) {
  nodes(ids: $ids) {
    ... on Issue {
      id
      number
      state
    }
  }
}
```

**Fetch closed issues (startup cleanup):**
```graphql
query($owner: String!, $repo: String!, $cursor: String) {
  repository(owner: $owner, name: $repo) {
    issues(states: [CLOSED], first: 50, after: $cursor) {
      nodes { id number }
      pageInfo { hasNextPage endCursor }
    }
  }
}
```

### Issue Normalization: GitHub -> Domain Model

```rust
fn normalize_github_issue(gh: &GitHubIssue) -> Issue {
    Issue {
        id: gh.id.clone(),                              // GraphQL node ID
        identifier: gh.number.to_string(),              // "42"
        title: gh.title.clone(),
        description: gh.body.clone(),                   // Option<String>
        priority: None,                                 // GitHub Issues have no priority; use labels
        state: gh.state.to_lowercase(),                 // "open" or "closed"
        branch_name: None,                              // not available from issue
        url: Some(gh.url.clone()),
        labels: gh.labels.iter().map(|l| l.to_lowercase()).collect(),
        blocked_by: vec![],                             // not supported in MVP
        created_at: Some(gh.created_at),
        updated_at: gh.updated_at,
    }
}
```

## Claude Code Agent Integration Detail

### CLI Invocation per Turn

```rust
async fn run_turn(
    workspace: &Path,
    prompt: &str,
    config: &ClaudeConfig,
) -> Result<TurnResult, AgentError> {
    let mut cmd = tokio::process::Command::new(&config.command);

    cmd.arg("--print")
       .arg("--output-format").arg("stream-json")
       .arg("--model").arg(&config.model)
       .arg("--max-turns").arg(config.max_turns_per_invocation.to_string())
       .arg("-p").arg(prompt)
       .current_dir(workspace)
       .stdout(Stdio::piped())
       .stderr(Stdio::piped())
       .kill_on_drop(true);  // cleanup on cancel

    if config.skip_permissions {
        cmd.arg("--dangerously-skip-permissions");
    }

    if let Some(ref tools) = config.allowed_tools {
        cmd.arg("--allowedTools").arg(tools.join(","));
    }

    let mut child = cmd.spawn().map_err(AgentError::SpawnFailed)?;
    // ... stream stdout, enforce timeouts, collect result
}
```

### Claude Code stream-json Event Types

```rust
#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClaudeEvent {
    #[serde(rename = "assistant")]
    Assistant { message: AssistantMessage },
    #[serde(rename = "tool_use")]
    ToolUse { tool: String, input: serde_json::Value },
    #[serde(rename = "tool_result")]
    ToolResult { tool: String, output: String },
    #[serde(rename = "result")]
    Result { result: String, usage: Option<Usage> },
    #[serde(rename = "error")]
    Error { error: ErrorDetail },
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: u64,
    output_tokens: u64,
    // cache_creation_input_tokens, cache_read_input_tokens may also appear
}
```

### Version Compatibility

- Test against Claude Code CLI version via `claude --version`
- Log the version at startup for diagnostics
- If `stream-json` output format is unavailable, fall back to `json` (non-streaming)
- If `--max-turns` is unavailable, omit it (Claude Code may not limit turns)

## Observability Data Model

### Token Accounting

```rust
struct TokenTotals {
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,           // input + output
    cache_read_tokens: u64,      // Claude-specific
    cache_creation_tokens: u64,  // Claude-specific
    seconds_running: f64,        // aggregate wall-clock
}
```

### Per-Session Metrics

```rust
struct SessionMetrics {
    issue_id: String,
    issue_identifier: String,
    session_id: String,           // "<issue_id>-<turn_number>"
    turn_count: u32,
    started_at: DateTime<Utc>,
    last_event_at: Option<DateTime<Utc>>,
    last_event_type: Option<String>,
    last_event_summary: Option<String>,  // truncated to 200 chars
    tokens: TokenTotals,
}
```

### Runtime Snapshot (for future API)

```rust
struct RuntimeSnapshot {
    generated_at: DateTime<Utc>,
    running: Vec<RunningEntry>,
    retrying: Vec<RetryEntry>,
    codex_totals: TokenTotals,   // "codex" name kept for spec compat
    rate_limits: Option<RateLimitInfo>,
}

struct RateLimitInfo {
    remaining: u32,
    limit: u32,
    reset_at: DateTime<Utc>,
    source: String,              // "github" or "anthropic"
}
```

## Workspace Hook Timeout Specification

| Hook | Default Timeout | On Failure | On Timeout |
|---|---|---|---|
| `after_create` | `hooks.timeout_ms` (60s) | Fatal: abort workspace creation, remove dir | Fatal: kill process, abort, remove dir |
| `before_run` | `hooks.timeout_ms` (60s) | Fatal: abort current attempt | Fatal: kill process, abort attempt |
| `after_run` | `hooks.timeout_ms` (60s) | Log warning, continue | Kill process, log warning, continue |
| `before_remove` | `hooks.timeout_ms` (60s) | Log warning, proceed with removal | Kill process, log warning, proceed |

Implementation: `tokio::time::timeout` wrapping `tokio::process::Command`.

## CLI UX and Error Reporting

### CLI Interface

```
symphony-rs [OPTIONS] [WORKFLOW_PATH]

Arguments:
  [WORKFLOW_PATH]  Path to WORKFLOW.md [default: ./WORKFLOW.md]

Options:
  -p, --port <PORT>  Enable HTTP server on this port
  -v, --verbose       Increase log verbosity (repeat for more)
  -q, --quiet         Suppress non-error output
      --dry-run       Validate config and exit without starting
  -h, --help          Print help
  -V, --version       Print version
```

### Exit Codes

| Code | Meaning |
|---|---|
| 0 | Normal shutdown (SIGTERM/SIGINT) |
| 1 | Startup validation failure |
| 2 | CLI argument error |
| 3 | Workflow file error (missing, invalid) |

### Startup Output

```
[INFO] symphony v0.1.0 starting
[INFO] workflow: ./WORKFLOW.md (last modified: 2026-03-05T10:00:00Z)
[INFO] tracker: github (owner/repo)
[INFO] agent: claude (claude-sonnet-4-20250514)
[INFO] workspace root: /home/user/symphony-workspaces
[INFO] concurrency: max 5 agents
[INFO] polling every 30s
[INFO] startup cleanup: removed 2 terminal workspaces
[INFO] first poll in 0ms
```

### Error Output Examples

```
[ERROR] startup failed: GITHUB_TOKEN is not set (tracker.api_key resolves to empty)
[ERROR] workflow parse error: WORKFLOW.md:3: expected map for front matter, got sequence
[WARN]  github api: rate limit low (remaining=87/5000, resets in 342s)
[ERROR] issue #42: agent turn failed after 3600s (timeout), scheduling retry #2 in 20s
[WARN]  issue #42: stall detected (no activity for 300s), killing agent process
```

## Feature Flags (Cargo features)

```toml
[features]
default = ["github-issues"]
github-issues = []              # MVP tracker
github-projects = []            # v0.2: Projects v2 support
http-server = ["dep:axum", "dep:tower"]  # Optional HTTP server
```

This keeps the binary small for MVP and allows opt-in for extensions.

## Dependencies (Revised)

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"
liquid = "0.26"
clap = { version = "4", features = ["derive"] }
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["json", "env-filter"] }
notify = "7"
chrono = { version = "0.4", features = ["serde"] }
thiserror = "2"

# Optional (http-server feature)
axum = { version = "0.8", optional = true }
tower = { version = "0.5", optional = true }

[dev-dependencies]
tempfile = "3"
wiremock = "0.6"
tokio-test = "0.4"
assert_cmd = "2"
predicates = "3"
```

## WORKFLOW.md Example (GitHub Issues + Claude Code)

```yaml
---
tracker:
  kind: github
  repo: "hiroki-wakamatsu/my-project"
  api_key: $GITHUB_TOKEN
  active_states:
    - open
  terminal_states:
    - closed
  labels:
    - symphony
polling:
  interval_ms: 30000
workspace:
  root: ~/symphony-workspaces
hooks:
  after_create: |
    git clone --depth 1 "https://github.com/{{ repo }}" .
  before_run: |
    git fetch origin main && git rebase origin/main || true
agent:
  max_concurrent_agents: 5
  max_turns: 10
claude:
  command: claude
  model: claude-sonnet-4-20250514
  skip_permissions: true
  max_turns_per_invocation: 50
  turn_timeout_ms: 3600000
  read_timeout_ms: 5000
  stall_timeout_ms: 300000
---

You are working on GitHub Issue #{{ issue.identifier }}

{% if attempt %}
This is continuation attempt #{{ attempt }}. Resume from current workspace state.
{% endif %}

Issue: #{{ issue.identifier }} - {{ issue.title }}
State: {{ issue.state }}
Labels: {{ issue.labels }}
URL: {{ issue.url }}

Description:
{% if issue.description %}
{{ issue.description }}
{% else %}
No description provided.
{% endif %}

Instructions:
1. This is an unattended session. Do not ask for human input.
2. Work only in the provided workspace directory.
3. Create a feature branch from main, implement the changes, and push.
4. Create a Pull Request using `gh pr create`.
5. If blocked, add a comment to the issue explaining the blocker.
6. Only stop if you encounter a true blocker (missing auth, permissions, secrets).
```

## Implementation Phases (TDD, Revised)

Each phase follows RED -> GREEN -> REFACTOR.

### Phase 1: Foundation (pure logic, no I/O)

**Goal**: Domain models, workflow parsing, config, prompt rendering.

**Test scenarios**:
1. `workflow_parse_valid`: Parse YAML front matter + prompt body
2. `workflow_parse_no_front_matter`: Entire file is prompt
3. `workflow_parse_missing_file`: Returns `missing_workflow_file` error
4. `workflow_parse_invalid_yaml`: Returns `workflow_parse_error`
5. `workflow_parse_non_map`: Returns `workflow_front_matter_not_a_map`
6. `config_defaults`: All fields have correct defaults when YAML is empty
7. `config_env_resolution`: `$GITHUB_TOKEN` resolves from env
8. `config_env_empty`: `$VAR` resolving to empty = missing
9. `config_path_expansion`: `~` expands to home dir
10. `prompt_render_basic`: Issue fields interpolated correctly
11. `prompt_render_attempt`: `attempt` variable available
12. `prompt_render_strict`: Unknown variable fails
13. `prompt_render_empty_body`: Falls back to default prompt
14. `issue_identifier_sanitize`: `ABC-123` -> `ABC-123`, `foo/bar` -> `foo_bar`

### Phase 2: Workspace Manager (filesystem I/O)

**Test scenarios**:
1. `workspace_create_new`: Creates dir, `created_now=true`
2. `workspace_reuse_existing`: Existing dir, `created_now=false`
3. `workspace_path_deterministic`: Same identifier = same path
4. `workspace_path_sanitized`: Special chars replaced with `_`
5. `workspace_path_containment`: Rejects `../` escape attempts
6. `hook_after_create_runs`: Runs only on new workspace
7. `hook_after_create_failure_removes_dir`: Failed hook cleans up
8. `hook_before_run_failure_aborts`: Failure aborts attempt
9. `hook_timeout_kills_process`: Exceeding timeout kills child
10. `hook_after_run_failure_ignored`: Logged but not fatal

### Phase 3: GitHub Tracker (HTTP, mocked)

**Test scenarios** (using `wiremock`):
1. `fetch_candidates_success`: Returns normalized issues
2. `fetch_candidates_pagination`: Follows endCursor across pages
3. `fetch_candidates_empty`: No matching issues returns empty vec
4. `fetch_candidates_label_filter`: Only issues with matching labels
5. `fetch_states_by_ids`: Returns state for each ID
6. `fetch_states_partial`: Some IDs not found, handled gracefully
7. `fetch_terminal_issues`: Returns closed issues for cleanup
8. `normalize_labels_lowercase`: Labels normalized
9. `error_auth_401`: Returns typed auth error
10. `error_rate_limit_403`: Respects rate limit, schedules retry
11. `error_network`: Returns typed network error
12. `error_graphql`: Returns typed GraphQL error

### Phase 4: Orchestrator (async, state machine)

**Test scenarios** (using `MemoryTracker` + mock agent):
1. `dispatch_priority_sort`: Lower priority number first, then oldest
2. `dispatch_respects_global_concurrency`: Won't exceed max
3. `dispatch_respects_per_state_concurrency`: State-specific limits
4. `dispatch_skips_claimed`: Already running/retrying issues skipped
5. `dispatch_blocks_on_config_error`: Invalid config skips dispatch
6. `reconcile_terminal_stops_and_cleans`: Terminal state -> stop + cleanup
7. `reconcile_active_updates_state`: Active state -> update snapshot
8. `reconcile_non_active_stops_no_cleanup`: Non-active -> stop only
9. `reconcile_stall_detection`: No events for stall_timeout -> kill + retry
10. `retry_normal_exit_continuation`: Normal exit -> 1s retry
11. `retry_abnormal_exit_backoff`: Error -> 10s * 2^(n-1) backoff
12. `retry_backoff_cap`: Capped at max_retry_backoff_ms
13. `retry_issue_gone_releases_claim`: Issue not in candidates -> release
14. `retry_no_slots_requeues`: Full slots -> requeue with error msg
15. `startup_cleanup_removes_terminal_workspaces`: On boot
16. `config_reload_updates_interval`: WORKFLOW.md change -> new interval

### Phase 5: Claude Code Agent Runner (subprocess)

**Test scenarios** (using mock `claude` script):
1. `launch_correct_args`: Verify CLI args and cwd
2. `parse_stream_events`: Parse assistant/tool_use/result events
3. `extract_usage_tokens`: Token counts from result event
4. `turn_timeout_kills`: Exceeding turn_timeout kills process
5. `process_cleanup_on_cancel`: Cancellation kills child process
6. `handle_error_event`: Error event -> AgentError
7. `handle_unexpected_exit`: Exit code != 0 -> failure
8. `skip_permissions_flag`: Flag added when configured

### Phase 6: Observability

**Test scenarios**:
1. `log_includes_issue_context`: issue_id + identifier in span
2. `log_includes_session_context`: session_id in span
3. `token_aggregation_across_sessions`: Totals accumulate correctly
4. `token_no_double_count`: Absolute totals tracked via deltas
5. `runtime_seconds_includes_active`: Snapshot includes live sessions
6. `rate_limit_tracking`: Latest rate limit info preserved

<!-- CODEX_REVIEWED: phases 7-9 completion + open risks documented 2026-03-08 -->

### Phase 7: HTTP Server (feature-gated, deferred) ✅

**Shipped**: PR #7 (2026-03-07)

Feature-gated (`--features http-server`) axum 0.8 server bound to 127.0.0.1 only.
Routes: `GET /` (HTML dashboard), `GET /api/status` (JSON RuntimeSnapshot), `POST /api/refresh`.

**Verification artifacts**: `rust/tests/http_server_test.rs` — 15 tests including
503-on-timeout (`start_paused=true` + `tokio::time::advance`), XSS assertion
(no `.innerHTML =` for dynamic data), loopback-only bind check.

**Dependencies / open risks**:
- `http-server` feature flag must be passed explicitly (`cargo build --features http-server`);
  default build omits axum/tower-http entirely.
- No authentication on the dashboard: do not expose the port beyond loopback.
  Future: add bearer-token or local-socket option if remote access is needed.
- `--port` flag silently warns (does not error) when feature is disabled; user
  may be confused. Consider hard-erroring in a future release.

**Pending tasks**:
- [ ] Consider hard-error (exit 1) when `--port` is used without `http-server` feature.
- [ ] Add auth option (bearer token / unix socket) before any non-loopback deployment.

### Phase 8: CLI + Integration ✅

**Shipped**: PR #10 (2026-03-08)

**Test scenarios (all passing)**:
1. `cli_default_workflow_path_searches_cwd` — no WORKFLOW.md → exit 3
2. `cli_default_workflow_path_uses_cwd_workflow` — valid WORKFLOW.md in CWD + `--dry-run` → exit 0
3. `cli_explicit_path_uses_provided_file` — custom path + `--dry-run` → exit 0
4. `cli_explicit_path_missing_exits_3` — non-existent path → exit 3
5. `cli_dry_run_validates_and_exits` — prints "Config validated successfully"
6. `cli_dry_run_shows_config_summary` — prints repo + "Max concurrent agents"
7. `cli_dry_run_invalid_config_exits_1` — bad repo format → exit 1
8. `cli_graceful_shutdown_on_sigterm` (unix only) — hanging TCP server → SIGTERM → exit 0

**Exit code contract**:
| Code | Meaning |
|------|---------|
| 0 | Normal shutdown (SIGTERM/SIGINT) or `--dry-run` success |
| 1 | Startup validation failure (config error) |
| 2 | CLI argument error (handled by clap automatically) |
| 3 | Workflow file error (missing / unreadable / invalid YAML) |

**Integration tests (4, in `rust/tests/integration_test.rs`)**:
- `integration_full_cycle_dispatch_and_completion`
- `integration_snapshot_shows_running_while_agent_active`
- `integration_full_cycle_multiple_issues_dispatched`
- `integration_closed_issue_never_dispatched`

**Dependencies / open risks**:
- `cli_graceful_shutdown_on_sigterm` uses a loopback TCP server that accepts but
  never responds; the test is `#[cfg(unix)]` only — Windows ctrl_c path untested.
- Integration tests use `MemoryTracker` (in-process); real GitHub API is not
  exercised. Add a smoke-test against staging GitHub repo before production deploy.
- `assert_cmd` rebuilds the binary per test run; first run is slow on cold cache.

**Pending tasks**:
- [ ] Add Windows graceful-shutdown test using `GenerateConsoleCtrlEvent` once
      Windows CI runner is available.
- [ ] Smoke-test against a real GitHub repo (staging) as a separate CI gate.

### Phase 9: Orchestrator Refactoring ✅

**Shipped**: PR #11 (2026-03-08)

Extracted `Orchestrator::cancel_all_agents(state: &OrchestratorState)` static helper
to deduplicate the cancel-all-running-agents pattern that appeared 4 times in the
event loop (interval tick, Tick message, RetryIssue, RefreshRequest).
Behaviour and cancel safety are unchanged; all 145 tests continue to pass.

**Pending tasks**:
- [ ] Consider further deduplication of the full `tokio::select! { biased; cancel => ... }`
      wrapper via a macro or helper if a fifth call site appears.

### Phase 10: GitHub Projects V2 Tracker Adapter 🔲

<!-- CODEX_REVIEWED -->
> **Status**: Specification complete (SPEC_GITHUB.md §19). Not yet implemented.

**Goal**: Add `tracker.kind: github-project` so that Symphony can treat a GitHub
Project's custom Status field as the source of truth for issue eligibility, rather
than the issue's own `OPEN`/`CLOSED` state.

**Why this matters**: Teams that organise work in GitHub Projects often use the
Status field ("Todo", "In Progress", "Done") rather than closing issues to mark
completion. The existing `github` adapter cannot represent this workflow.

#### 10.1 Prerequisites

- Familiarity with SPEC_GITHUB.md §19 (read it first).
- GitHub PAT with `read:project` and `repo` scopes.
- A test GitHub Project (v2) with at least one issue in an active status.

#### 10.2 Implementation Tasks (TDD order)

| ID | Task | Notes |
|---|---|---|
| P10-001 | `ProjectTrackerConfig` struct | Extend `AppConfig` with `owner`, `owner_type`, `project_number`, `status_field_name`, `active_statuses`, `terminal_statuses`, `on_completion_set_status` |
| P10-002 | `ProjectMeta` cache struct | `project_node_id`, `status_field_id`, `status_options: HashMap<String, String>` |
| P10-003 | Field discovery query | `organization(login).projectV2(number).fields` → populate `ProjectMeta` |
| P10-004 | Item poll query | Paginated `node(id).items` with `fieldValueByName(name: "Status")` and `content { ... on Issue }` |
| P10-005 | Client-side filter | Filter items: Issue OPEN + status in active_statuses + optional label filter |
| P10-006 | Issue model mapping | Map ProjectV2 item to `Issue`; store `project_item_id` side-map |
| P10-007 | `fetch_issues_by_ids` reconciliation | Paginate all items, filter by issue node ID client-side |
| P10-008 | `GitHubProjectTracker` implements `Tracker` trait | Wire P10-003 through P10-007 |
| P10-009 | `on_completion_set_status` mutation | `updateProjectV2ItemFieldValue` after successful agent run (optional config) |
| P10-010 | Rate limit guard | Check `X-RateLimit-Remaining`; skip poll cycle if below threshold |
| P10-011 | Wiremock integration tests | Field discovery, poll (single page + pagination), reconciliation, rate limit backoff |
| P10-012 | CLI `--dry-run` output | Print `project_number`, `active_statuses`, `terminal_statuses` |
| P10-013 | `tracker.kind` dispatch | Config validation routes to `GitHubTracker` or `GitHubProjectTracker` |
| P10-014 | WORKFLOW.md example | Document `github-project` config in README |

#### 10.3 Key Design Decisions

**Dual `id` problem**: A ProjectV2Item has its own node ID (`ProjectV2Item.id`),
separate from the underlying `Issue.id`. Symphony's `Issue.id` must be the **issue
node ID** (used for agent workspace naming and reconciliation). The `project_item_id`
must be stored separately for status-update mutations.

```rust
// Side-map stored in GitHubProjectTracker (not in Issue model)
item_id_map: RwLock<HashMap<String, String>>,  // issue_node_id → project_item_id
```

**No server-side filter**: All items are fetched and filtered client-side. For large
projects this is O(n) per poll. The delta-optimisation in P10-010 (skip items whose
`updatedAt` did not change) brings amortised cost down significantly.

**`is_active()` semantics**: The `github-project` adapter synthesises the `Issue.state`
field from the project Status — it is `OPEN` only when Status is in `active_statuses`.
This keeps the orchestrator's `issue.is_active()` call unchanged.

#### 10.4 Test Scenarios

```
project_tracker_discovers_fields            — fields query returns status field + options
project_tracker_polls_active_items          — items with active status → returned as Issues
project_tracker_skips_terminal_items        — items with terminal status → excluded
project_tracker_skips_closed_issues         — CLOSED issues with active status → excluded
project_tracker_skips_draft_issues          — DraftIssue content → excluded
project_tracker_paginates_items             — 150 items → two pages of 100+50
project_tracker_reconciles_by_issue_id      — fetch_issues_by_ids filters by issue node ID
project_tracker_rate_limit_backoff          — remaining=0 → skip poll, log warning
project_tracker_status_update_on_completion — updateProjectV2ItemFieldValue called after Ok(())
project_tracker_unknown_status_excluded     — status not in active/terminal → excluded (warn)
```

#### 10.5 Open Questions / Risks

- **Large projects**: Projects with > 500 items will hit rate limits at 30s poll.
  Consider a configurable `max_items_per_poll` to fail-fast during development.
- **`user` vs `organization` owner type**: The GraphQL query root differs
  (`organization(login:)` vs `user(login:)`). Needs runtime branching.
- **Draft issues**: `ProjectV2Item.content` can be a `DraftIssue`, which has no
  `id` node ID. Must be skipped explicitly.
- **Project not shared with repo**: A Project can contain issues from multiple
  repos. The `repo` config field is still needed for posting comments / workspace
  paths but may not match all project items.

## Risk Mitigation (Revised)

| Risk | Impact | Mitigation |
|---|---|---|
| Claude Code CLI changes | Agent runner breaks | Pin version, test in CI, abstract behind trait |
| GitHub rate limits (5k/hr) | Polling stalls | Track remaining, backoff, batch reconciliation |
| No persistent session | Higher latency per turn | Workspace state persists; acceptable tradeoff |
| `--dangerously-skip-permissions` | Security risk | Explicit opt-in, document prominently, recommend allowlist |
| WORKFLOW.md TOCTOU on reload | Stale config | Re-read before each dispatch (defensive reload) |
| Large repos in workspace | Slow clone/fetch | `--depth 1` in hooks, document best practices |

## Migration Path for SPEC.md

1. Create `SPEC_GITHUB.md` as a fork of `SPEC.md`
2. Apply all tracker/agent changes listed above
3. Keep section numbering aligned with original for traceability
4. Add "Differences from Original SPEC.md" section at top
5. Mark GitHub-specific additions with `[GitHub]` prefix
6. Mark Claude-specific additions with `[Claude]` prefix
7. Future: if upstream SPEC.md evolves, diff and merge changes

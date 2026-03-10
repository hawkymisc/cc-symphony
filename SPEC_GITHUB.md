# Symphony Service Specification (GitHub + Claude Code Variant)

Status: Draft v1 (language-agnostic)

Purpose: Define a service that orchestrates coding agents to get project work done.

## Differences from Original SPEC.md

This document is a fork of the original Symphony SPEC.md with the following changes:

| Area | Original | This Variant |
|---|---|---|
| Issue Tracker | Linear (GraphQL) | **GitHub Issues** (GraphQL v4 / REST) |
| Tracker Auth | `LINEAR_API_KEY` | **`GITHUB_TOKEN`** |
| Project Filter | `tracker.project_slug` (Linear slugId) | **`tracker.repo`** (`owner/repo`) |
| Active States | `Todo`, `In Progress` | **`open`** |
| Terminal States | `Closed`, `Cancelled`, etc. | **`closed`** |
| Coding Agent | Codex app-server (JSON-RPC over stdio) | **Claude Code CLI** (subprocess per turn) |
| Agent Config Key | `codex` | **`claude`** |
| Session Protocol | JSON-RPC handshake + persistent process | **CLI invocation per turn, stream-json output** |
| Blocker Relations | Linear `blocks` inverse relations | **Not available in GitHub Issues MVP** |
| Client-side Tool | `linear_graphql` | **`github_graphql` (future extension)** |

Sections that are unchanged from the original are marked with `[Unchanged]`. Sections with
modifications are marked with `[GitHub]` or `[Claude]` to indicate the source of the change.

## 1. Problem Statement

[Unchanged from original except for the tracker name.]

Symphony is a long-running automation service that continuously reads work from an issue tracker
(GitHub Issues in this specification variant), creates an isolated workspace for each issue, and
runs a coding agent session for that issue inside the workspace.

The service solves four operational problems:

- It turns issue execution into a repeatable daemon workflow instead of manual scripts.
- It isolates agent execution in per-issue workspaces so agent commands run only inside per-issue
  workspace directories.
- It keeps the workflow policy in-repo (`WORKFLOW.md`) so teams version the agent prompt and runtime
  settings with their code.
- It provides enough observability to operate and debug multiple concurrent agent runs.

Implementations are expected to document their trust and safety posture explicitly. This
specification does not require a single approval, sandbox, or operator-confirmation policy; some
implementations may target trusted environments with a high-trust configuration, while others may
require stricter approvals or sandboxing.

Important boundary:

- Symphony is a scheduler/runner and tracker reader.
- Ticket writes (state transitions, comments, PR links) are typically performed by the coding agent
  using tools available in the workflow/runtime environment (for example `gh` CLI).
- A successful run may end at a workflow-defined handoff state (for example a specific label applied
  or a PR created), not necessarily issue closure.

## 2. Goals and Non-Goals

[Unchanged]

### 2.1 Goals

- Poll the issue tracker on a fixed cadence and dispatch work with bounded concurrency.
- Maintain a single authoritative orchestrator state for dispatch, retries, and reconciliation.
- Create deterministic per-issue workspaces and preserve them across runs.
- Stop active runs when issue state changes make them ineligible.
- Recover from transient failures with exponential backoff.
- Load runtime behavior from a repository-owned `WORKFLOW.md` contract.
- Expose operator-visible observability (at minimum structured logs).
- Support restart recovery without requiring a persistent database.

### 2.2 Non-Goals

- Rich web UI or multi-tenant control plane.
- Prescribing a specific dashboard or terminal UI implementation.
- General-purpose workflow engine or distributed job scheduler.
- Built-in business logic for how to edit tickets, PRs, or comments. (That logic lives in the
  workflow prompt and agent tooling.)
- Mandating strong sandbox controls beyond what the coding agent and host OS provide.
- Mandating a single default approval, sandbox, or operator-confirmation posture for all
  implementations.

## 3. System Overview

### 3.1 Main Components

1. `Workflow Loader`
   - Reads `WORKFLOW.md`.
   - Parses YAML front matter and prompt body.
   - Returns `{config, prompt_template}`.

2. `Config Layer`
   - Exposes typed getters for workflow config values.
   - Applies defaults and environment variable indirection.
   - Performs validation used by the orchestrator before dispatch.

3. `Issue Tracker Client` [GitHub]
   - Fetches candidate issues in active states from a GitHub repository.
   - Fetches current states for specific issue IDs (reconciliation).
   - Fetches terminal-state issues during startup cleanup.
   - Normalizes GitHub API payloads into a stable issue model.

4. `Orchestrator`
   - Owns the poll tick.
   - Owns the in-memory runtime state.
   - Decides which issues to dispatch, retry, stop, or release.
   - Tracks session metrics and retry queue state.

5. `Workspace Manager`
   - Maps issue identifiers to workspace paths.
   - Ensures per-issue workspace directories exist.
   - Runs workspace lifecycle hooks.
   - Cleans workspaces for terminal issues.

6. `Agent Runner` [Claude]
   - Creates workspace.
   - Builds prompt from issue + workflow template.
   - Launches the Claude Code CLI subprocess.
   - Streams agent updates back to the orchestrator.

7. `Status Surface` (optional)
   - Presents human-readable runtime status (for example terminal output, dashboard, or other
     operator-facing view).

8. `Logging`
   - Emits structured runtime logs to one or more configured sinks.

### 3.2 Abstraction Levels

Symphony is easiest to port when kept in these layers:

1. `Policy Layer` (repo-defined)
   - `WORKFLOW.md` prompt body.
   - Team-specific rules for ticket handling, validation, and handoff.

2. `Configuration Layer` (typed getters)
   - Parses front matter into typed runtime settings.
   - Handles defaults, environment tokens, and path normalization.

3. `Coordination Layer` (orchestrator)
   - Polling loop, issue eligibility, concurrency, retries, reconciliation.

4. `Execution Layer` (workspace + agent subprocess)
   - Filesystem lifecycle, workspace preparation, coding-agent CLI protocol.

5. `Integration Layer` (GitHub adapter) [GitHub]
   - API calls and normalization for tracker data.

6. `Observability Layer` (logs + optional status surface)
   - Operator visibility into orchestrator and agent behavior.

### 3.3 External Dependencies

- Issue tracker API (GitHub REST/GraphQL v4 for `tracker.kind: github`). [GitHub]
- Local filesystem for workspaces and logs.
- Optional workspace population tooling (for example Git CLI, if used).
- Coding-agent executable (Claude Code CLI, invoked as a subprocess per turn). [Claude]
- Host environment authentication for the issue tracker (`GITHUB_TOKEN`) and coding agent
  (`ANTHROPIC_API_KEY`, consumed by Claude Code internally). [GitHub] [Claude]

## 4. Core Domain Model

### 4.1 Entities

#### 4.1.1 Issue

Normalized issue record used by orchestration, prompt rendering, and observability output.

Fields:

- `id` (string)
  - Stable tracker-internal ID. [GitHub]: GraphQL node ID (e.g., `I_kwDOxyz123`).
- `identifier` (string)
  - Human-readable ticket key. [GitHub]: Issue number as string (e.g., `42`).
- `title` (string)
- `description` (string or null)
  - [GitHub]: Maps to the issue `body` field.
- `priority` (integer or null)
  - Lower numbers are higher priority in dispatch sorting.
  - [GitHub]: GitHub Issues have no native priority field. Implementations may derive priority
    from labels (e.g., `priority:1`) or leave as null (sorts last).
- `state` (string)
  - Current tracker state name. [GitHub]: `open` or `closed`.
- `branch_name` (string or null)
  - [GitHub]: Not available from issue metadata; always null unless derived from linked PRs.
- `url` (string or null)
  - [GitHub]: Issue HTML URL (e.g., `https://github.com/owner/repo/issues/42`).
- `labels` (list of strings)
  - Normalized to lowercase.
- `blocked_by` (list of blocker refs)
  - [GitHub]: Not available in GitHub Issues. Always empty list in MVP.
  - Future: may be derived from "Tracked by" / task list references.
- `created_at` (timestamp or null)
- `updated_at` (timestamp or null)

#### 4.1.2 Workflow Definition

[Unchanged]

Parsed `WORKFLOW.md` payload:

- `config` (map)
  - YAML front matter root object.
- `prompt_template` (string)
  - Markdown body after front matter, trimmed.

#### 4.1.3 Service Config (Typed View)

[Unchanged]

Typed runtime values derived from `WorkflowDefinition.config` plus environment resolution.

Examples:

- poll interval
- workspace root
- active and terminal issue states
- concurrency limits
- coding-agent executable/args/timeouts
- workspace hooks

#### 4.1.4 Workspace

[Unchanged]

Filesystem workspace assigned to one issue identifier.

Fields (logical):

- `path` (workspace path; current runtime typically uses absolute paths, but relative roots are
  possible if configured without path separators)
- `workspace_key` (sanitized issue identifier)
- `created_now` (boolean, used to gate `after_create` hook)

#### 4.1.5 Run Attempt

[Unchanged]

One execution attempt for one issue.

Fields (logical):

- `issue_id`
- `issue_identifier`
- `attempt` (integer or null, `null` for first run, `>=1` for retries/continuation)
- `workspace_path`
- `started_at`
- `status`
- `error` (optional)

#### 4.1.6 Live Session (Agent Session Metadata) [Claude]

State tracked while a coding-agent subprocess is running.

Fields:

- `session_id` (string, `<issue_id>-<turn_number>`)
- `agent_pid` (integer or null)
  - PID of the Claude Code subprocess.
- `last_event` (string/enum or null)
- `last_event_timestamp` (timestamp or null)
- `last_event_message` (summarized payload, truncated to 200 chars)
- `input_tokens` (integer)
- `output_tokens` (integer)
- `total_tokens` (integer)
- `cache_read_tokens` (integer)
- `cache_creation_tokens` (integer)
- `turn_count` (integer)
  - Number of coding-agent turns started within the current worker lifetime.

#### 4.1.7 Retry Entry

[Unchanged]

Scheduled retry state for an issue.

Fields:

- `issue_id`
- `identifier` (best-effort human ID for status surfaces/logs)
- `attempt` (integer, 1-based for retry queue)
- `due_at_ms` (monotonic clock timestamp)
- `timer_handle` (runtime-specific timer reference)
- `error` (string or null)

#### 4.1.8 Orchestrator Runtime State

Single authoritative in-memory state owned by the orchestrator.

Fields:

- `poll_interval_ms` (current effective poll interval)
- `max_concurrent_agents` (current effective global concurrency limit)
- `running` (map `issue_id -> running entry`)
- `claimed` (set of issue IDs reserved/running/retrying)
- `retry_attempts` (map `issue_id -> RetryEntry`)
- `completed` (set of issue IDs; bookkeeping only, not dispatch gating)
- `agent_totals` (aggregate tokens + runtime seconds)
- `rate_limits` (latest rate-limit snapshot; may track GitHub API and Anthropic API separately)

### 4.2 Stable Identifiers and Normalization Rules

- `Issue ID`
  - [GitHub]: GraphQL node ID. Use for tracker lookups and internal map keys.
- `Issue Identifier`
  - [GitHub]: Issue number as string. Use for human-readable logs and workspace naming.
- `Workspace Key`
  - Derive from `issue.identifier` by replacing any character not in `[A-Za-z0-9._-]` with `_`.
  - Use the sanitized value for the workspace directory name.
  - [GitHub]: Issue numbers are already safe (digits only), so sanitization is a no-op.
- `Normalized Issue State`
  - Compare states after `trim` + `lowercase`.
- `Session ID`
  - Compose as `<issue_id>-<turn_number>`.

## 5. Workflow Specification (Repository Contract)

### 5.1 File Discovery and Path Resolution

[Unchanged]

Workflow file path precedence:

1. Explicit application/runtime setting (set by CLI startup path).
2. Default: `WORKFLOW.md` in the current process working directory.

Loader behavior:

- If the file cannot be read, return `missing_workflow_file` error.
- The workflow file is expected to be repository-owned and version-controlled.

### 5.2 File Format

[Unchanged]

`WORKFLOW.md` is a Markdown file with optional YAML front matter.

Design note:

- `WORKFLOW.md` should be self-contained enough to describe and run different workflows (prompt,
  runtime settings, hooks, and tracker selection/config) without requiring out-of-band
  service-specific configuration.

Parsing rules:

- If file starts with `---`, parse lines until the next `---` as YAML front matter.
- Remaining lines become the prompt body.
- If front matter is absent, treat the entire file as prompt body and use an empty config map.
- YAML front matter must decode to a map/object; non-map YAML is an error.
- Prompt body is trimmed before use.

Returned workflow object:

- `config`: front matter root object (not nested under a `config` key).
- `prompt_template`: trimmed Markdown body.

### 5.3 Front Matter Schema

Top-level keys:

- `tracker`
- `polling`
- `workspace`
- `hooks`
- `agent`
- `claude` [Claude]

Unknown keys should be ignored for forward compatibility.

Note:

- The workflow front matter is extensible. Optional extensions may define additional top-level keys
  (for example `server`) without changing the core schema above.
- Extensions should document their field schema, defaults, validation rules, and whether changes
  apply dynamically or require restart.
- Common extension: `server.port` (integer) enables the optional HTTP server described in Section
  13.7.

#### 5.3.1 `tracker` (object) [GitHub]

Fields:

- `kind` (string)
  - Required for dispatch.
  - Supported value: `github`
- `endpoint` (string)
  - Default for `tracker.kind == "github"`: `https://api.github.com/graphql`
  - Override for GitHub Enterprise: set to the enterprise GraphQL endpoint.
- `api_key` (string)
  - May be a literal token or `$VAR_NAME`.
  - Canonical environment variable for `tracker.kind == "github"`: `GITHUB_TOKEN`.
  - If `$VAR_NAME` resolves to an empty string, treat the key as missing.
  - Required scope: `repo` for private repos, `public_repo` for public repos.
- `repo` (string)
  - Required for dispatch when `tracker.kind == "github"`.
  - Format: `owner/repo` (e.g., `myorg/myproject`).
- `labels` (list of strings, optional)
  - If present, only issues with at least one of these labels are fetched as candidates.
  - Useful for filtering a repository to only symphony-managed issues.
  - Label matching is case-insensitive.
- `active_states` (list of strings or comma-separated string)
  - Default: `open`
  - [GitHub]: GitHub Issues have two native states: `open` and `closed`.
  - For richer state workflows, use labels (e.g., `status:todo`, `status:in-progress`).
- `terminal_states` (list of strings or comma-separated string)
  - Default: `closed`

#### 5.3.2 `polling` (object)

[Unchanged]

Fields:

- `interval_ms` (integer or string integer)
  - Default: `30000`
  - Changes should be re-applied at runtime and affect future tick scheduling without restart.

#### 5.3.3 `workspace` (object)

[Unchanged]

Fields:

- `root` (path string or `$VAR`)
  - Default: `<system-temp>/symphony_workspaces`
  - `~` and strings containing path separators are expanded.
  - Bare strings without path separators are preserved as-is (relative roots are allowed but
    discouraged).

#### 5.3.4 `hooks` (object)

[Unchanged]

Fields:

- `after_create` (multiline shell script string, optional)
  - Runs only when a workspace directory is newly created.
  - Failure aborts workspace creation.
- `before_run` (multiline shell script string, optional)
  - Runs before each agent attempt after workspace preparation and before launching the coding
    agent.
  - Failure aborts the current attempt.
- `after_run` (multiline shell script string, optional)
  - Runs after each agent attempt (success, failure, timeout, or cancellation) once the workspace
    exists.
  - Failure is logged but ignored.
- `before_remove` (multiline shell script string, optional)
  - Runs before workspace deletion if the directory exists.
  - Failure is logged but ignored; cleanup still proceeds.
- `timeout_ms` (integer, optional)
  - Default: `60000`
  - Applies to all workspace hooks.
  - Non-positive values should be treated as invalid and fall back to the default.
  - Changes should be re-applied at runtime for future hook executions.

#### 5.3.5 `agent` (object)

[Unchanged]

Fields:

- `max_concurrent_agents` (integer or string integer)
  - Default: `10`
  - Changes should be re-applied at runtime and affect subsequent dispatch decisions.
- `max_turns` (integer or string integer)
  - Default: `20`
  - Maximum number of agent turns per worker session.
- `max_retry_backoff_ms` (integer or string integer)
  - Default: `300000` (5 minutes)
  - Changes should be re-applied at runtime and affect future retry scheduling.
- `max_concurrent_agents_by_state` (map `state_name -> positive integer`)
  - Default: empty map.
  - State keys are normalized (`trim` + `lowercase`) for lookup.
  - Invalid entries (non-positive or non-numeric) are ignored.

#### 5.3.6 `claude` (object) [Claude]

Fields for Claude Code CLI integration. This replaces the `codex` section from the original spec.

- `command` (string)
  - Default: `claude`
  - The Claude Code CLI executable name or path.
- `model` (string)
  - Default: `claude-sonnet-4-20250514`
  - The model ID passed to Claude Code via `--model`.
- `skip_permissions` (boolean)
  - Default: `false`
  - When `true`, passes `--dangerously-skip-permissions` to the CLI.
  - **Security warning**: This grants the agent unrestricted tool access. Only use in trusted
    environments with proper workspace isolation.
- `allowed_tools` (list of strings, optional)
  - If present, passed as `--allowedTools` to restrict which tools the agent can use.
  - Example: `["Bash", "Read", "Write", "Edit", "Glob", "Grep"]`
  - Ignored when `skip_permissions` is `true` (skip_permissions implies all tools allowed).
- `max_turns_per_invocation` (integer)
  - Default: `50`
  - Passed to Claude Code via `--max-turns` to limit turns within a single CLI invocation.
  - This is distinct from `agent.max_turns`, which limits the number of CLI invocations per
    worker session.
- `turn_timeout_ms` (integer)
  - Default: `3600000` (1 hour)
  - Maximum wall-clock time for a single Claude Code CLI invocation.
- `stall_timeout_ms` (integer)
  - Default: `300000` (5 minutes)
  - If `<= 0`, stall detection is disabled.
  - Enforced by the orchestrator based on event inactivity from the subprocess stdout.

### 5.4 Prompt Template Contract

[Unchanged except for the default fallback prompt.]

The Markdown body of `WORKFLOW.md` is the per-issue prompt template.

Rendering requirements:

- Use a strict template engine (Liquid-compatible semantics are sufficient).
- Unknown variables must fail rendering.
- Unknown filters must fail rendering.

Template input variables:

- `issue` (object)
  - Includes all normalized issue fields, including labels.
- `attempt` (integer or null)
  - `null`/absent on first attempt.
  - Integer on retry or continuation run.

Fallback prompt behavior:

- If the workflow prompt body is empty, the runtime may use a minimal default prompt
  (`You are working on an issue from GitHub.`). [GitHub]
- Workflow file read/parse failures are configuration/validation errors and should not silently fall
  back to a prompt.

### 5.5 Workflow Validation and Error Surface

[Unchanged]

Error classes:

- `missing_workflow_file`
- `workflow_parse_error`
- `workflow_front_matter_not_a_map`
- `template_parse_error` (during prompt rendering)
- `template_render_error` (unknown variable/filter, invalid interpolation)

Dispatch gating behavior:

- Workflow file read/YAML errors block new dispatches until fixed.
- Template errors fail only the affected run attempt.

## 6. Configuration Specification

### 6.1 Source Precedence and Resolution Semantics

[Unchanged]

Configuration precedence:

1. Workflow file path selection (runtime setting -> cwd default).
2. YAML front matter values.
3. Environment indirection via `$VAR_NAME` inside selected YAML values.
4. Built-in defaults.

Value coercion semantics:

- Path/command fields support:
  - `~` home expansion
  - `$VAR` expansion for env-backed path values
  - Apply expansion only to values intended to be local filesystem paths; do not rewrite URIs or
    arbitrary shell command strings.

### 6.2 Dynamic Reload Semantics

[Unchanged]

Dynamic reload is required:

- The software should watch `WORKFLOW.md` for changes.
- On change, it should re-read and re-apply workflow config and prompt template without restart.
- The software should attempt to adjust live behavior to the new config (for example polling
  cadence, concurrency limits, active/terminal states, claude settings, workspace paths/hooks, and
  prompt content for future runs).
- Reloaded config applies to future dispatch, retry scheduling, reconciliation decisions, hook
  execution, and agent launches.
- Implementations are not required to restart in-flight agent sessions automatically when config
  changes.
- Extensions that manage their own listeners/resources (for example an HTTP server port change) may
  require restart unless the implementation explicitly supports live rebind.
- Implementations should also re-validate/reload defensively during runtime operations (for example
  before dispatch) in case filesystem watch events are missed.
- Invalid reloads should not crash the service; keep operating with the last known good effective
  configuration and emit an operator-visible error.

### 6.3 Dispatch Preflight Validation

This validation is a scheduler preflight run before attempting to dispatch new work. It validates
the workflow/config needed to poll and launch workers, not a full audit of all possible workflow
behavior.

Startup validation:

- Validate configuration before starting the scheduling loop.
- If startup validation fails, fail startup and emit an operator-visible error.

Per-tick dispatch validation:

- Re-validate before each dispatch cycle.
- If validation fails, skip dispatch for that tick, keep reconciliation active, and emit an
  operator-visible error.

Validation checks:

- Workflow file can be loaded and parsed.
- `tracker.kind` is present and supported (`github`). [GitHub]
- `tracker.api_key` is present after `$` resolution.
- `tracker.repo` is present and in `owner/repo` format. [GitHub]
- `claude.command` is present and non-empty. [Claude]

### 6.4 Config Fields Summary (Cheat Sheet)

This section is intentionally redundant so a coding agent can implement the config layer quickly.

- `tracker.kind`: string, required, `github` [GitHub]
- `tracker.endpoint`: string, default `https://api.github.com/graphql` when `tracker.kind=github`
  [GitHub]
- `tracker.api_key`: string or `$VAR`, canonical env `GITHUB_TOKEN` when `tracker.kind=github`
  [GitHub]
- `tracker.repo`: string (`owner/repo`), required when `tracker.kind=github` [GitHub]
- `tracker.labels`: list of strings, optional issue label filter [GitHub]
- `tracker.active_states`: list/string, default `open` [GitHub]
- `tracker.terminal_states`: list/string, default `closed` [GitHub]
- `polling.interval_ms`: integer, default `30000`
- `workspace.root`: path, default `<system-temp>/symphony_workspaces`
- `hooks.after_create`: shell script or null
- `hooks.before_run`: shell script or null
- `hooks.after_run`: shell script or null
- `hooks.before_remove`: shell script or null
- `hooks.timeout_ms`: integer, default `60000`
- `agent.max_concurrent_agents`: integer, default `10`
- `agent.max_turns`: integer, default `20`
- `agent.max_retry_backoff_ms`: integer, default `300000` (5m)
- `agent.max_concurrent_agents_by_state`: map of positive integers, default `{}`
- `claude.command`: string, default `claude` [Claude]
- `claude.model`: string, default `claude-sonnet-4-20250514` [Claude]
- `claude.skip_permissions`: boolean, default `false` [Claude]
- `claude.allowed_tools`: list of strings, optional [Claude]
- `claude.max_turns_per_invocation`: integer, default `50` [Claude]
- `claude.turn_timeout_ms`: integer, default `3600000` [Claude]
- `claude.stall_timeout_ms`: integer, default `300000` [Claude]
- `server.port` (extension): integer, optional; enables the optional HTTP server, `0` may be used
  for ephemeral local bind, and CLI `--port` overrides it

## 7. Orchestration State Machine

[Unchanged from original. The orchestrator is tracker-agnostic.]

The orchestrator is the only component that mutates scheduling state. All worker outcomes are
reported back to it and converted into explicit state transitions.

### 7.1 Issue Orchestration States

This is not the same as tracker states (`open`, `closed`, etc.). This is the service's internal
claim state.

1. `Unclaimed`
   - Issue is not running and has no retry scheduled.

2. `Claimed`
   - Orchestrator has reserved the issue to prevent duplicate dispatch.
   - In practice, claimed issues are either `Running` or `RetryQueued`.

3. `Running`
   - Worker task exists and the issue is tracked in `running` map.

4. `RetryQueued`
   - Worker is not running, but a retry timer exists in `retry_attempts`.

5. `Released`
   - Claim removed because issue is terminal, non-active, missing, or retry path completed without
     re-dispatch.

Important nuance:

- A successful worker exit does not mean the issue is done forever.
- The worker may continue through multiple back-to-back coding-agent turns before it exits.
- After each normal turn completion, the worker re-checks the tracker issue state.
- If the issue is still in an active state, the worker should start another turn in the same
  workspace, up to `agent.max_turns`.
- [Claude]: Since Claude Code CLI runs as a fresh subprocess per turn (no persistent session),
  each turn is a separate invocation. The workspace state provides continuity.
- The first turn should use the full rendered task prompt.
- Continuation turns should send a continuation prompt that references the prior work in the
  workspace rather than resending the original task prompt.
- Once the worker exits normally, the orchestrator still schedules a short continuation retry
  (about 1 second) so it can re-check whether the issue remains active and needs another worker
  session.

### 7.2 Run Attempt Lifecycle

A run attempt transitions through these phases:

1. `PreparingWorkspace`
2. `BuildingPrompt`
3. `LaunchingAgentProcess`
4. `StreamingTurn`
5. `Finishing`
6. `Succeeded`
7. `Failed`
8. `TimedOut`
9. `Stalled`
10. `CanceledByReconciliation`

Note: `InitializingSession` from the original spec is removed because Claude Code CLI has no
handshake phase. [Claude]

Distinct terminal reasons are important because retry logic and logs differ.

### 7.3 Transition Triggers

- `Poll Tick`
  - Reconcile active runs.
  - Validate config.
  - Fetch candidate issues.
  - Dispatch until slots are exhausted.

- `Worker Exit (normal)`
  - Remove running entry.
  - Update aggregate runtime totals.
  - Schedule continuation retry (attempt `1`) after the worker exhausts or finishes its in-process
    turn loop.

- `Worker Exit (abnormal)`
  - Remove running entry.
  - Update aggregate runtime totals.
  - Schedule exponential-backoff retry.

- `Agent Update Event` [Claude]
  - Update live session fields, token counters.

- `Retry Timer Fired`
  - Re-fetch active candidates and attempt re-dispatch, or release claim if no longer eligible.
  - [GitHub]: Re-dispatch uses the `is_continuable` predicate (active + not blocked + no
    `symphony-done` label). The `symphony-doing` label does NOT block continuation because
    it was set by the current orchestrator instance.

- `Reconciliation State Refresh`
  - Stop runs whose issue states are terminal or no longer active.

- `Stall Timeout`
  - Kill worker and schedule retry.

### 7.4 Idempotency and Recovery Rules

[Unchanged]

- The orchestrator serializes state mutations through one authority to avoid duplicate dispatch.
- `claimed` and `running` checks are required before launching any worker.
- Reconciliation runs before dispatch on every tick.
- Restart recovery is tracker-driven and filesystem-driven (no durable orchestrator DB required).
- Startup terminal cleanup removes stale workspaces for issues already in terminal states.

## 8. Polling, Scheduling, and Reconciliation

### 8.1 Poll Loop

[Unchanged]

At startup, the service validates config, performs startup cleanup, schedules an immediate tick, and
then repeats every `polling.interval_ms`.

The effective poll interval should be updated when workflow config changes are re-applied.

Tick sequence:

1. Reconcile running issues.
2. Run dispatch preflight validation.
3. Fetch candidate issues from tracker using active states.
4. Sort issues by dispatch priority.
5. Dispatch eligible issues while slots remain.
6. Notify observability/status consumers of state changes.

If per-tick validation fails, dispatch is skipped for that tick, but reconciliation still happens
first.

### 8.2 Candidate Selection Rules

An issue is dispatch-eligible only if all are true:

- It has `id`, `identifier`, `title`, and `state`.
- Its state is in `active_states` and not in `terminal_states`.
- It is not already in `running`.
- It is not already in `claimed`.
- Global concurrency slots are available.
- Per-state concurrency slots are available.
- [GitHub]: If `tracker.labels` is configured, the issue has at least one matching label.
- [GitHub]: The issue does not have the `symphony-done` label (marks completed work).
- [GitHub]: The issue does not have the `symphony-doing` label (marks in-progress work by another instance).

Note: The blocker rule from the original spec (`Todo` state with non-terminal blockers) is not
applicable in the GitHub Issues MVP because GitHub does not have native blocker relations. [GitHub]

### 8.1.1 Symphony Label Convention [GitHub]

Symphony uses two reserved labels to track issue lifecycle independently of GitHub's `open`/`closed`
state. This allows human operators to keep issues open for review while preventing the orchestrator
from re-dispatching completed work.

| Label | Meaning | New dispatch | Continuation (re-dispatch) |
|---|---|---|---|
| _(none)_ | Untouched — eligible for work | Allowed | N/A |
| `symphony-doing` | An orchestrator instance is actively working on this issue | Blocked | Allowed (same instance) |
| `symphony-done` | Agent has completed its work on this issue | Blocked | Blocked |

- The agent (or workflow hooks) is responsible for adding/removing these labels via `gh` CLI or
  GitHub API.
- The orchestrator reads labels but never writes them; label management is the agent's responsibility
  (consistent with SPEC §1 boundary: "Ticket writes are performed by the coding agent").
- When an issue has `symphony-done`, the continuation retry loop (§7.2–7.3) stops, preventing
  infinite re-dispatch even if the issue remains `open`.

Sorting order (stable intent):

1. `priority` ascending (1..4 are preferred; null/unknown sorts last)
   [GitHub]: Priority derived from labels if available, otherwise null.
2. `created_at` oldest first
3. `identifier` lexicographic tie-breaker (numeric for GitHub issue numbers)

### 8.3 Concurrency Control

[Unchanged]

Global limit:

- `available_slots = max(max_concurrent_agents - running_count, 0)`

Per-state limit:

- `max_concurrent_agents_by_state[state]` if present (state key normalized)
- otherwise fallback to global limit

The runtime counts issues by their current tracked state in the `running` map.

### 8.4 Retry and Backoff

[Unchanged]

Retry entry creation:

- Cancel any existing retry timer for the same issue.
- Store `attempt`, `identifier`, `error`, `due_at_ms`, and new timer handle.

Backoff formula:

- Normal continuation retries after a clean worker exit use a short fixed delay of `1000` ms.
- Failure-driven retries use `delay = min(10000 * 2^(attempt - 1), agent.max_retry_backoff_ms)`.
- Power is capped by the configured max retry backoff (default `300000` / 5m).

Retry handling behavior:

1. Fetch active candidate issues (not all issues).
2. Find the specific issue by `issue_id`.
3. If not found, release claim.
4. If found and still candidate-eligible:
   - Dispatch if slots are available.
   - Otherwise requeue with error `no available orchestrator slots`.
5. If found but no longer active, release claim.

### 8.5 Active Run Reconciliation

[Unchanged]

Reconciliation runs every tick and has two parts.

Part A: Stall detection

- For each running issue, compute `elapsed_ms` since:
  - `last_event_timestamp` if any event has been seen, else
  - `started_at`
- If `elapsed_ms > claude.stall_timeout_ms`, terminate the worker and queue a retry.
- If `stall_timeout_ms <= 0`, skip stall detection entirely.

Part B: Tracker state refresh

- Fetch current issue states for all running issue IDs.
- For each running issue:
  - If tracker state is terminal: terminate worker and clean workspace.
  - If tracker state is still active: update the in-memory issue snapshot.
  - If tracker state is neither active nor terminal: terminate worker without workspace cleanup.
- If state refresh fails, keep workers running and try again on the next tick.

### 8.6 Startup Terminal Workspace Cleanup

[Unchanged]

When the service starts:

1. Query tracker for issues in terminal states.
2. For each returned issue identifier, remove the corresponding workspace directory.
3. If the terminal-issues fetch fails, log a warning and continue startup.

This prevents stale terminal workspaces from accumulating after restarts.

## 9. Workspace Management and Safety

[Unchanged from original. Workspace management is tracker-agnostic and agent-agnostic.]

### 9.1 Workspace Layout

Workspace root:

- `workspace.root` (normalized path; the current config layer expands path-like values and preserves
  bare relative names)

Per-issue workspace path:

- `<workspace.root>/<sanitized_issue_identifier>`

Workspace persistence:

- Workspaces are reused across runs for the same issue.
- Successful runs do not auto-delete workspaces.

### 9.2 Workspace Creation and Reuse

Input: `issue.identifier`

Algorithm summary:

1. Sanitize identifier to `workspace_key`.
2. Compute workspace path under workspace root.
3. Ensure the workspace path exists as a directory.
4. Mark `created_now=true` only if the directory was created during this call; otherwise
   `created_now=false`.
5. If `created_now=true`, run `after_create` hook if configured.

### 9.3 Optional Workspace Population (Implementation-Defined)

The spec does not require any built-in VCS or repository bootstrap behavior.

Implementations may populate or synchronize the workspace using implementation-defined logic and/or
hooks (for example `after_create` and/or `before_run`).

Failure handling:

- Workspace population/synchronization failures return an error for the current attempt.
- If failure happens while creating a brand-new workspace, implementations may remove the partially
  prepared directory.
- Reused workspaces should not be destructively reset on population failure unless that policy is
  explicitly chosen and documented.

### 9.4 Workspace Hooks

Supported hooks:

- `hooks.after_create`
- `hooks.before_run`
- `hooks.after_run`
- `hooks.before_remove`

Execution contract:

- Execute in a local shell context appropriate to the host OS, with the workspace directory as
  `cwd`.
- On POSIX systems, `sh -lc <script>` (or a stricter equivalent such as `bash -lc <script>`) is a
  conforming default.
- Hook timeout uses `hooks.timeout_ms`; default: `60000 ms`.
- Log hook start, failures, and timeouts.

Failure semantics:

- `after_create` failure or timeout is fatal to workspace creation.
- `before_run` failure or timeout is fatal to the current run attempt.
- `after_run` failure or timeout is logged and ignored.
- `before_remove` failure or timeout is logged and ignored.

### 9.5 Safety Invariants

This is the most important portability constraint.

Invariant 1: Run the coding agent only in the per-issue workspace path.

- Before launching the coding-agent subprocess, validate:
  - `cwd == workspace_path`

Invariant 2: Workspace path must stay inside workspace root.

- Normalize both paths to absolute.
- Require `workspace_path` to have `workspace_root` as a prefix directory.
- Reject any path outside the workspace root.

Invariant 3: Workspace key is sanitized.

- Only `[A-Za-z0-9._-]` allowed in workspace directory names.
- Replace all other characters with `_`.

## 10. Agent Runner Protocol (Claude Code Integration) [Claude]

This section defines the contract for integrating Claude Code CLI as the coding agent.

### 10.1 Launch Contract

Claude Code CLI is invoked as a subprocess per turn. There is no persistent app-server process.

Subprocess launch parameters:

- Command: `claude.command` (default: `claude`)
- Arguments:
  - `--print` (non-interactive mode, output to stdout)
  - `--output-format stream-json` (newline-delimited JSON events on stdout)
  - `--model <claude.model>`
  - `--max-turns <claude.max_turns_per_invocation>`
  - `-p <rendered_prompt>` (prompt text)
  - If `claude.skip_permissions` is true: `--dangerously-skip-permissions`
  - If `claude.allowed_tools` is set: `--allowedTools <comma-separated list>`
- Working directory: workspace path
- Stdout/stderr: separate streams
- Framing: newline-delimited JSON events on stdout

Recommended additional process settings:

- `kill_on_drop`: true (ensure child is cleaned up on cancellation)
- Max line size: 10 MB (for safe buffering)

### 10.2 Session Lifecycle

Unlike the original Codex app-server protocol, there is no JSON-RPC handshake. Each turn is a
complete CLI invocation:

1. Build the rendered prompt (full task prompt for turn 1, continuation prompt for later turns).
2. Spawn the Claude Code subprocess with the prompt.
3. Stream and parse JSON events from stdout until the process exits.
4. Collect exit code and final usage statistics.

Session identifiers:

- `session_id = "<issue_id>-<turn_number>"`
- Turn numbers are 1-based within a worker session.

### 10.3 Streaming Turn Processing

The client reads newline-delimited JSON events from stdout until the subprocess exits.

Completion conditions:

- Process exits with code 0 -> success
- Process exits with non-zero code -> failure
- Turn timeout (`claude.turn_timeout_ms`) exceeded -> kill process, failure
- Stall detected (no stdout activity for `claude.stall_timeout_ms`) -> kill process, failure

Line handling requirements:

- Read JSON events from stdout only.
- Buffer partial stdout lines until newline arrives.
- Attempt JSON parse on complete stdout lines.
- Stderr is not part of the event stream:
  - Log it as diagnostics (truncated)
  - Do not attempt JSON parsing on stderr

Claude Code stream-json event types:

Events are newline-delimited JSON objects. The implementation should handle at minimum:

- `{"type": "assistant", "message": {...}}` - Assistant response content
- `{"type": "tool_use", "tool": "...", ...}` - Tool invocation
- `{"type": "tool_result", ...}` - Tool execution result
- `{"type": "result", "result": "...", "usage": {...}}` - Final result with token usage
- `{"type": "error", ...}` - Error during execution

Unknown event types should be logged and ignored (forward compatibility).

### 10.4 Emitted Runtime Events (Upstream to Orchestrator)

The agent runner emits structured events to the orchestrator. Each event should include:

- `event` (enum/string)
- `timestamp` (UTC timestamp)
- `agent_pid` (if available)
- optional `usage` map (token counts)
- payload fields as needed

Important emitted events:

- `session_started` - CLI process spawned
- `startup_failed` - CLI process failed to start
- `turn_completed` - CLI process exited 0
- `turn_failed` - CLI process exited non-zero
- `turn_timed_out` - Turn timeout exceeded
- `turn_stalled` - Stall timeout exceeded
- `agent_event` - Forwarded Claude Code stdout event (for observability)

### 10.5 Permission and Safety Policy [Claude]

Claude Code permission behavior is controlled by CLI flags rather than a protocol handshake.

Policy configuration:

- `claude.skip_permissions: true` -> `--dangerously-skip-permissions`
  - Grants unrestricted tool access. For trusted environments only.
- `claude.allowed_tools: [...]` -> `--allowedTools Bash,Read,Write,...`
  - Restricts which tools the agent may use.
  - Recommended for production deployments to limit blast radius.
- If neither is set, Claude Code runs in its default interactive permission mode, which will
  cause the subprocess to hang waiting for user approval. Implementations should treat this as a
  configuration error for unattended operation and fail validation.

Implementation must ensure:

- Unattended operation requires either `skip_permissions: true` or a non-empty `allowed_tools` list.
- This is validated at dispatch preflight time.

### 10.6 Timeouts and Error Mapping

Timeouts:

- `claude.turn_timeout_ms`: total time for a single CLI invocation
- `claude.stall_timeout_ms`: enforced by orchestrator based on stdout event inactivity

Error mapping (recommended normalized categories):

- `claude_not_found` - CLI executable not found in PATH
- `invalid_workspace_cwd` - workspace path validation failed
- `turn_timeout` - CLI invocation exceeded turn_timeout_ms
- `turn_stalled` - no stdout activity for stall_timeout_ms
- `process_exit` - CLI exited with non-zero code
- `turn_failed` - CLI reported error in stream
- `permission_config_error` - neither skip_permissions nor allowed_tools configured

### 10.7 Agent Runner Contract

The `Agent Runner` wraps workspace + prompt + Claude Code CLI.

Behavior:

1. Create/reuse workspace for issue.
2. Build prompt from workflow template.
3. Spawn Claude Code CLI subprocess in workspace directory.
4. Stream stdout events and forward to orchestrator.
5. On any error, fail the worker attempt (the orchestrator will retry).

Note:

- Workspaces are intentionally preserved after successful runs.
- Each turn is a separate CLI invocation. Workspace state (files on disk) provides continuity
  between turns, not an in-memory session.

## 11. Issue Tracker Integration Contract (GitHub) [GitHub]

### 11.1 Required Operations

An implementation must support these tracker adapter operations:

1. `fetch_candidate_issues()`
   - Return issues in configured active states for the configured repository.
   - If `tracker.labels` is configured, filter by label.

2. `fetch_issues_by_states(state_names)`
   - Used for startup terminal cleanup.
   - [GitHub]: Map state names to GitHub `IssueState` enum values (`OPEN`, `CLOSED`).

3. `fetch_issue_states_by_ids(issue_ids)`
   - Used for active-run reconciliation.
   - [GitHub]: Use GraphQL `nodes` query with issue node IDs.

### 11.2 Query Semantics (GitHub)

GitHub-specific requirements for `tracker.kind == "github"`:

- GraphQL endpoint (default `https://api.github.com/graphql`)
- Auth token sent in `Authorization: Bearer <token>` header
- `tracker.repo` is split into `owner` and `repo` for GraphQL variables
- Candidate issue query filters repository using
  `repository(owner: $owner, name: $repo) { issues(...) }`
- If `tracker.labels` is configured, include in query: `labels: $labels`
- Issue-state refresh query uses GraphQL node IDs with variable type `[ID!]`
- Pagination required for candidate issues
- Page size default: `50`
- Maximum pages per fetch: `10` (500 issue cap to prevent runaway)
- Network timeout: `30000 ms`

Rate limit handling:

- Parse `X-RateLimit-Remaining` and `X-RateLimit-Reset` from response headers.
- If `remaining < 100`, log warning.
- If `remaining == 0`, sleep until reset timestamp (with jitter up to 5s).
- On HTTP 403 with rate limit error, apply exponential backoff: `min(1s * 2^attempt, 60s)`.
- On 5xx or network error, retry up to 3 times with backoff: `1s, 2s, 4s`.

Important:

- GitHub GraphQL schema is stable but may add fields. Keep query construction isolated and test
  the exact query fields/types required by this specification.

### 11.3 Normalization Rules

Candidate issue normalization should produce fields listed in Section 4.1.1.

GitHub-specific normalization details:

- `id` -> GraphQL node ID (`id` field)
- `identifier` -> issue `number` as string
- `title` -> issue `title`
- `description` -> issue `body` (may be null)
- `priority` -> null (or derived from label patterns like `priority:1`)
- `state` -> `open` or `closed` (lowercase)
- `branch_name` -> null
- `url` -> issue HTML URL
- `labels` -> lowercase label name strings
- `blocked_by` -> empty list (not available in GitHub Issues MVP)
- `created_at` -> parse ISO-8601 `createdAt` field
- `updated_at` -> parse ISO-8601 `updatedAt` field

### 11.4 Error Handling Contract

Recommended error categories:

- `unsupported_tracker_kind`
- `missing_tracker_api_key`
- `missing_tracker_repo`
- `invalid_tracker_repo_format` (not `owner/repo`)
- `github_api_request` (transport failures)
- `github_api_status` (non-200 HTTP)
- `github_api_rate_limited` (HTTP 403 rate limit)
- `github_graphql_errors`
- `github_unknown_payload`
- `github_missing_end_cursor` (pagination integrity error)

Orchestrator behavior on tracker errors:

- Candidate fetch failure: log and skip dispatch for this tick.
- Running-state refresh failure: log and keep active workers running.
- Startup terminal cleanup failure: log warning and continue startup.

### 11.5 Tracker Writes (Important Boundary)

Symphony does not require first-class tracker write APIs in the orchestrator.

- Ticket mutations (state transitions, comments, PR metadata) are typically handled by the coding
  agent using tools available in the workflow prompt (for example `gh` CLI). [GitHub]
- The service remains a scheduler/runner and tracker reader.
- Workflow-specific success often means "reached a handoff state" (for example a specific label
  applied or a PR created) rather than issue closure.

## 12. Prompt Construction and Context Assembly

[Unchanged]

### 12.1 Inputs

Inputs to prompt rendering:

- `workflow.prompt_template`
- normalized `issue` object
- optional `attempt` integer (retry/continuation metadata)

### 12.2 Rendering Rules

- Render with strict variable checking.
- Render with strict filter checking.
- Convert issue object keys to strings for template compatibility.
- Preserve nested arrays/maps (labels) so templates can iterate.

### 12.3 Retry/Continuation Semantics

`attempt` should be passed to the template because the workflow prompt may provide different
instructions for:

- first run (`attempt` null or absent)
- continuation run after a successful prior session
- retry after error/timeout/stall

### 12.4 Failure Semantics

If prompt rendering fails:

- Fail the run attempt immediately.
- Let the orchestrator treat it like any other worker failure and decide retry behavior.

## 13. Logging, Status, and Observability

### 13.1 Logging Conventions

[Unchanged]

Required context fields for issue-related logs:

- `issue_id`
- `issue_identifier`

Required context for coding-agent session lifecycle logs:

- `session_id`

Message formatting requirements:

- Use stable `key=value` phrasing.
- Include action outcome (`completed`, `failed`, `retrying`, etc.).
- Include concise failure reason when present.
- Avoid logging large raw payloads unless necessary.

### 13.2 Logging Outputs and Sinks

[Unchanged]

### 13.3 Runtime Snapshot / Monitoring Interface (Optional but Recommended)

If the implementation exposes a synchronous runtime snapshot (for dashboards or monitoring), it
should return:

- `running` (list of running session rows)
- each running row should include `turn_count`
- `retrying` (list of retry queue rows)
- `agent_totals`
  - `input_tokens`
  - `output_tokens`
  - `total_tokens`
  - `cache_read_tokens` [Claude]
  - `cache_creation_tokens` [Claude]
  - `seconds_running` (aggregate runtime seconds as of snapshot time, including active sessions)
- `rate_limits` (latest rate-limit snapshot, if available; may include both GitHub API and
  Anthropic API rate limits)

### 13.4 Optional Human-Readable Status Surface

[Unchanged]

### 13.5 Session Metrics and Token Accounting [Claude]

Token accounting rules:

- Claude Code `result` events include a `usage` object with token counts.
- Extract `input_tokens` and `output_tokens` from the `usage` object.
- Also extract `cache_read_input_tokens` and `cache_creation_input_tokens` if present.
- `total_tokens = input_tokens + output_tokens`
- Each CLI invocation provides final usage in its `result` event; use these as absolute values
  for that turn.
- Accumulate aggregate totals in orchestrator state across all turns and sessions.

Runtime accounting:

- Runtime should be reported as a live aggregate at snapshot/render time.
- Implementations may maintain a cumulative counter for ended sessions and add active-session
  elapsed time derived from `running` entries (for example `started_at`) when producing a
  snapshot/status view.
- Add run duration seconds to the cumulative ended-session runtime when a session ends (normal exit
  or cancellation/termination).

Rate-limit tracking:

- Track GitHub API rate limits from response headers.
- Anthropic API rate limits may be available from Claude Code stderr output (best-effort).
- Any human-readable presentation of rate-limit data is implementation-defined.

### 13.6 Humanized Agent Event Summaries (Optional)

[Unchanged]

### 13.7 Optional HTTP Server Extension

[Unchanged from original. See original SPEC.md Section 13.7 for full details.]

## 14. Failure Model and Recovery Strategy

### 14.1 Failure Classes

1. `Workflow/Config Failures`
   - Missing `WORKFLOW.md`
   - Invalid YAML front matter
   - Unsupported tracker kind or missing tracker credentials/repo [GitHub]
   - Missing coding-agent executable
   - Missing permission configuration for unattended mode [Claude]

2. `Workspace Failures`
   - Workspace directory creation failure
   - Workspace population/synchronization failure (implementation-defined; may come from hooks)
   - Invalid workspace path configuration
   - Hook timeout/failure

3. `Agent Session Failures` [Claude]
   - CLI process spawn failure
   - Turn failed (non-zero exit)
   - Turn timeout
   - Subprocess exit during streaming
   - Stalled session (no stdout activity)

4. `Tracker Failures` [GitHub]
   - API transport errors
   - Non-200 status
   - Rate limiting (HTTP 403)
   - GraphQL errors
   - Malformed payloads

5. `Observability Failures`
   - Snapshot timeout
   - Dashboard render errors
   - Log sink configuration failure

### 14.2 Recovery Behavior

[Unchanged]

### 14.3 Partial State Recovery (Restart)

[Unchanged]

### 14.4 Operator Intervention Points

Operators can control behavior by:

- Editing `WORKFLOW.md` (prompt and most runtime settings).
- `WORKFLOW.md` changes should be detected and re-applied automatically without restart.
- Changing issue states in the tracker: [GitHub]
  - Closing an issue -> running session is stopped and workspace cleaned when reconciled.
  - Reopening a closed issue -> becomes eligible for dispatch on next poll.
- Removing/adding the filter label from/to an issue (if `tracker.labels` is configured). [GitHub]
- Restarting the service for process recovery or deployment (not as the normal path for applying
  workflow config changes).

## 15. Security and Operational Safety

### 15.1 Trust Boundary Assumption

Each implementation defines its own trust boundary.

Operational safety requirements:

- Implementations should state clearly whether they are intended for trusted environments, more
  restrictive environments, or both.
- Implementations should state clearly whether they rely on `--dangerously-skip-permissions`,
  `--allowedTools` restrictions, or other controls. [Claude]
- Workspace isolation and path validation are important baseline controls, but they are not a
  substitute for whatever permission policy an implementation chooses.

### 15.2 Filesystem Safety Requirements

[Unchanged]

### 15.3 Secret Handling

- Support `$VAR` indirection in workflow config.
- Do not log API tokens or secret env values.
- Validate presence of secrets without printing them.
- `GITHUB_TOKEN` is the primary secret managed by Symphony. [GitHub]
- `ANTHROPIC_API_KEY` is consumed by Claude Code internally; Symphony does not need to read it
  directly but should validate that Claude Code can start successfully. [Claude]

### 15.4 Hook Script Safety

[Unchanged]

### 15.5 Harness Hardening Guidance

Running coding agents against repositories, issue trackers, and other inputs that may contain
sensitive data or externally-controlled content can be dangerous. A permissive deployment can lead
to data leaks, destructive mutations, or full machine compromise if the agent is induced to execute
harmful commands or use overly-powerful integrations.

Implementations should explicitly evaluate their own risk profile and harden the execution harness
where appropriate.

Possible hardening measures include:

- Using `claude.allowed_tools` to restrict the agent to a minimal tool set instead of running with
  `--dangerously-skip-permissions`. [Claude]
- Adding external isolation layers such as OS/container/VM sandboxing, network restrictions, or
  separate credentials.
- Filtering which GitHub issues, repositories, or labels are eligible for dispatch so untrusted
  or out-of-scope tasks do not automatically reach the agent. [GitHub]
- Using a `GITHUB_TOKEN` with minimal required scopes rather than a broad personal access token.
  [GitHub]
- Reducing the set of filesystem paths and network destinations available to the agent to the
  minimum needed for the workflow.

## 16. Reference Algorithms (Language-Agnostic)

### 16.1 Service Startup

```text
function start_service():
  configure_logging()
  start_observability_outputs()
  start_workflow_watch(on_change=reload_and_reapply_workflow)

  state = {
    poll_interval_ms: get_config_poll_interval_ms(),
    max_concurrent_agents: get_config_max_concurrent_agents(),
    running: {},
    claimed: set(),
    retry_attempts: {},
    completed: set(),
    agent_totals: {input_tokens: 0, output_tokens: 0, total_tokens: 0, seconds_running: 0},
    rate_limits: null
  }

  validation = validate_dispatch_config()
  if validation is not ok:
    log_validation_error(validation)
    fail_startup(validation)

  startup_terminal_workspace_cleanup()
  schedule_tick(delay_ms=0)

  event_loop(state)
```

### 16.2 Poll-and-Dispatch Tick

[Unchanged from original, see SPEC.md Section 16.2]

### 16.3 Reconcile Active Runs

[Unchanged from original, see SPEC.md Section 16.3]

### 16.4 Dispatch One Issue

[Unchanged from original, see SPEC.md Section 16.4]

### 16.5 Worker Attempt (Workspace + Prompt + Agent) [Claude]

```text
function run_agent_attempt(issue, attempt, orchestrator_channel):
  workspace = workspace_manager.create_for_issue(issue.identifier)
  if workspace failed:
    fail_worker("workspace error")

  if run_hook("before_run", workspace.path) failed:
    fail_worker("before_run hook error")

  max_turns = config.agent.max_turns
  turn_number = 1

  while true:
    prompt = build_turn_prompt(workflow_template, issue, attempt, turn_number, max_turns)
    if prompt failed:
      run_hook_best_effort("after_run", workspace.path)
      fail_worker("prompt error")

    turn_result = claude_cli.run_turn(
      workspace=workspace.path,
      prompt=prompt,
      config=claude_config,
      on_event=(event) -> send(orchestrator_channel, {agent_update, issue.id, event})
    )

    if turn_result failed:
      run_hook_best_effort("after_run", workspace.path)
      fail_worker("agent turn error: " + turn_result.error)

    refreshed_issue = tracker.fetch_issue_states_by_ids([issue.id])
    if refreshed_issue failed:
      run_hook_best_effort("after_run", workspace.path)
      fail_worker("issue state refresh error")

    issue = refreshed_issue[0] or issue

    if issue.state is not active:
      break

    if turn_number >= max_turns:
      break

    turn_number = turn_number + 1

  run_hook_best_effort("after_run", workspace.path)

  exit_normal()
```

### 16.6 Worker Exit and Retry Handling

[Unchanged from original, see SPEC.md Section 16.6]

## 17. Test and Validation Matrix

A conforming implementation should include tests that cover the behaviors defined in this
specification.

Validation profiles:

- `Core Conformance`: deterministic tests required for all conforming implementations.
- `Extension Conformance`: required only for optional features that an implementation chooses to
  ship.
- `Real Integration Profile`: environment-dependent smoke/integration checks recommended before
  production use.

### 17.1 Workflow and Config Parsing

- Workflow file path precedence:
  - explicit runtime path is used when provided
  - cwd default is `WORKFLOW.md` when no explicit runtime path is provided
- Workflow file changes are detected and trigger re-read/re-apply without restart
- Invalid workflow reload keeps last known good effective configuration and emits an
  operator-visible error
- Missing `WORKFLOW.md` returns typed error
- Invalid YAML front matter returns typed error
- Front matter non-map returns typed error
- Config defaults apply when optional values are missing
- `tracker.kind` validation enforces currently supported kind (`github`) [GitHub]
- `tracker.api_key` works (including `$VAR` indirection)
- `tracker.repo` validation enforces `owner/repo` format [GitHub]
- `$VAR` resolution works for tracker API key and path values
- `~` path expansion works
- `claude.command` is preserved as a string [Claude]
- `claude.skip_permissions` or `claude.allowed_tools` required for unattended mode [Claude]
- Per-state concurrency override map normalizes state names and ignores invalid values
- Prompt template renders `issue` and `attempt`
- Prompt rendering fails on unknown variables (strict mode)

### 17.2 Workspace Manager and Safety

[Unchanged]

### 17.3 Issue Tracker Client [GitHub]

- Candidate issue fetch uses active states and repository
- GitHub query uses the specified repository filter (`owner`/`repo`)
- If `tracker.labels` is configured, query includes label filter
- Empty `fetch_issues_by_states([])` returns empty without API call
- Pagination preserves order across multiple pages
- Pagination stops at maximum page limit (10 pages / 500 issues)
- Labels are normalized to lowercase
- Issue state refresh by ID returns minimal normalized issues
- Issue state refresh query uses GraphQL node IDs (`[ID!]`)
- Rate limit headers are parsed and tracked
- Rate limit exhaustion triggers appropriate backoff
- Error mapping for request errors, non-200, rate limits, GraphQL errors, malformed payloads

### 17.4 Orchestrator Dispatch, Reconciliation, and Retry

- Dispatch sort order is priority then oldest creation time
- Active-state issue refresh updates running entry state
- Non-active state stops running agent without workspace cleanup
- Terminal state stops running agent and cleans workspace
- Reconciliation with no running issues is a no-op
- Normal worker exit schedules a short continuation retry (attempt 1)
- Abnormal worker exit increments retries with 10s-based exponential backoff
- Retry backoff cap uses configured `agent.max_retry_backoff_ms`
- Retry queue entries include attempt, due time, identifier, and error
- Stall detection kills stalled sessions and schedules retry
- Slot exhaustion requeues retries with explicit error reason
- If a snapshot API is implemented, it returns running rows, retry rows, token totals, and rate
  limits
- If a snapshot API is implemented, timeout/unavailable cases are surfaced

### 17.5 Coding-Agent CLI Client [Claude]

- Launch command uses workspace cwd and invokes Claude Code with correct arguments
- `--print` and `--output-format stream-json` are always passed
- `--model` uses configured model
- `--dangerously-skip-permissions` is passed when `skip_permissions` is true
- `--allowedTools` is passed when `allowed_tools` is configured
- Validation fails if neither `skip_permissions` nor `allowed_tools` is configured
- Turn timeout is enforced; process is killed on timeout
- Partial JSON lines are buffered until newline
- Stdout and stderr are handled separately; JSON events are parsed from stdout only
- Non-JSON stderr lines are logged but do not crash parsing
- Token usage is extracted from `result` events
- Unknown event types are logged and ignored (forward compatibility)
- Process is cleaned up (killed) on cancellation

### 17.6 Observability

[Unchanged]

### 17.7 CLI and Host Lifecycle

[Unchanged]

### 17.8 Real Integration Profile (Recommended)

These checks are recommended for production readiness and may be skipped in CI when credentials,
network access, or external service permissions are unavailable.

- A real tracker smoke test can be run with valid credentials supplied by `GITHUB_TOKEN` or a
  documented local bootstrap mechanism. [GitHub]
- A real agent smoke test verifies Claude Code CLI is installed and can start with the configured
  permission flags. [Claude]
- Real integration tests should use isolated test identifiers/workspaces and clean up tracker
  artifacts when practical.
- A skipped real-integration test should be reported as skipped, not silently treated as passed.
- If a real-integration profile is explicitly enabled in CI or release validation, failures should
  fail that job.

## 18. Implementation Checklist (Definition of Done)

### 18.1 Required for Conformance

- Workflow path selection supports explicit runtime path and cwd default
- `WORKFLOW.md` loader with YAML front matter + prompt body split
- Typed config layer with defaults and `$` resolution
- Dynamic `WORKFLOW.md` watch/reload/re-apply for config and prompt
- Polling orchestrator with single-authority mutable state
- GitHub Issues tracker client with candidate fetch + state refresh + terminal fetch [GitHub]
- Workspace manager with sanitized per-issue workspaces
- Workspace lifecycle hooks (`after_create`, `before_run`, `after_run`, `before_remove`)
- Hook timeout config (`hooks.timeout_ms`, default `60000`)
- Claude Code CLI subprocess agent runner with stream-json event parsing [Claude]
- Claude launch command config (`claude.command`, default `claude`) [Claude]
- Permission validation (`skip_permissions` or `allowed_tools` required) [Claude]
- Strict prompt rendering with `issue` and `attempt` variables
- Exponential retry queue with continuation retries after normal exit
- Configurable retry backoff cap (`agent.max_retry_backoff_ms`, default 5m)
- Reconciliation that stops runs on terminal/non-active tracker states
- Workspace cleanup for terminal issues (startup sweep + active transition)
- Structured logs with `issue_id`, `issue_identifier`, and `session_id`
- Operator-visible observability (structured logs; optional snapshot/status surface)

### 18.2 Recommended Extensions (Not Required for Conformance)

- Optional HTTP server honors CLI `--port` over `server.port`, uses a safe default bind host, and
  exposes the baseline endpoints/error semantics in Section 13.7 if shipped.
- Optional `github_graphql` client-side tool extension (future; equivalent of original
  `linear_graphql`). [GitHub]
- GitHub Projects v2 tracker adapter (`tracker.kind: github-project`). [GitHub]
  See Section 19 for the full specification.
- Priority derivation from GitHub labels. [GitHub]
- Blocker derivation from GitHub task lists / tracked-by references. [GitHub]
- TODO: Persist retry queue and session metadata across process restarts.
- TODO: Add pluggable issue tracker adapters beyond GitHub.

---

## Section 19: GitHub Projects V2 Tracker Adapter [GitHub]

> **Status**: Specification only. Not yet implemented.
> Reference implementation target: Rust crate `symphony`, tracker kind `github-project`.

### 19.1 Motivation

The existing `github` tracker kind polls plain GitHub Issues filtered by label and
state. This covers simple workflows, but many teams organize work in GitHub Projects
(v2), which adds:

- A custom **Status field** (single-select) that is independent of the issue's
  `OPEN`/`CLOSED` state. For example, an issue can remain OPEN but have Status
  "Done" (waiting for merge review) or Status "Blocked".
- Custom fields (priority, iteration, estimate) that are project-specific.
- A project board view that acts as the team's canonical source of workflow truth.

The `github-project` adapter uses the **ProjectV2 GraphQL API** to treat the
project's Status field as the authoritative source for active/terminal states,
rather than the issue's own state.

### 19.2 Configuration

```yaml
tracker:
  kind: github-project       # New tracker kind
  owner: myorg               # GitHub org or user login (required)
  owner_type: organization   # "organization" | "user" (default: organization)
  project_number: 42         # Project number from URL (required)
  repo: myorg/myrepo         # Still required: used to post comments, fetch issue body
  api_key: ${GITHUB_TOKEN}   # PAT with scopes: read:project, repo

  # Status field name in the project (default: "Status")
  status_field_name: Status

  # Items with these Status values are eligible for dispatch
  active_statuses:
    - "In Progress"
    - "Todo"

  # Items with these Status values are considered terminal (abandon if running)
  terminal_statuses:
    - "Done"
    - "Cancelled"

  # Labels on the underlying issue for additional filtering (optional)
  # If empty, no label filter is applied
  labels: []
```

> **Compatibility note**: `active_states` / `terminal_states` (used by the
> `github` adapter) become `active_statuses` / `terminal_statuses` for the
> `github-project` adapter to distinguish project Status values from issue states.

### 19.3 GraphQL API Overview

The adapter makes two categories of GraphQL calls:

#### 19.3.1 Project field discovery (once per startup)

Fetch the project's field definitions to resolve the Status field ID and
option IDs. This mapping is cached for the lifetime of the process.

```graphql
query($owner: String!, $number: Int!, $ownerType: String!) {
  # ownerType branch: organization(...) or user(...)
  organization(login: $owner) {
    projectV2(number: $number) {
      id          # ProjectV2 node ID — used for subsequent item queries
      fields(first: 50) {
        nodes {
          ... on ProjectV2SingleSelectField {
            id    # STATUS_FIELD_ID
            name  # e.g. "Status"
            options {
              id   # option node ID — required for updateProjectV2ItemFieldValue
              name # e.g. "In Progress"
            }
          }
        }
      }
    }
  }
}
```

Cache structure (in-memory, never persisted):

```rust
struct ProjectMeta {
    project_node_id: String,
    status_field_id: String,
    // status option name -> option node ID
    status_options: HashMap<String, String>,
}
```

#### 19.3.2 Polling: fetch active project items

Called on each poll interval. Paginates through all project items and
filters client-side (the GraphQL API offers no server-side Status filter).

```graphql
query($projectId: ID!, $after: String) {
  node(id: $projectId) {
    ... on ProjectV2 {
      items(first: 100, after: $after) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id    # ProjectV2Item node ID (used for field updates, NOT the issue ID)
          status: fieldValueByName(name: "Status") {
            ... on ProjectV2ItemFieldSingleSelectValue {
              name    # e.g. "In Progress"
            }
          }
          content {
            ... on Issue {
              id          # Issue node ID — used as the Symphony issue.id
              number      # Issue number
              title
              body
              state       # OPEN | CLOSED (issue's own state)
              labels(first: 10) {
                nodes { name }
              }
              createdAt
              updatedAt
            }
          }
        }
      }
    }
  }
}
```

Client-side filtering rules (applied after fetching all pages):

1. `content` must be an `Issue` (skip DraftIssue and PullRequest items).
2. Issue `state` must be `OPEN` (closed issues are never dispatched).
3. `status.name` must be in `active_statuses`.
4. If `labels` config is non-empty, the issue must have at least one matching label.

#### 19.3.3 Reconciliation: fetch status for specific issue IDs

Used when `handle_retry` re-checks a running issue. Fetches the current
project Status for a set of issue node IDs.

```graphql
query($projectId: ID!, $after: String) {
  # Same pagination as §19.3.2 — filter by issue.id client-side
  # No server-side filter by issue ID is available in the ProjectV2 API
}
```

> **Implementation note**: There is no efficient "fetch items by issue ID" API in
> ProjectV2. The reconciliation query must paginate all items and filter by
> `content { ... on Issue { id } }`. For large projects this may be slow.
> A future optimisation is to cache `issue_id → project_item_id` between polls.

### 19.4 Issue Model Mapping

| Symphony `Issue` field | Source |
|---|---|
| `id` | Issue node ID (`content { ... on Issue { id } }`) |
| `identifier` | Issue number as string (`number`) |
| `title` | `title` |
| `description` | `body` |
| `state` | Derived: `OPEN` if status in active_statuses, else `CLOSED` |
| `labels` | `labels.nodes[].name` |
| `priority` | Not available from Projects v2 without a custom Priority field |
| `created_at` | `createdAt` |
| `updated_at` | `updatedAt` |
| `project_item_id` | ProjectV2Item node ID (stored separately; used for field updates) |

> The adapter stores `project_item_id → issue_id` in a side-map so that
> `updateProjectV2ItemFieldValue` can be called without a second lookup.

### 19.5 Normalised `is_active()` Semantics

For the `github-project` adapter, `Issue::is_active()` returns `true` when:
- Issue `state` is `OPEN`, **and**
- The project Status name is in `active_statuses`

This overrides the base `github` adapter behaviour where only `state == OPEN` is
checked.

### 19.6 Status Update on Completion (Optional Extension)

When an agent run completes successfully, the adapter **may** (not required)
update the project item's Status field to a configured value (e.g. "Done"):

```graphql
mutation($projectId: ID!, $itemId: ID!, $fieldId: ID!, $optionId: String!) {
  updateProjectV2ItemFieldValue(
    input: {
      projectId: $projectId
      itemId: $itemId
      fieldId: $fieldId
      value: { singleSelectOptionId: $optionId }
    }
  ) {
    projectV2Item { id }
  }
}
```

Config to enable:

```yaml
tracker:
  on_completion_set_status: "Done"   # optional; omit to disable
```

### 19.7 Rate Limit Considerations

| Scenario | Estimated cost |
|---|---|
| Field discovery (startup) | ~10–20 points (one-time) |
| Full project poll, 100 items | ~50–150 points |
| Full project poll, 1000 items | ~500–1500 points (10 pages) |
| Reconciliation (same as poll) | Same as poll |

With a 30-second poll interval and 5,000 points/hour limit, a 1000-item project
consumes roughly 1,500 × 120 = 180,000 points/hour — **exceeding the limit**.
Implementations MUST:

1. Check `X-RateLimit-Remaining` on each response and back off when below a
   configurable threshold (default: 500 remaining).
2. Cache the project item list between polls and perform delta-reconciliation
   only for items whose `updatedAt` changed (requires storing the last-seen
   `updatedAt` per item).
3. Consider increasing `poll_interval_ms` for large projects (recommended
   minimum: 60,000 ms for projects with > 200 items).

### 19.8 Error Handling

| Error | Handling |
|---|---|
| 401 Unauthorized | Fatal startup error — check token scopes (`read:project`, `repo`) |
| 403 Forbidden on project | Fatal — check project visibility and token access |
| Project not found (null) | Fatal — check `owner`, `owner_type`, `project_number` |
| Status field not found | Fatal — check `status_field_name` config |
| Rate limit (429 / remaining=0) | Exponential backoff; log warning; skip this poll cycle |
| Pagination error mid-poll | Log warning; use last-known-good item list for this cycle |
| `updateProjectV2ItemFieldValue` fails | Non-fatal; log warning; do not retry agent |

### 19.9 Differences from `github` Adapter

| Aspect | `github` adapter | `github-project` adapter |
|---|---|---|
| Active/terminal filter | Issue `state` (OPEN/CLOSED) | Project Status field (custom) |
| Config key | `active_states`, `terminal_states` | `active_statuses`, `terminal_statuses` |
| API | GraphQL IssueConnection | GraphQL ProjectV2 items |
| Server-side filter | Yes (state, labels) | No — client-side only |
| Reconciliation | Fetch by issue ID | Paginate all items, filter client-side |
| Status update | Not applicable | Optional `on_completion_set_status` |
| Rate limit impact | Low (filtered server-side) | Medium-high (all items fetched) |

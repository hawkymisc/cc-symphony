# Symphony

**[English](README.md)** | [日本語](docs/ja/README.md) | [中文](docs/zh/README.md) | [한국어](docs/ko/README.md)

Symphony turns project work into isolated, autonomous implementation runs, allowing teams to manage
work instead of supervising coding agents.

[![Symphony demo video preview](.github/media/symphony-demo-poster.jpg)](.github/media/symphony-demo.mp4)

_In this [demo video](.github/media/symphony-demo.mp4), Symphony monitors a Linear board for work and spawns agents to handle the tasks. The agents complete the tasks and provide proof of work: CI status, PR review feedback, complexity analysis, and walkthrough videos. When accepted, the agents land the PR safely. Engineers do not need to supervise Codex; they can manage the work at a higher level._

> [!WARNING]
> Symphony is a low-key engineering preview for testing in trusted environments.

---

## Implementations

| Implementation | Tracker | Agent | Status |
|---|---|---|---|
| [Elixir](elixir/) (reference) | Linear | Codex | Upstream original |
| [Rust](rust/) | GitHub Issues | Claude Code CLI | ✅ All phases complete |

---

## Rust Implementation (GitHub + Claude Code)

The Rust implementation connects **GitHub Issues** with the **Claude Code CLI** to automate coding tasks.

### Requirements

- Rust 1.75+
- [`claude` CLI](https://claude.ai/code) installed and authenticated
- GitHub personal access token (see [Security & Token Setup](#security--token-setup) for scopes)

### Quick Start

**1. Build and install**

```bash
cd rust
cargo install --path .
```

Alternatively, run directly from the build output:

```bash
cargo build --release
./target/release/symphony --help
```

**2. Set credentials**

```bash
export GITHUB_TOKEN=ghp_xxxxxxxxxxxx
```

**3. Create `WORKFLOW.md`**

```markdown
---
tracker:
  kind: github
  repo: "owner/your-repo"
  api_key: "$GITHUB_TOKEN"          # resolves from env at startup
  labels: ["symphony"]              # optional: only pick up issues with this label
agent:
  max_concurrent_agents: 3
polling:
  interval_ms: 30000                # poll every 30 s
---
You are a coding agent working on {{ issue.title }} (#{{ issue.identifier }}).

Repository: {{ repo }}

Issue description:
{{ issue.description }}

Please implement a solution, open a PR, and close the issue when done.
```

**4. Validate config (dry run)**

```bash
symphony ./WORKFLOW.md --dry-run
```

Expected output:
```
Config validated successfully
  Tracker: github (owner/your-repo)
  Model: claude-sonnet-4-20250514
  Max concurrent agents: 3
  Poll interval: 30000ms
```

**5. Run**

```bash
symphony ./WORKFLOW.md
```

**With HTTP observability dashboard (optional):**

```bash
cargo build --release --features http-server
./target/release/symphony ./WORKFLOW.md --port 8080
# Open http://127.0.0.1:8080 in browser
```

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Normal shutdown (SIGTERM / SIGINT) |
| 1 | Config / startup validation failure |
| 2 | CLI argument error |
| 3 | Workflow file error (missing / unreadable / invalid YAML) |

### WORKFLOW.md Reference

```yaml
---
tracker:
  kind: github               # required
  repo: "owner/repo"         # required (owner/repo format)
  api_key: "$GITHUB_TOKEN"   # required; $VAR resolves from env
  endpoint: "..."            # optional: override GitHub GraphQL URL
  labels: ["symphony"]       # optional: label filter

agent:
  max_concurrent_agents: 10  # default: 10
  max_turns: 20              # default: 20; reserved — not yet implemented
  max_retry_backoff_ms: 300000  # default: 5 min
  max_retry_queue_size: 1000    # default: 1000; oldest entry evicted when full

polling:
  interval_ms: 30000         # default: 30 s

claude:
  command: "claude"          # default: claude
  model: "claude-sonnet-4-20250514"
  max_turns_per_invocation: 50  # default: 50
  skip_permissions: false    # set true only in trusted environments
  allowed_tools:             # required when skip_permissions=false (one of this or skip_permissions: true)
    - "Bash"                 # illustrative — tailor to your workflow; Bash grants full shell access
    - "Read"
    - "Write"

workspace:
  root: "~/symphony-workspaces"  # default: $TMPDIR/symphony_workspaces

hooks:
  after_create:  "./scripts/setup.sh"    # runs once when workspace is first created
  before_run:    "./scripts/prepare.sh"  # runs before each agent invocation (fatal on failure)
  after_run:     "./scripts/cleanup.sh"  # runs after each agent invocation (non-fatal)
  before_remove: "./scripts/teardown.sh" # runs before workspace removal when issue is abandoned (non-fatal)
  timeout_ms: 60000                      # default: 60 s; applies to all hooks
---
Prompt template here. Available variables:

{{ issue.title }}        — issue title
{{ issue.identifier }}   — issue number (e.g. "42")
{{ issue.description }}  — issue body
{{ repo }}               — "owner/repo"
{{ attempt }}            — retry attempt number (1-indexed; absent on first run)
```

---

## Label Lifecycle

Symphony uses two reserved GitHub labels to track issue progress and prevent infinite re-dispatch
loops. Since GitHub Issues only have `open` / `closed` states, labels provide a lightweight signal
for the orchestrator to know when work is complete — without requiring issue closure.

Symphony は2つの予約ラベルを使い、Issue の進捗管理と無限再ディスパッチループの防止を行います。
GitHub Issues は `open` / `closed` の2状態しかないため、ラベルがオーケストレータに作業完了を
伝える軽量なシグナルとなります（Issue をクローズせずに済みます）。

| Label | Managed by | Meaning / 意味 |
|-------|-----------|----------------|
| `symphony-doing` | **Orchestrator** (automatic) | Work in progress — blocks new dispatch from other instances / 作業中 — 他インスタンスからの新規ディスパッチをブロック |
| `symphony-done` | **Agent** (via workflow) | Work complete — stops re-dispatch loop / 作業完了 — 再ディスパッチループを停止 |

**Flow / フロー:**

```
Issue created (open, no labels)
  │
  ▼
Orchestrator dispatches ──→ adds `symphony-doing`
  │
  ▼
Agent runs and completes task
  │
  ▼
Agent adds `symphony-done` ──→ Orchestrator removes `symphony-doing`
  │
  ▼
Issue remains open with `symphony-done` ──→ no re-dispatch
  │
  ▼
Human reviews and closes issue
```

To instruct the agent to add the label, include a completion protocol in your workflow template:

エージェントにラベルを付与させるには、ワークフローテンプレートに完了プロトコルを記述します:

```markdown
## Completion protocol

When your work is complete:
1. Add the `symphony-done` label:
   `gh issue edit {{ issue.identifier }} --repo owner/repo --add-label symphony-done`
2. Do NOT close the issue — a human will review and close it.
```

> **Note**: Create both labels in your repository before running Symphony.
> **注意**: Symphony を実行する前に、リポジトリに両方のラベルを作成してください。
>
> ```bash
> gh label create symphony-doing --description "Symphony: agent working" --color FBCA04
> gh label create symphony-done  --description "Symphony: agent completed" --color 0E8A16
> ```

---

## Security & Token Setup

### Token Architecture

Symphony uses a single `GITHUB_TOKEN` for two purposes:

| Consumer | Operations | Why it needs the token |
|----------|-----------|----------------------|
| **Symphony** (main process) | Issue polling via GraphQL API | Reads issues to find work |
| **Claude Code** (child process) | `git push`, `gh pr create`, `gh issue comment` | Implements changes and opens PRs |

> **Important**: The `GITHUB_TOKEN` environment variable is inherited by Claude Code child processes. This is by design — Claude Code needs it to push branches and create PRs via `gh`.

### Recommended: Fine-grained Personal Access Token

Use a [fine-grained PAT](https://github.com/settings/personal-access-tokens/new) scoped to the target repository only:

| Scope | Permission | Reason |
|-------|-----------|--------|
| **Contents** | Read and Write | `git push` from Claude Code |
| **Issues** | Read and Write | Polling (Symphony) + commenting (Claude Code) |
| **Pull Requests** | Read and Write | `gh pr create` from Claude Code |
| **Metadata** | Read-only | Auto-granted |

```bash
# Dedicated fine-grained PAT (single repo, minimal scopes)
export GITHUB_TOKEN=github_pat_xxxxxxxxxxxx
```

### Avoid: Classic PATs and `gh auth token`

| Method | Risk |
|--------|------|
| Classic PAT with `repo` scope | Grants write access to **all** repositories |
| `gh auth token` | Returns the `gh` CLI's OAuth token, often with broad org-wide scopes |

Both work for development/testing, but for unattended production use, a fine-grained PAT limits the blast radius if the token is leaked or misused by the agent.

### `skip_permissions` and Agent Sandboxing

```yaml
claude:
  skip_permissions: true   # ⚠️ Gives Claude Code full system access
```

When `skip_permissions: true`, Claude Code runs with `--dangerously-skip-permissions`, meaning it can execute arbitrary shell commands, read/write any file, and access all environment variables (including `GITHUB_TOKEN`).

**Mitigations**:
- Run Symphony in an isolated environment (container, VM, or dedicated user)
- Use a fine-grained PAT scoped to a single repository
- Set `allowed_tools` in config to restrict Claude Code's tool access (alternative to `skip_permissions`)
- The HTTP dashboard binds to `127.0.0.1` only — do not expose to the network

---

## Feature Status

### ✅ Implemented

| Feature | Notes |
|---|---|
| GitHub Issues polling | GraphQL v4, pagination, label filtering |
| Issue dispatch | FIFO-by-created_at (priority field is always null for GitHub Issues), concurrency limit, claim deduplication |
| Claude Code CLI integration | Subprocess, streaming JSON events, token tracking |
| Workspace management | Per-issue directories, hook scripts (after_create / before_run / after_run) |
| Retry with exponential backoff | Configurable cap, consecutive failure tracking |
| Graceful shutdown | SIGTERM / SIGINT → cancel-safe exit |
| Dry-run mode | `--dry-run` validates config and exits |
| Observability snapshot | `RuntimeSnapshot` via internal message channel |
| HTTP dashboard | Feature-gated (`--features http-server`); `GET /`, `GET /api/status`, `POST /api/refresh` |
| Structured logging | `tracing` (human-readable by default); issue_id + identifier in every span |
| Token aggregation | Input / output / cache-read / cache-creation tokens tracked across all sessions |
| Completed issue count | `OrchestratorState.completed_count` (u64, monotonically increasing); exposed in snapshot and dashboard |
| Workspace cleanup (`before_remove` hook) | `cleanup_workspace` called when a retried issue is found to be terminal or not found; `before_remove` hook fires before directory removal |
| Tracker failure backoff | Consecutive tracker poll failures trigger exponential backoff (capped at 5 min); non-blocking via `skip_ticks_until` |
| API key masking | `TrackerConfig` and `GitHubConfig` custom `Debug` impls replace `api_key` with `[REDACTED]` |
| Retry queue eviction | `max_retry_queue_size` (default 1000); oldest entry evicted when full, workspace cleaned up asynchronously |
| Label-based dispatch control | `symphony-doing` (auto-managed by orchestrator) and `symphony-done` (set by agent) labels prevent infinite re-dispatch loops; see [Label Lifecycle](#label-lifecycle) |

### 🔲 Not Yet Implemented

| Feature | Notes |
|---|---|
| GitHub Projects v2 | Only GitHub Issues supported; Projects v2 custom fields not yet mapped |
| HTTP dashboard auth | Dashboard binds to loopback only; no bearer token / unix socket option |
| Windows graceful shutdown | SIGTERM test is `#[cfg(unix)]`; Windows `Ctrl+C` path untested |
| Real-GitHub CI gate | Integration tests use `MemoryTracker`; no staging smoke test |
| Config hot-reload | `ConfigReloaded` message exists but doesn't re-parse WORKFLOW.md |
| Per-state concurrency limits | Global limit only; no per-label / per-project slot control |
| Priority-based dispatch | GitHub Issues have no native priority field; dispatch always falls back to oldest-first (created_at) |

---

## Running the Original Elixir Implementation

Check out [elixir/README.md](elixir/README.md) for the upstream Linear + Codex reference implementation.

---

## License

This project is licensed under the [Apache License 2.0](LICENSE).

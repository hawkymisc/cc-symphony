# Symphony

[English](../../README.md) | [日本語](../ja/README.md) | **[中文](README.md)** | [한국어](../ko/README.md)

Symphony 将项目工作转化为独立的自主实现运行，使团队能够管理工作而非监督编码代理。

[![Symphony 演示视频预览](../../.github/media/symphony-demo-poster.jpg)](../../.github/media/symphony-demo.mp4)

_在这个[演示视频](../../.github/media/symphony-demo.mp4)中，Symphony 监控 Linear 看板上的工作并生成代理来处理任务。代理完成任务并提供工作证明：CI 状态、PR 审查反馈、复杂度分析和演练视频。批准后，代理安全地合并 PR。工程师无需监督 Codex，可以在更高层次管理工作。_

> [!WARNING]
> Symphony 是一个用于在可信环境中测试的低调工程预览版。

---

## 实现列表

| 实现 | 追踪器 | 代理 | 状态 |
|------|--------|------|------|
| [Elixir](../../elixir/)（参考实现） | Linear | Codex | 上游原始版本 |
| [Rust](../../rust/) | GitHub Issues | Claude Code CLI | ✅ 所有阶段完成 |

---

## Rust 实现（GitHub + Claude Code）

Rust 实现将 **GitHub Issues** 与 **Claude Code CLI** 连接，实现编码任务自动化。

### 系统要求

- Rust 1.75+
- [`claude` CLI](https://claude.ai/code) 已安装并完成认证
- GitHub 个人访问令牌（作用域请参阅[安全与令牌设置](#安全与令牌设置)）

### 快速开始

**1. 构建和安装**

```bash
cd rust
cargo install --path .
```

或者直接从构建输出运行：

```bash
cargo build --release
./target/release/symphony --help
```

**2. 设置凭据**

```bash
export GITHUB_TOKEN=ghp_xxxxxxxxxxxx
```

**3. 创建 `WORKFLOW.md`**

```markdown
---
tracker:
  kind: github
  repo: "owner/your-repo"
  api_key: "$GITHUB_TOKEN"          # 启动时从环境变量解析
  labels: ["symphony"]              # 可选：仅获取带有此标签的 Issue
agent:
  max_concurrent_agents: 3
polling:
  interval_ms: 30000                # 每 30 秒轮询一次
---
You are a coding agent working on {{ issue.title }} (#{{ issue.identifier }}).

Repository: {{ repo }}

Issue description:
{{ issue.description }}

Please implement a solution, open a PR, and close the issue when done.
```

**4. 验证配置（试运行）**

```bash
symphony ./WORKFLOW.md --dry-run
```

预期输出：
```
Config validated successfully
  Tracker: github (owner/your-repo)
  Model: claude-sonnet-4-20250514
  Max concurrent agents: 3
```

**5. 运行**

```bash
symphony ./WORKFLOW.md
```

**附带 HTTP 可观测性仪表板（可选）：**

```bash
cargo build --release --features http-server
./target/release/symphony ./WORKFLOW.md --port 8080
# 在浏览器中打开 http://127.0.0.1:8080
```

### 退出码

| 代码 | 含义 |
|------|------|
| 0 | 正常关闭（SIGTERM / SIGINT） |
| 1 | 配置 / 启动验证失败 |
| 2 | CLI 参数错误 |
| 3 | 工作流文件错误（未找到 / 不可读 / 无效 YAML） |

### WORKFLOW.md 参考

```yaml
---
tracker:
  kind: github               # 必填
  repo: "owner/repo"         # 必填（owner/repo 格式）
  api_key: "$GITHUB_TOKEN"   # 必填；$VAR 从环境变量解析
  endpoint: "..."            # 可选：覆盖 GitHub GraphQL URL
  labels: ["symphony"]       # 可选：标签过滤器

agent:
  max_concurrent_agents: 10  # 默认：10
  max_retry_backoff_ms: 300000  # 默认：5 分钟

polling:
  interval_ms: 30000         # 默认：30 秒

claude:
  command: "claude"          # 默认：claude
  model: "claude-sonnet-4-20250514"
  max_turns_per_invocation: 50
  skip_permissions: false    # 仅在可信环境中设置为 true

workspace:
  root: "~/symphony-workspaces"  # 默认：$TMPDIR/symphony_workspaces

hooks:
  after_create:  "./scripts/setup.sh"    # 工作区首次创建时运行一次
  before_run:    "./scripts/prepare.sh"  # 每次代理调用前运行（失败时致命）
  after_run:     "./scripts/cleanup.sh"  # 每次代理调用后运行（非致命）
  before_remove: "./scripts/teardown.sh" # Issue 放弃时工作区删除前运行（非致命）
  timeout_ms: 60000                      # 默认：60 秒；适用于所有钩子
---
提示模板。可用变量：

{{ issue.title }}        — Issue 标题
{{ issue.identifier }}   — Issue 编号（例如 "42"）
{{ issue.description }}  — Issue 正文
{{ repo }}               — "owner/repo"
{{ attempt }}            — 重试次数（从 1 开始；首次运行时不存在）
```

---

## 安全与令牌设置

### 令牌架构

Symphony 将单个 `GITHUB_TOKEN` 用于两个目的：

| 使用者 | 操作 | 需要令牌的原因 |
|--------|------|---------------|
| **Symphony**（主进程） | 通过 GraphQL API 轮询 Issue | 读取 Issue 以查找工作 |
| **Claude Code**（子进程） | `git push`、`gh pr create`、`gh issue comment` | 实现更改并创建 PR |

> **重要**：`GITHUB_TOKEN` 环境变量会被 Claude Code 子进程继承。这是设计如此 — Claude Code 需要它来推送分支和通过 `gh` 创建 PR。

### 推荐：Fine-grained 个人访问令牌

使用仅限于目标仓库的 [Fine-grained PAT](https://github.com/settings/personal-access-tokens/new)：

| 作用域 | 权限 | 原因 |
|--------|------|------|
| **Contents** | Read and Write | Claude Code 的 `git push` |
| **Issues** | Read and Write | 轮询（Symphony）+ 评论（Claude Code） |
| **Pull Requests** | Read and Write | Claude Code 的 `gh pr create` |
| **Metadata** | Read-only | 自动授予 |

```bash
# 专用 Fine-grained PAT（单个仓库，最小作用域）
export GITHUB_TOKEN=github_pat_xxxxxxxxxxxx
```

### 避免使用：Classic PAT 和 `gh auth token`

| 方法 | 风险 |
|------|------|
| 带 `repo` 作用域的 Classic PAT | 授予对**所有**仓库的写入权限 |
| `gh auth token` | 返回 `gh` CLI 的 OAuth 令牌，通常具有广泛的组织范围作用域 |

两者都适用于开发/测试，但对于无人值守的生产使用，Fine-grained PAT 可以在令牌泄露或被代理滥用时限制影响范围。

### `skip_permissions` 与代理沙箱

```yaml
claude:
  skip_permissions: true   # ⚠️ 赋予 Claude Code 完全系统访问权限
```

当 `skip_permissions: true` 时，Claude Code 以 `--dangerously-skip-permissions` 运行，意味着它可以执行任意 shell 命令、读写任意文件，并访问所有环境变量（包括 `GITHUB_TOKEN`）。

**缓解措施**：
- 在隔离环境中运行 Symphony（容器、虚拟机或专用用户）
- 使用限定单个仓库的 Fine-grained PAT
- 在配置中设置 `allowed_tools` 来限制 Claude Code 的工具访问（`skip_permissions` 的替代方案）
- HTTP 仪表板仅绑定到 `127.0.0.1` — 请勿暴露到网络

---

## 功能状态

### ✅ 已实现

| 功能 | 备注 |
|------|------|
| GitHub Issues 轮询 | GraphQL v4，分页，标签过滤 |
| Issue 调度 | FIFO-by-created_at（GitHub Issues 的优先级字段始终为 null），并发限制，声明去重 |
| Claude Code CLI 集成 | 子进程，流式 JSON 事件，令牌追踪 |
| 工作区管理 | 每个 Issue 的目录，钩子脚本（after_create / before_run / after_run） |
| 指数退避重试 | 可配置上限，连续失败追踪 |
| 优雅关闭 | SIGTERM / SIGINT → 取消安全退出 |
| 试运行模式 | `--dry-run` 验证配置并退出 |
| 可观测性快照 | 通过内部消息通道的 `RuntimeSnapshot` |
| HTTP 仪表板 | Feature-gated（`--features http-server`）；`GET /`、`GET /api/status`、`POST /api/refresh` |
| 结构化日志 | `tracing`（默认人类可读格式）；每个 span 包含 issue_id + identifier |
| 令牌聚合 | 跨所有会话追踪输入 / 输出 / 缓存读取 / 缓存创建令牌 |
| 已完成 Issue 计数 | `OrchestratorState.completed_count`（u64，单调递增）；在快照和仪表板中公开 |
| 工作区清理（`before_remove` 钩子） | 当重试中的 Issue 为 terminal 或未找到时调用 `cleanup_workspace`；目录删除前触发 `before_remove` 钩子 |
| 追踪器故障退避 | 连续追踪器轮询故障触发指数退避（上限 5 分钟）；通过 `skip_ticks_until` 非阻塞 |
| API 密钥掩码 | `TrackerConfig` 和 `GitHubConfig` 的自定义 `Debug` 实现将 `api_key` 替换为 `[REDACTED]` |
| 重试队列驱逐 | `max_retry_queue_size`（默认 1000）；满时驱逐最旧条目，工作区异步清理 |

### 🔲 未实现

| 功能 | 备注 |
|------|------|
| GitHub Projects v2 | 仅支持 GitHub Issues；Projects v2 自定义字段尚未映射 |
| HTTP 仪表板认证 | 仪表板仅绑定到回环地址；无 Bearer 令牌 / UNIX 套接字选项 |
| Windows 优雅关闭 | SIGTERM 测试为 `#[cfg(unix)]`；Windows `Ctrl+C` 路径未测试 |
| 真实 GitHub CI 门控 | 集成测试使用 `MemoryTracker`；无暂存环境冒烟测试 |
| 配置热重载 | `ConfigReloaded` 消息存在但不会重新解析 WORKFLOW.md |
| 按状态并发限制 | 仅全局限制；无按标签 / 按项目的插槽控制 |
| 基于优先级的调度 | GitHub Issues 没有原生优先级字段；调度始终回退到最旧优先（created_at） |

---

## 运行原始 Elixir 实现

请参阅 [elixir/README.md](../../elixir/README.md) 了解上游 Linear + Codex 参考实现。

---

## 许可证

本项目基于 [Apache License 2.0](../../LICENSE) 许可。

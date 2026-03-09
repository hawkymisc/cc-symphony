# TASK.md — Symphony Rust 実装 Phase 9: 堅牢化・リファクタリング・テスト改善

**作成日**: 2026-03-09
**ベースコミット**: 1caeb05 (main)
**テスト総数**: 167+
**根拠**: `docs/technical-review-2026-03-09.md` の包括的技術レビュー結果

---

## 概要

技術レビューで検出された改善点を3つのサブフェーズに分けて対応する。
Phase 10 (GitHub Projects v2) の開始前にすべて完了させる。

- **Phase 9A: 堅牢化 (Hardening)** — HIGH 優先度の運用耐性向上
- **Phase 9B: リファクタリング (Refactoring)** — MEDIUM 優先度の設計改善
- **Phase 9C: テスト改善 (Test Improvements)** — テストカバレッジ・保守性向上

---

## Phase 9A: 堅牢化 (Hardening)

**入口基準**: main ブランチの全テスト（167+）がパスしていること
**出口基準**: H-1, H-2, H-3 のすべてが実装・テスト済み、全テストパス

### ✅ 9A-001: 連続トラッカー障害のバックオフ実装

| 項目 | 内容 |
|------|------|
| **優先度** | HIGH |
| **複雑度** | M |
| **依存** | なし |
| **変更ファイル** | `rust/src/orchestrator/mod.rs`, `rust/src/orchestrator/state.rs`, `rust/tests/orchestrator_test.rs` |

**問題**: `handle_tick()` (L194-218) でトラッカーエラーが発生した場合、`warn!` ログのみで次の tick でも同じエラーが繰り返される。GitHub API 障害時に30秒ごとの無限ポーリングとなり、レート制限を浪費する。

**実装詳細**:

1. `OrchestratorState` に `consecutive_tracker_failures: u32` フィールドを追加する（`rust/src/orchestrator/state.rs` L83付近）
2. `handle_tick()` の `Ok(candidates)` 成功パスで `state.consecutive_tracker_failures = 0` にリセットする
3. `handle_tick()` の `Err(e)` パスで以下を実装する:
   - `state.consecutive_tracker_failures += 1`
   - バックオフ計算: `min(poll_interval_ms * 2^(failures-1), 300_000)` （最大5分）
   - `tokio::time::sleep(Duration::from_millis(backoff))` を挿入
   - `warn!` ログにバックオフ秒数と連続失敗回数を含める
4. `TrackerError::RateLimited` の場合は `retry_after_seconds` をバックオフとして使用する

**受入基準**:
- [ ] 連続トラッカー障害でバックオフが指数的に増加する
- [ ] 成功ポーリングで `consecutive_tracker_failures` が0にリセットされる
- [ ] `RateLimited` エラーは `retry_after_seconds` を尊重する
- [ ] バックオフは300秒（5分）を超えない
- [ ] テスト3件以上追加

**TDDアプローチ**:
```
RED: test_tracker_failure_backoff_increments — 連続失敗でバックオフが増加
RED: test_tracker_failure_backoff_resets_on_success — 成功後リセット
RED: test_tracker_failure_backoff_capped — 最大値を超えない
RED: test_tracker_rate_limited_uses_retry_after — RateLimited の retry_after を使用
```

---

### ✅ 9A-002: API トークンの Debug 実装マスク

| 項目 | 内容 |
|------|------|
| **優先度** | HIGH |
| **複雑度** | S |
| **依存** | なし |
| **変更ファイル** | `rust/src/config.rs`, `rust/src/tracker/github.rs`, `rust/tests/domain_test.rs` (または新規テスト) |

**問題**: `TrackerConfig` は `#[derive(Debug)]` を使用しており、`api_key` フィールドがログ出力時に平文で表示される。`GitHubConfig` の `api_key` も同様。

**実装詳細**:

1. `TrackerConfig` (L64) の `#[derive(Debug)]` を手動 `impl Debug` に置換する:
   - `api_key` フィールドを `"[REDACTED]"` と表示する
   - 他のフィールドは通常通り表示する
2. `GitHubConfig` (L32, `tracker/github.rs`) の `#[derive(Debug)]` も同様に置換する:
   - `api_key` フィールドを `"[REDACTED]"` と表示する
3. `ClaudeConfig` (L200, `config.rs`) は現時点でトークンを直接保持しないため変更不要

**受入基準**:
- [ ] `TrackerConfig` の `Debug` 出力に `api_key` の実値が含まれない
- [ ] `GitHubConfig` の `Debug` 出力に `api_key` の実値が含まれない
- [ ] `format!("{:?}", config)` でトークン値が `[REDACTED]` に置換される
- [ ] 既存テストが引き続きパスする
- [ ] テスト2件追加

**TDDアプローチ**:
```
RED: test_tracker_config_debug_masks_api_key — Debug 出力に api_key の値が含まれない
RED: test_github_config_debug_masks_api_key — Debug 出力に api_key の値が含まれない
```

---

### ✅ 9A-003: `is_blocked()` ユニットテスト拡充

| 項目 | 内容 |
|------|------|
| **優先度** | HIGH |
| **複雑度** | S |
| **依存** | なし |
| **変更ファイル** | `rust/src/domain/issue.rs` |

**問題**: `is_blocked()` はビジネスクリティカルなロジック（ブロッカーが存在するIssueの dispatch をスキップする）だが、テストが1件しかない（L138-157）。エッジケースが未検証。

**現状テスト**: `issue_is_blocked_checks_blockers` — ブロッカーなし、非アクティブブロッカー1件、アクティブブロッカー1件の3パターンのみ

**追加テストケース**:

1. **アクティブブロッカー複数**: 3件のアクティブブロッカーがある場合 `is_blocked() == true`
2. **全ブロッカー非アクティブ**: 5件すべて `is_active: false` → `is_blocked() == false`
3. **混合 (非アクティブ多数 + アクティブ1件)**: 10件のうち1件だけアクティブ → `is_blocked() == true`
4. **空のブロッカーリスト**: `blocked_by: vec![]` → `is_blocked() == false`（既存テストと重複だが明示的に分離）
5. **`is_blocked()` と `is_active()` の組み合わせ**: closedかつblockedの場合、両方のメソッドが正しく動作する

**受入基準**:
- [ ] `is_blocked()` に対するテストが5件以上存在する
- [ ] エッジケース（複数ブロッカー、全非アクティブ、混合）がカバーされている
- [ ] `is_blocked()` と `is_active()` / `is_terminal()` の組み合わせテストがある

**TDDアプローチ**:
このタスクはテスト追加のみ。既存ロジックの正しさを検証する形で、RED→GREENではなくGREENの確認を行う。ただし、もしバグが見つかった場合はREDから修正する。

---

## Phase 9B: リファクタリング (Refactoring)

**入口基準**: Phase 9A の全タスクが完了していること
**出口基準**: M-1, M-2, M-3, M-4 のすべてが実装・テスト済み、全テストパス

### ✅ 9B-001: `AgentRunConfig` 構造体の導入

| 項目 | 内容 |
|------|------|
| **優先度** | MEDIUM |
| **複雑度** | M |
| **依存** | なし |
| **変更ファイル** | `rust/src/agent/mod.rs`, `rust/src/agent/claude.rs`, `rust/src/orchestrator/mod.rs`, `rust/tests/agent_test.rs`, `rust/tests/orchestrator_test.rs`, `rust/tests/integration_test.rs` |

**問題**: `AgentRunner::run()` の第3引数が `config: &AppConfig` であり、エージェント実行に不要な設定（`tracker`, `polling`, `hooks` など）まで渡している。これは最小特権の原則に反し、テスト時にも不要な設定を構築する必要がある。

**実装詳細**:

1. `rust/src/agent/mod.rs` に `AgentRunConfig` 構造体を定義する:
   ```rust
   pub struct AgentRunConfig {
       pub workspace_root: PathBuf,
       pub prompt_template: String,
       pub repo: String,
       pub claude: ClaudeConfig,  // command, model, skip_permissions, etc.
   }
   ```
2. `AppConfig` に `fn to_agent_run_config(&self) -> AgentRunConfig` メソッドを追加する（`config.rs`）
3. `AgentRunner` trait の `run()` シグネチャを変更する:
   - Before: `config: &AppConfig`
   - After: `config: &AgentRunConfig`
4. `ClaudeRunner::run()` 実装を `AgentRunConfig` を使用するよう更新する（`agent/claude.rs`）
5. `orchestrator/mod.rs` の `dispatch_issue()` (L221付近) で `config.to_agent_run_config()` を呼び出して渡す
6. テストファイルの `MockAgent` 実装を更新する

**受入基準**:
- [ ] `AgentRunner::run()` が `AppConfig` ではなく `AgentRunConfig` を受け取る
- [ ] `AgentRunConfig` にはエージェント実行に必要な設定のみが含まれる
- [ ] `AppConfig::to_agent_run_config()` メソッドが存在する
- [ ] 全テストパス
- [ ] テストの `make_config()` が簡素化される

**TDDアプローチ**:
```
RED: test_agent_run_config_contains_required_fields — AgentRunConfig の全フィールドが設定される
RED: test_app_config_to_agent_run_config — AppConfig から AgentRunConfig への変換が正しい
GREEN: 構造体定義と変換メソッドを実装
REFACTOR: 既存テストを AgentRunConfig に移行
```

---

### ✅ 9B-002: テスト共有ヘルパーの集約

| 項目 | 内容 |
|------|------|
| **優先度** | MEDIUM |
| **複雑度** | S |
| **依存** | 9B-001（AgentRunConfig 導入後に make_config が変わるため） |
| **変更ファイル** | `rust/tests/common/mod.rs`（新規作成）, `rust/tests/agent_test.rs`, `rust/tests/orchestrator_test.rs`, `rust/tests/tracker_test.rs`, `rust/tests/integration_test.rs`, `rust/tests/observability_test.rs` |

**問題**: `make_config()` 関数が `agent_test.rs:21`, `orchestrator_test.rs:80`, `tracker_test.rs:10` にそれぞれ異なるシグネチャで重複定義されている。テスト用ヘルパー関数の保守コストが高い。

**実装詳細**:

1. `rust/tests/common/mod.rs` を新規作成する
2. 以下の共通ヘルパーを `common` モジュールに集約する:
   - `make_app_config()` — デフォルトの `AppConfig` を生成（tracker.api_key, tracker.repo を設定済み）
   - `make_app_config_with_concurrency(max: usize)` — 並行数指定版
   - `make_github_config(server_uri: &str, labels: Vec<String>)` — GitHubConfig 生成
   - `make_test_issue(id: &str, identifier: &str)` — テスト用 Issue 生成
   - `make_test_issue_with_priority(id: &str, identifier: &str, priority: Option<i32>)` — 優先度付き Issue
3. 各テストファイルの `make_config()` を `common::make_app_config()` 等に置換する
4. 各テストファイルの先頭に `mod common;` を追加する

**受入基準**:
- [ ] `rust/tests/common/mod.rs` が存在する
- [ ] 3つ以上のテストファイルが `common` モジュールを使用している
- [ ] テストファイル内に `make_config()` の重複定義がない
- [ ] 全テストパス

**TDDアプローチ**:
リファクタリングのみ。テストが GREEN のまま移行する。RED が発生した場合はリグレッションバグとして修正する。

---

### ✅ 9B-003: リトライキューのサイズ上限

| 項目 | 内容 |
|------|------|
| **優先度** | MEDIUM |
| **複雑度** | S |
| **依存** | なし |
| **変更ファイル** | `rust/src/orchestrator/state.rs`, `rust/src/orchestrator/mod.rs`, `rust/src/config.rs`, `rust/tests/orchestrator_test.rs` |

**問題**: `OrchestratorState.retry_attempts: HashMap<String, RetryEntry>` にサイズ上限がない。長期運用で大量のIssueが失敗・リトライを繰り返すとメモリが際限なく増加する。

**実装詳細**:

1. `AgentConfig` に `max_retry_queue_size: usize` フィールドを追加する（デフォルト: 1000）
   - `config.rs` L164付近の `AgentConfig` struct に追加
   - `default_max_retry_queue_size()` 関数を追加
2. `OrchestratorState` に `max_retry_queue_size: usize` フィールドを追加する
3. `handle_worker_finished()` (L315付近) でリトライエントリを挿入する前にサイズチェックする:
   - `state.retry_attempts.len() >= state.max_retry_queue_size` の場合
   - 最も古い（`due_at` が最も過去の）エントリを削除する
   - `warn!("Retry queue full ({} entries), evicting oldest entry for issue {}", ...)` をログ出力する
   - エビクションされたエントリの `claimed` を解放する
4. `handle_retry()` (L423付近) でも同様のサイズチェックを行う

**受入基準**:
- [ ] `AgentConfig.max_retry_queue_size` が設定可能（デフォルト1000）
- [ ] リトライキューがサイズ上限に達した場合、最も古いエントリがエビクションされる
- [ ] エビクションされたエントリの `claimed` が解放される
- [ ] エビクション時に `warn!` ログが出力される
- [ ] テスト2件追加

**TDDアプローチ**:
```
RED: test_retry_queue_evicts_oldest_when_full — キュー満杯時に最古エントリがエビクションされる
RED: test_retry_queue_eviction_releases_claim — エビクション時に claimed が解放される
```

---

### ✅ 9B-004: ページネーション上限超過の警告

| 項目 | 内容 |
|------|------|
| **優先度** | MEDIUM |
| **複雑度** | S |
| **依存** | なし |
| **変更ファイル** | `rust/src/tracker/github.rs`, `rust/tests/tracker_test.rs` |

**問題**: `fetch_issues_paginated()` (L214付近) で `MAX_PAGES` (10ページ = 500 Issues) に到達した場合、`warn!` ログのみで呼び出し元には正常な結果が返される。呼び出し元は結果が切り捨てられたことを知る手段がない。

**実装詳細**:

1. `warn!` メッセージを改善し、取得済み件数を含める
   - Before: `warn!("Reached maximum pages ({}) during fetch", MAX_PAGES)`
   - After: `warn!("Pagination limit reached: fetched {} issues across {} pages (max {}). Some issues may be omitted. Consider using label filters to reduce result set.", all_issues.len(), MAX_PAGES, MAX_PAGES * DEFAULT_PAGE_SIZE)`

**受入基準**:
- [ ] 500+ Issues で警告ログに取得済み件数と推定上限が含まれる
- [ ] 警告ログにラベルフィルター使用の推奨が含まれる
- [ ] テスト1件追加

**TDDアプローチ**:
```
RED: test_pagination_limit_warning_includes_count — MAX_PAGES 到達時の warn ログに件数が含まれる
```

---

### ✅ 9B-005: コード品質の軽微修正 (3件)

| 項目 | 内容 |
|------|------|
| **優先度** | LOW |
| **複雑度** | S |
| **依存** | なし |
| **変更ファイル** | `rust/src/config.rs`, `rust/src/tracker/github.rs`, `rust/src/orchestrator/mod.rs` |

**修正1: `config.rs:387-389` 文字列連結の簡素化**

現在のコード（`resolve_env_var()` 内）:
```rust
let before = result[..abs_pos].to_string();
let after = result[abs_pos + 1 + var_end..].to_string();
result = format!("{}{}{}", before, var_value, after);
```

改善: 中間変数の `.to_string()` は `format!` 内のスライス参照で十分:
```rust
result = format!("{}{}{}", &result[..abs_pos], var_value, &result[abs_pos + 1 + var_end..]);
```
注意: `result` への再代入前にスライスを取るため、borrow checker に注意。一時変数が必要な場合はそのまま維持する。

**修正2: `tracker/github.rs:301-310` `split_once('/')` の使用**

現在のコード:
```rust
fn parse_repo(&self) -> Result<(&str, &str), TrackerError> {
    let parts: Vec<&str> = self.config.repo.split('/').collect();
    if parts.len() != 2 {
        return Err(TrackerError::ApiRequest(...));
    }
    Ok((parts[0], parts[1]))
}
```

改善:
```rust
fn parse_repo(&self) -> Result<(&str, &str), TrackerError> {
    self.config.repo.split_once('/').ok_or_else(|| {
        TrackerError::ApiRequest(format!(
            "Invalid repo format: {}. Expected owner/repo",
            self.config.repo
        ))
    })
}
```

注意: `split_once` は `owner/repo/extra` の場合に `("owner", "repo/extra")` を返す。`config.validate()` で事前チェック済みのため安全だが、防御的にガードを追加することも検討する。

**修正3: `orchestrator/mod.rs:340` `attempt: 0` の定数化**

```rust
/// Retry attempt value indicating a successful (normal) exit.
const NORMAL_EXIT_ATTEMPT: u32 = 0;
```

**受入基準**:
- [ ] 3件すべて修正済み
- [ ] `split_once` 使用時に `"owner/repo/extra"` のようなケースが正しく処理される
- [ ] 全テストパス

**TDDアプローチ**:
リファクタリングのみ。`split_once` 変更については既存 `test_parse_repo` と新規 `test_parse_repo_with_extra_slash` (9C-005) でカバーする。

---

### ✅ 9B-006: Clippy 警告の解消 (8件)

| 項目 | 内容 |
|------|------|
| **優先度** | LOW |
| **複雑度** | S |
| **依存** | なし |
| **変更ファイル** | `rust/src/prompt.rs`, `rust/src/workflow.rs`, `rust/tests/observability_test.rs`, `rust/tests/orchestrator_test.rs`, `rust/tests/domain_test.rs`, `rust/tests/cli_test.rs` |

**修正一覧**:

1. `src/prompt.rs:125` — 未使用 import `use chrono::Utc;` を `#[cfg(test)]` 内に移動
2. `src/workflow.rs:173-174` — needless borrows を削除
3. `tests/observability_test.rs:190` — needless borrows を削除
4. `tests/orchestrator_test.rs:621, 685` — unused `tx` 変数を `_tx` にリネーム
5. `tests/domain_test.rs:6, 13` — 未使用 import を削除
6. `tests/cli_test.rs:27, 203` — deprecated `cargo_bin` を新しいAPIに移行

**受入基準**:
- [ ] `cargo clippy --all-features -- -W clippy::all` が警告ゼロ
- [ ] 全テストパス

---

## Phase 9C: テスト改善 (Test Improvements)

**入口基準**: Phase 9B の 9B-001, 9B-002 が完了していること（テストヘルパー集約後のほうが効率的）
**出口基準**: 全タスク完了、テスト総数 185+ 以上、全テストパス

### ✅ 9C-001: `workflow.rs` フロントマター解析のエッジケーステスト

| 項目 | 内容 |
|------|------|
| **優先度** | MEDIUM |
| **複雑度** | S |
| **依存** | なし |
| **変更ファイル** | `rust/src/workflow.rs` |

**追加テストケース**:

1. **BOM 付き UTF-8**: `\u{FEFF}---\nkey: value\n---\nPrompt` — BOM がフロントマター検出を阻害しないか
2. **CRLF 改行**: `---\r\nkey: value\r\n---\r\nPrompt` — Windows 改行で正しく解析されるか
3. **フロントマター内の `---` 行**: `---\nkey: "a---b"\n---\nPrompt` — 値に含まれる `---` が閉じタグと誤認されないか
4. **空白のみの行がフロントマターと本文の間にある場合**: `---\nkey: value\n---\n   \nPrompt`
5. **フロントマターの `---` 前後に空白がある場合**: `  ---  \nkey: value\n  ---  \nPrompt`

**受入基準**:
- [ ] 5件以上のエッジケーステストが追加されている
- [ ] BOM付きファイルが正しく処理される（または明示的にエラーとなる）
- [ ] CRLF 改行が正しく処理される
- [ ] フロントマター閉じタグの誤検出が発生しない

**TDDアプローチ**:
```
RED: test_workflow_bom_utf8 — BOM付きUTF-8ファイルの解析
RED: test_workflow_crlf_line_endings — CRLF改行での解析
RED: test_workflow_triple_dash_in_value — 値内の --- がフロントマターに影響しない
RED: test_workflow_whitespace_between_sections — 空白行の処理
RED: test_workflow_indented_delimiters — インデントされた区切り線の処理
```

---

### ✅ 9C-002: `orchestrator_test.rs` の3ファイル分割

| 項目 | 内容 |
|------|------|
| **優先度** | MEDIUM |
| **複雑度** | M |
| **依存** | 9B-002（テストヘルパー集約後が効率的） |
| **変更ファイル** | `rust/tests/orchestrator_test.rs`（分割元）, `rust/tests/orchestrator_dispatch_test.rs`（新規）, `rust/tests/orchestrator_retry_test.rs`（新規）, `rust/tests/orchestrator_state_test.rs`（新規） |

**分割方針**:

1. **`orchestrator_dispatch_test.rs`** — Issue ディスパッチ関連テスト
   - `test_dispatch_*` 系テスト
   - `test_reconcile_*` 系テスト
   - 対象: `select_candidates()`, ディスパッチ順序、並行数制限

2. **`orchestrator_retry_test.rs`** — リトライ関連テスト
   - `test_retry_*` 系テスト
   - バックオフ計算、リトライキュー、連続失敗

3. **`orchestrator_state_test.rs`** — 状態管理・スナップショット関連テスト
   - `test_snapshot_*` 系テスト
   - `test_agent_update_*` 系テスト
   - `test_worker_finished_*` 系テスト

**受入基準**:
- [ ] `orchestrator_test.rs` が削除され、3つのファイルに分割されている
- [ ] 各ファイルが300行以下
- [ ] 共通ヘルパーは `tests/common/mod.rs` を使用している
- [ ] 全テストパス、テスト数は分割前と同一

---

### ✅ 9C-003: ディスパッチロジックの `is_blocked()` 統合テスト

| 項目 | 内容 |
|------|------|
| **優先度** | MEDIUM |
| **複雑度** | S |
| **依存** | 9A-003 |
| **変更ファイル** | `rust/tests/orchestrator_dispatch_test.rs`（または既存テストファイル） |

**問題**: `select_candidates()` で `is_blocked()` がフィルター条件として使用されている（`dispatch.rs:42`）が、ブロッカー付き Issue のディスパッチスキップをテストする統合テストが存在しない。

**追加テストケース**:

1. ブロッカーが1件でもアクティブな Issue はディスパッチ対象から除外される
2. 全ブロッカーが非アクティブな Issue はディスパッチ対象に含まれる
3. ブロッカーが解除された Issue が次の tick でディスパッチされる

**受入基準**:
- [ ] `select_candidates()` のブロッカーフィルターが統合テストでカバーされている
- [ ] テスト3件追加

---

### ✅ 9C-004: `http_server_test.rs` のフレイキーテスト対策

| 項目 | 内容 |
|------|------|
| **優先度** | LOW |
| **複雑度** | S |
| **依存** | なし |
| **変更ファイル** | `rust/tests/http_server_test.rs` |

**問題**: `tokio::time::sleep(Duration::from_millis(50))` を使用したテストがCI環境で不安定化するリスクがある。

**実装詳細**:

1. `sleep(50ms)` を使用しているテストを特定する
2. 可能な場合は `start_paused = true` とポーリングベースの確認に置換する
3. ポーリングが不可能な場合は sleep の値を 200ms 以上に引き上げ、CI 環境でのマージンを確保する
4. テスト名にコメントで「CI stability」を記載する

**受入基準**:
- [ ] `sleep(50ms)` が存在しない（200ms 以上またはポーリングベースに置換）
- [ ] 全テストパス
- [ ] CI 環境で10回連続実行してフレイキーが発生しない

---

### ✅ 9C-005: `split_once` 変更に伴う `parse_repo` エッジケーステスト

| 項目 | 内容 |
|------|------|
| **優先度** | LOW |
| **複雑度** | S |
| **依存** | 9B-005 |
| **変更ファイル** | `rust/src/tracker/github.rs` |

**追加テストケース**:

1. `"owner/repo"` → `Ok(("owner", "repo"))`（既存テスト）
2. `"invalid"` → `Err`（既存テスト）
3. `"owner/repo/extra"` → 動作を明確化（エラーにするか、許容するか決定）
4. `"/repo"` → エラー（空の owner）
5. `"owner/"` → エラー（空の repo）

**受入基準**:
- [ ] 上記5パターンのテストが存在する
- [ ] 空の owner/repo が適切にエラーとなる

---

## LOW 優先度タスク (Phase 10 以降で検討)

以下は Phase 9 のスコープ外だが、記録として残す。

### 🔲 9X-001: `shellexpand` crate 導入

| 項目 | 内容 |
|------|------|
| **優先度** | LOW |
| **複雑度** | S |
| **変更ファイル** | `rust/Cargo.toml`, `rust/src/config.rs` |

`config.rs` の `expand_paths()` と `resolve_env_var()` で手動実装している `~` と `$VAR` の展開を `shellexpand` crate に置換する。

### 🔲 9X-002: tokio features の最小化

| 項目 | 内容 |
|------|------|
| **優先度** | LOW |
| **複雑度** | S |
| **変更ファイル** | `rust/Cargo.toml` |

`tokio = { version = "1", features = ["full"] }` を個別 features に変更: `rt-multi-thread`, `macros`, `time`, `signal`, `process`, `io-util`, `sync`, `fs`。コンパイル時間短縮が期待できる。

### 🔲 9X-003: Property-based テスト (proptest)

| 項目 | 内容 |
|------|------|
| **優先度** | LOW |
| **複雑度** | M |
| **変更ファイル** | `rust/Cargo.toml`, `rust/tests/property_test.rs`（新規） |

`proptest` crate を導入し、以下のプロパティテストを追加:
- `sanitized_identifier()` — 任意の入力文字列でパニックしない、出力にパス区切り文字が含まれない
- `compute_backoff()` — 任意の attempt 値で `max_backoff_ms` を超えない
- `resolve_env_var()` — 任意の入力でパニックしない

### 🔲 9X-004: コードカバレッジ CI 統合

| 項目 | 内容 |
|------|------|
| **優先度** | LOW |
| **複雑度** | M |
| **変更ファイル** | `.github/workflows/ci.yml`, `rust/Cargo.toml` |

`cargo-llvm-cov` または `cargo-tarpaulin` を CI パイプラインに統合し、カバレッジレポートを生成する。

---

## リスクアセスメント

| リスク | 影響度 | 発生確率 | 対策 |
|--------|--------|----------|------|
| **9B-001 のトレイト変更による大規模な破壊的変更** | HIGH | MEDIUM | 9B-001 → 9B-002 の順序で、トレイト変更後にヘルパー集約して二度手間を回避 |
| **9A-001 のバックオフがイベントループをブロック** | MEDIUM | LOW | `sleep` の代わりに `interval.reset_after()` でインターバルを動的変更する方法も検討 |
| **9C-002 のテスト分割で意図しないテスト漏れ** | MEDIUM | LOW | 分割前後で `cargo test` の出力を diff し、テスト名が全て一致することを確認 |
| **9B-005 の `split_once` で `owner/repo/extra` が通過** | LOW | MEDIUM | `config.validate()` が事前に拒否済みだが、防御的ガードも追加 |
| **9A-002 のカスタム Debug 実装漏れ** | MEDIUM | LOW | `#[deny(missing_debug_implementations)]` を `lib.rs` に追加することも検討 |

---

## 作業順序サマリー

```
Phase 9A (並列可能):
  9A-001 ──┐
  9A-002 ──┤── すべて独立
  9A-003 ──┘

Phase 9B (一部依存あり):
  9B-001 (AgentRunConfig) ──→ 9B-002 (テストヘルパー集約)
  9B-003 (リトライ上限) ────── 独立
  9B-004 (ページネーション警告) ── 独立
  9B-005 (コード品質) ──→ 9C-005 (parse_repo テスト)
  9B-006 (Clippy) ────── 独立

Phase 9C (一部依存あり):
  9C-001 (workflow エッジケース) ── 独立
  9C-002 (テスト分割) ──→ 9B-002 に依存
  9C-003 (is_blocked 統合テスト) ──→ 9A-003 に依存
  9C-004 (フレイキー対策) ── 独立
  9C-005 (parse_repo テスト) ──→ 9B-005 に依存
```

---

## 推定工数

| フェーズ | タスク数 | 推定時間 |
|---------|---------|---------|
| Phase 9A | 3 | 2-3時間 |
| Phase 9B | 6 | 4-6時間 |
| Phase 9C | 5 | 3-4時間 |
| **合計** | **14** | **9-13時間** |

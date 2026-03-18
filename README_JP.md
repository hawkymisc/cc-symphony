# Symphony

> 📖 English version: [README.md](README.md)

Symphony はプロジェクトの作業を独立した自律的な実装実行に変換し、チームがコーディングエージェントを監視する代わりに作業を管理できるようにします。

[![Symphony デモ動画プレビュー](.github/media/symphony-demo-poster.jpg)](.github/media/symphony-demo.mp4)

_この[デモ動画](.github/media/symphony-demo.mp4)では、Symphony が Linear ボードの作業を監視し、タスクを処理するエージェントを起動します。エージェントはタスクを完了し、作業の証拠（CI ステータス、PR レビューフィードバック、複雑度分析、ウォークスルー動画）を提供します。承認後、エージェントは安全に PR をマージします。エンジニアは Codex を監視する必要がなく、より高いレベルで作業を管理できます。_

> [!WARNING]
> Symphony は信頼できる環境でのテスト用エンジニアリングプレビューです。

---

## 実装一覧

| 実装 | トラッカー | エージェント | ステータス |
|---|---|---|---|
| [Elixir](elixir/) (リファレンス) | Linear | Codex | アップストリームオリジナル |
| [Rust](rust/) | GitHub Issues | Claude Code CLI | ✅ 全フェーズ完了 |

---

## Rust 実装 (GitHub + Claude Code)

Rust 実装は **GitHub Issues** と **Claude Code CLI** を接続し、コーディングタスクを自動化します。

### 必要要件

- Rust 1.75+
- [`claude` CLI](https://claude.ai/code) がインストール・認証済みであること
- GitHub パーソナルアクセストークン（スコープについては[セキュリティ & トークン設定](#セキュリティ--トークン設定)を参照）

### クイックスタート

**1. ビルドとインストール**

```bash
cd rust
cargo install --path .
```

または、ビルド出力から直接実行：

```bash
cargo build --release
./target/release/symphony --help
```

**2. 認証情報の設定**

```bash
export GITHUB_TOKEN=ghp_xxxxxxxxxxxx
```

**3. `WORKFLOW.md` の作成**

```markdown
---
tracker:
  kind: github
  repo: "owner/your-repo"
  api_key: "$GITHUB_TOKEN"          # 起動時に環境変数から解決
  labels: ["symphony"]              # オプション: このラベルの Issue のみ取得
agent:
  max_concurrent_agents: 3
polling:
  interval_ms: 30000                # 30秒ごとにポーリング
---
You are a coding agent working on {{ issue.title }} (#{{ issue.identifier }}).

Repository: {{ repo }}

Issue description:
{{ issue.description }}

Please implement a solution, open a PR, and close the issue when done.
```

**4. 設定の検証（ドライラン）**

```bash
symphony ./WORKFLOW.md --dry-run
```

期待される出力：
```
Config validated successfully
  Tracker: github (owner/your-repo)
  Model: claude-sonnet-4-20250514
  Max concurrent agents: 3
  Poll interval: 30000ms
```

**5. 実行**

```bash
symphony ./WORKFLOW.md
```

**HTTP 可観測性ダッシュボード付き（オプション）：**

```bash
cargo build --release --features http-server
./target/release/symphony ./WORKFLOW.md --port 8080
# ブラウザで http://127.0.0.1:8080 を開く
```

### 終了コード

| コード | 意味 |
|--------|------|
| 0 | 正常シャットダウン（SIGTERM / SIGINT） |
| 1 | 設定 / 起動時バリデーションエラー |
| 2 | CLI 引数エラー |
| 3 | ワークフローファイルエラー（未発見 / 読取不可 / 無効な YAML） |

### WORKFLOW.md リファレンス

```yaml
---
tracker:
  kind: github               # 必須
  repo: "owner/repo"         # 必須（owner/repo 形式）
  api_key: "$GITHUB_TOKEN"   # 必須; $VAR は環境変数から解決
  endpoint: "..."            # オプション: GitHub GraphQL URL を上書き
  labels: ["symphony"]       # オプション: ラベルフィルター

agent:
  max_concurrent_agents: 10  # デフォルト: 10
  max_turns: 20              # デフォルト: 20; 予約済み — 未実装
  max_retry_backoff_ms: 300000  # デフォルト: 5分
  max_retry_queue_size: 1000    # デフォルト: 1000; 満杯時は最古のエントリをエビクション

polling:
  interval_ms: 30000         # デフォルト: 30秒

claude:
  command: "claude"          # デフォルト: claude
  model: "claude-sonnet-4-20250514"
  max_turns_per_invocation: 50  # デフォルト: 50
  skip_permissions: false    # 信頼できる環境でのみ true に設定
  allowed_tools:             # skip_permissions=false の場合に必須
    - "Bash"                 # 例示用 — ワークフローに合わせて調整
    - "Read"
    - "Write"

workspace:
  root: "~/symphony-workspaces"  # デフォルト: $TMPDIR/symphony_workspaces

hooks:
  after_create:  "./scripts/setup.sh"    # ワークスペース初回作成時に一度だけ実行
  before_run:    "./scripts/prepare.sh"  # 各エージェント呼び出し前に実行（失敗時は致命的）
  after_run:     "./scripts/cleanup.sh"  # 各エージェント呼び出し後に実行（非致命的）
  before_remove: "./scripts/teardown.sh" # Issue 放棄時のワークスペース削除前に実行（非致命的）
  timeout_ms: 60000                      # デフォルト: 60秒; 全フックに適用
---
プロンプトテンプレート。使用可能な変数:

{{ issue.title }}        — Issue タイトル
{{ issue.identifier }}   — Issue 番号（例: "42"）
{{ issue.description }}  — Issue 本文
{{ repo }}               — "owner/repo"
{{ attempt }}            — リトライ回数（1始まり; 初回実行時は未設定）
```

---

## ラベルライフサイクル

Symphony は2つの予約 GitHub ラベルを使い、Issue の進捗管理と無限再ディスパッチループの防止を行います。GitHub Issues は `open` / `closed` の2状態しかないため、ラベルがオーケストレータに作業完了を伝える軽量なシグナルとなります（Issue をクローズせずに済みます）。

| ラベル | 管理者 | 意味 |
|-------|--------|------|
| `symphony-doing` | **オーケストレータ**（自動） | 作業中 — 他インスタンスからの新規ディスパッチをブロック |
| `symphony-done` | **エージェント**（ワークフロー経由） | 作業完了 — 再ディスパッチループを停止 |

**フロー:**

```
Issue 作成（open、ラベルなし）
  │
  ▼
オーケストレータがディスパッチ ──→ `symphony-doing` を付与
  │
  ▼
エージェントがタスクを実行・完了
  │
  ▼
エージェントが `symphony-done` を付与 ──→ オーケストレータが `symphony-doing` を除去
  │
  ▼
Issue は `symphony-done` 付きで open のまま ──→ 再ディスパッチなし
  │
  ▼
人間がレビューして Issue をクローズ
```

エージェントにラベルを付与させるには、ワークフローテンプレートに完了プロトコルを記述します：

```markdown
## 完了プロトコル

作業が完了したら：
1. `symphony-done` ラベルを付与：
   `gh issue edit {{ issue.identifier }} --repo owner/repo --add-label symphony-done`
2. Issue はクローズしないでください — 人間がレビューしてクローズします。
```

> **注意**: Symphony を実行する前に、リポジトリに両方のラベルを作成してください。
>
> ```bash
> gh label create symphony-doing --description "Symphony: agent working" --color FBCA04
> gh label create symphony-done  --description "Symphony: agent completed" --color 0E8A16
> ```

---

## セキュリティ & トークン設定

### トークンアーキテクチャ

Symphony は単一の `GITHUB_TOKEN` を2つの目的で使用します：

| 使用者 | 操作 | トークンが必要な理由 |
|--------|------|---------------------|
| **Symphony**（メインプロセス） | GraphQL API による Issue ポーリング | 作業対象の Issue を読み取る |
| **Claude Code**（子プロセス） | `git push`, `gh pr create`, `gh issue comment` | 変更を実装し PR を作成する |

> **重要**: `GITHUB_TOKEN` 環境変数は Claude Code 子プロセスに継承されます。これは設計上意図的なものです — Claude Code がブランチの push や `gh` による PR 作成に必要とするためです。

### 推奨: Fine-grained パーソナルアクセストークン

対象リポジトリのみにスコープを限定した [Fine-grained PAT](https://github.com/settings/personal-access-tokens/new) を使用してください：

| スコープ | 権限 | 理由 |
|---------|------|------|
| **Contents** | Read and Write | Claude Code からの `git push` |
| **Issues** | Read and Write | ポーリング（Symphony）+ コメント（Claude Code） |
| **Pull Requests** | Read and Write | Claude Code からの `gh pr create` |
| **Metadata** | Read-only | 自動付与 |

```bash
# 専用の Fine-grained PAT（単一リポジトリ、最小スコープ）
export GITHUB_TOKEN=github_pat_xxxxxxxxxxxx
```

### 避けるべき: Classic PAT と `gh auth token`

| 方法 | リスク |
|------|--------|
| `repo` スコープの Classic PAT | **すべての**リポジトリへの書き込み権限を付与 |
| `gh auth token` | `gh` CLI の OAuth トークンを返す（多くの場合、広範な組織スコープを含む） |

どちらも開発・テストには使えますが、無人の本番運用では Fine-grained PAT を使用することで、トークンが漏洩したりエージェントに悪用された場合の影響範囲を限定できます。

### `skip_permissions` とエージェントのサンドボックス

```yaml
claude:
  skip_permissions: true   # ⚠️ Claude Code にフルシステムアクセスを付与
```

`skip_permissions: true` の場合、Claude Code は `--dangerously-skip-permissions` 付きで実行され、任意のシェルコマンドの実行、任意のファイルの読み書き、すべての環境変数（`GITHUB_TOKEN` を含む）へのアクセスが可能になります。

**緩和策**:
- Symphony を隔離された環境（コンテナ、VM、専用ユーザー）で実行する
- 単一リポジトリにスコープを限定した Fine-grained PAT を使用する
- 設定で `allowed_tools` を指定し、Claude Code のツールアクセスを制限する（`skip_permissions` の代替）
- HTTP ダッシュボードは `127.0.0.1` のみにバインド — ネットワークに公開しないこと

---

## 機能ステータス

### ✅ 実装済み

| 機能 | 備考 |
|------|------|
| GitHub Issues ポーリング | GraphQL v4、ページネーション、ラベルフィルタリング |
| Issue ディスパッチ | FIFO-by-created_at（GitHub Issues ではプライオリティフィールドは常に null）、並行数制限、クレーム重複排除 |
| Claude Code CLI 統合 | サブプロセス、ストリーミング JSON イベント、トークン追跡 |
| ワークスペース管理 | Issue ごとのディレクトリ、フックスクリプト（after_create / before_run / after_run） |
| 指数バックオフ付きリトライ | 設定可能な上限、連続失敗追跡 |
| グレースフルシャットダウン | SIGTERM / SIGINT → キャンセル安全な終了 |
| ドライランモード | `--dry-run` で設定を検証して終了 |
| 可観測性スナップショット | 内部メッセージチャネル経由の `RuntimeSnapshot` |
| HTTP ダッシュボード | Feature-gated（`--features http-server`）; `GET /`, `GET /api/status`, `POST /api/refresh` |
| 構造化ログ | `tracing`（デフォルトは人間可読形式）; 全 span に issue_id + identifier |
| トークン集約 | 全セッションにわたる入力 / 出力 / キャッシュ読取 / キャッシュ作成トークンを追跡 |
| 完了 Issue カウント | `OrchestratorState.completed_count`（u64、単調増加）; スナップショットとダッシュボードで公開 |
| ワークスペースクリーンアップ（`before_remove` フック） | リトライ中の Issue が terminal または未発見の場合に `cleanup_workspace` を呼び出し; ディレクトリ削除前に `before_remove` フックが起動 |
| トラッカー障害バックオフ | 連続するトラッカーポーリング障害で指数バックオフ（最大5分）; `skip_ticks_until` による非ブロッキング |
| API キーマスキング | `TrackerConfig` と `GitHubConfig` のカスタム `Debug` 実装で `api_key` を `[REDACTED]` に置換 |
| リトライキューエビクション | `max_retry_queue_size`（デフォルト 1000）; 満杯時に最古のエントリをエビクション、ワークスペースは非同期でクリーンアップ |
| ラベルベースのディスパッチ制御 | `symphony-doing`（オーケストレータが自動管理）と `symphony-done`（エージェントが設定）ラベルで無限再ディスパッチループを防止 |

### 🔲 未実装

| 機能 | 備考 |
|------|------|
| GitHub Projects v2 | GitHub Issues のみサポート; Projects v2 カスタムフィールドは未マッピング |
| HTTP ダッシュボード認証 | ダッシュボードはループバックのみにバインド; ベアラートークン / UNIX ソケットオプションなし |
| Windows グレースフルシャットダウン | SIGTERM テストは `#[cfg(unix)]`; Windows `Ctrl+C` パスは未テスト |
| 実 GitHub CI ゲート | 統合テストは `MemoryTracker` を使用; ステージング用スモークテストなし |
| 設定ホットリロード | `ConfigReloaded` メッセージは存在するが WORKFLOW.md の再解析は行わない |
| ステートごとの並行数制限 | グローバル制限のみ; ラベル / プロジェクトごとのスロット制御なし |
| プライオリティベースのディスパッチ | GitHub Issues にはネイティブのプライオリティフィールドがない; ディスパッチは常に最古優先（created_at）にフォールバック |

---

## オリジナル Elixir 実装の実行

アップストリームの Linear + Codex リファレンス実装については [elixir/README.md](elixir/README.md) を参照してください。

---

## ライセンス

このプロジェクトは [Apache License 2.0](LICENSE) の下でライセンスされています。

# Symphony

[English](../../README.md) | [日本語](../ja/README.md) | [中文](../zh/README.md) | **[한국어](README.md)**

Symphony는 프로젝트 작업을 독립적이고 자율적인 구현 실행으로 전환하여, 팀이 코딩 에이전트를 감독하는 대신 작업을 관리할 수 있게 합니다.

[![Symphony 데모 영상 미리보기](../../.github/media/symphony-demo-poster.jpg)](../../.github/media/symphony-demo.mp4)

_이 [데모 영상](../../.github/media/symphony-demo.mp4)에서 Symphony는 Linear 보드의 작업을 모니터링하고 태스크를 처리할 에이전트를 생성합니다. 에이전트는 태스크를 완료하고 작업 증거(CI 상태, PR 리뷰 피드백, 복잡도 분석, 워크스루 영상)를 제공합니다. 승인되면 에이전트가 안전하게 PR을 머지합니다. 엔지니어는 Codex를 감독할 필요 없이 더 높은 수준에서 작업을 관리할 수 있습니다._

> [!WARNING]
> Symphony는 신뢰할 수 있는 환경에서의 테스트용 엔지니어링 프리뷰입니다.

---

## 구현 목록

| 구현 | 트래커 | 에이전트 | 상태 |
|------|--------|---------|------|
| [Elixir](../../elixir/) (레퍼런스) | Linear | Codex | 업스트림 원본 |
| [Rust](../../rust/) | GitHub Issues | Claude Code CLI | ✅ 모든 단계 완료 |

---

## Rust 구현 (GitHub + Claude Code)

Rust 구현은 **GitHub Issues**와 **Claude Code CLI**를 연결하여 코딩 태스크를 자동화합니다.

### 요구사항

- Rust 1.75+
- [`claude` CLI](https://claude.ai/code) 설치 및 인증 완료
- GitHub 개인 액세스 토큰 (범위는 [보안 및 토큰 설정](#보안-및-토큰-설정) 참조)

### 빠른 시작

**1. 빌드 및 설치**

```bash
cd rust
cargo install --path .
```

또는 빌드 출력에서 직접 실행:

```bash
cargo build --release
./target/release/symphony --help
```

**2. 자격 증명 설정**

```bash
export GITHUB_TOKEN=ghp_xxxxxxxxxxxx
```

**3. `WORKFLOW.md` 생성**

```markdown
---
tracker:
  kind: github
  repo: "owner/your-repo"
  api_key: "$GITHUB_TOKEN"          # 시작 시 환경 변수에서 해석
  labels: ["symphony"]              # 선택: 이 라벨의 Issue만 가져옴
agent:
  max_concurrent_agents: 3
polling:
  interval_ms: 30000                # 30초마다 폴링
---
You are a coding agent working on {{ issue.title }} (#{{ issue.identifier }}).

Repository: {{ repo }}

Issue description:
{{ issue.description }}

Please implement a solution, open a PR, and close the issue when done.
```

**4. 설정 검증 (드라이 런)**

```bash
symphony ./WORKFLOW.md --dry-run
```

예상 출력:
```
Config validated successfully
  Tracker: github (owner/your-repo)
  Model: claude-sonnet-4-20250514
  Max concurrent agents: 3
  Poll interval: 30000ms
```

**5. 실행**

```bash
symphony ./WORKFLOW.md
```

**HTTP 관측성 대시보드 포함 (선택):**

```bash
cargo build --release --features http-server
./target/release/symphony ./WORKFLOW.md --port 8080
# 브라우저에서 http://127.0.0.1:8080 열기
```

### 종료 코드

| 코드 | 의미 |
|------|------|
| 0 | 정상 종료 (SIGTERM / SIGINT) |
| 1 | 설정 / 시작 검증 실패 |
| 2 | CLI 인수 오류 |
| 3 | 워크플로 파일 오류 (미발견 / 읽기 불가 / 유효하지 않은 YAML) |

### WORKFLOW.md 레퍼런스

```yaml
---
tracker:
  kind: github               # 필수
  repo: "owner/repo"         # 필수 (owner/repo 형식)
  api_key: "$GITHUB_TOKEN"   # 필수; $VAR은 환경 변수에서 해석
  endpoint: "..."            # 선택: GitHub GraphQL URL 재정의
  labels: ["symphony"]       # 선택: 라벨 필터

agent:
  max_concurrent_agents: 10  # 기본값: 10
  max_turns: 20              # 기본값: 20; 예약됨 — 미구현
  max_retry_backoff_ms: 300000  # 기본값: 5분
  max_retry_queue_size: 1000    # 기본값: 1000; 가득 차면 가장 오래된 항목 퇴거

polling:
  interval_ms: 30000         # 기본값: 30초

claude:
  command: "claude"          # 기본값: claude
  model: "claude-sonnet-4-20250514"
  max_turns_per_invocation: 50  # 기본값: 50
  skip_permissions: false    # 신뢰할 수 있는 환경에서만 true로 설정
  allowed_tools:             # skip_permissions=false인 경우 필수 (이 목록 또는 skip_permissions: true 중 하나)
    - "Bash"                 # 예시용 — 워크플로에 맞게 조정; Bash는 전체 셸 접근 권한 부여
    - "Read"
    - "Write"

workspace:
  root: "~/symphony-workspaces"  # 기본값: $TMPDIR/symphony_workspaces

hooks:
  after_create:  "./scripts/setup.sh"    # 워크스페이스 첫 생성 시 한 번 실행
  before_run:    "./scripts/prepare.sh"  # 각 에이전트 호출 전 실행 (실패 시 치명적)
  after_run:     "./scripts/cleanup.sh"  # 각 에이전트 호출 후 실행 (비치명적)
  before_remove: "./scripts/teardown.sh" # Issue 포기 시 워크스페이스 삭제 전 실행 (비치명적)
  timeout_ms: 60000                      # 기본값: 60초; 모든 훅에 적용
---
프롬프트 템플릿. 사용 가능한 변수:

{{ issue.title }}        — Issue 제목
{{ issue.identifier }}   — Issue 번호 (예: "42")
{{ issue.description }}  — Issue 본문
{{ repo }}               — "owner/repo"
{{ attempt }}            — 재시도 횟수 (1부터 시작; 첫 실행 시 없음)
```

---

## 보안 및 토큰 설정

### 토큰 아키텍처

Symphony는 단일 `GITHUB_TOKEN`을 두 가지 목적으로 사용합니다:

| 사용자 | 작업 | 토큰이 필요한 이유 |
|--------|------|-------------------|
| **Symphony** (메인 프로세스) | GraphQL API를 통한 Issue 폴링 | 작업할 Issue를 읽기 위해 |
| **Claude Code** (자식 프로세스) | `git push`, `gh pr create`, `gh issue comment` | 변경사항을 구현하고 PR을 생성하기 위해 |

> **중요**: `GITHUB_TOKEN` 환경 변수는 Claude Code 자식 프로세스에 상속됩니다. 이는 설계 의도입니다 — Claude Code가 브랜치를 push하고 `gh`를 통해 PR을 생성하는 데 필요합니다.

### 권장: Fine-grained 개인 액세스 토큰

대상 리포지토리만으로 범위를 제한한 [Fine-grained PAT](https://github.com/settings/personal-access-tokens/new)를 사용하세요:

| 범위 | 권한 | 이유 |
|------|------|------|
| **Contents** | Read and Write | Claude Code에서의 `git push` |
| **Issues** | Read and Write | 폴링 (Symphony) + 댓글 (Claude Code) |
| **Pull Requests** | Read and Write | Claude Code에서의 `gh pr create` |
| **Metadata** | Read-only | 자동 부여 |

```bash
# 전용 Fine-grained PAT (단일 리포지토리, 최소 범위)
export GITHUB_TOKEN=github_pat_xxxxxxxxxxxx
```

### 피해야 할 것: Classic PAT과 `gh auth token`

| 방법 | 위험 |
|------|------|
| `repo` 범위의 Classic PAT | **모든** 리포지토리에 대한 쓰기 권한 부여 |
| `gh auth token` | `gh` CLI의 OAuth 토큰을 반환, 종종 광범위한 조직 범위 포함 |

둘 다 개발/테스트에는 사용할 수 있지만, 무인 프로덕션 사용에는 Fine-grained PAT를 사용하여 토큰이 유출되거나 에이전트에 의해 오용될 경우 영향 범위를 제한해야 합니다.

### `skip_permissions`와 에이전트 샌드박싱

```yaml
claude:
  skip_permissions: true   # ⚠️ Claude Code에 전체 시스템 액세스 권한 부여
```

`skip_permissions: true`일 때, Claude Code는 `--dangerously-skip-permissions`로 실행되며, 임의의 셸 명령 실행, 모든 파일 읽기/쓰기, 모든 환경 변수(`GITHUB_TOKEN` 포함) 접근이 가능합니다.

**완화 조치**:
- Symphony를 격리된 환경(컨테이너, VM 또는 전용 사용자)에서 실행
- 단일 리포지토리로 범위를 제한한 Fine-grained PAT 사용
- 설정에서 `allowed_tools`를 지정하여 Claude Code의 도구 접근 제한 (`skip_permissions`의 대안)
- HTTP 대시보드는 `127.0.0.1`에만 바인드 — 네트워크에 노출하지 않을 것

---

## 기능 상태

### ✅ 구현 완료

| 기능 | 비고 |
|------|------|
| GitHub Issues 폴링 | GraphQL v4, 페이지네이션, 라벨 필터링 |
| Issue 디스패치 | FIFO-by-created_at (GitHub Issues에서 우선순위 필드는 항상 null), 동시성 제한, 클레임 중복 제거 |
| Claude Code CLI 통합 | 서브프로세스, 스트리밍 JSON 이벤트, 토큰 추적 |
| 워크스페이스 관리 | Issue별 디렉토리, 훅 스크립트 (after_create / before_run / after_run) |
| 지수 백오프 재시도 | 설정 가능한 상한, 연속 실패 추적 |
| 그레이스풀 셧다운 | SIGTERM / SIGINT → 취소 안전 종료 |
| 드라이 런 모드 | `--dry-run`으로 설정 검증 후 종료 |
| 관측성 스냅샷 | 내부 메시지 채널을 통한 `RuntimeSnapshot` |
| HTTP 대시보드 | Feature-gated (`--features http-server`); `GET /`, `GET /api/status`, `POST /api/refresh` |
| 구조화 로깅 | `tracing` (기본값은 사람이 읽을 수 있는 형식); 모든 span에 issue_id + identifier |
| 토큰 집계 | 모든 세션에 걸쳐 입력 / 출력 / 캐시 읽기 / 캐시 생성 토큰 추적 |
| 완료 Issue 카운트 | `OrchestratorState.completed_count` (u64, 단조 증가); 스냅샷과 대시보드에서 공개 |
| 워크스페이스 정리 (`before_remove` 훅) | 재시도 중인 Issue가 terminal이거나 미발견 시 `cleanup_workspace` 호출; 디렉토리 삭제 전 `before_remove` 훅 실행 |
| 트래커 장애 백오프 | 연속 트래커 폴링 장애 시 지수 백오프 (최대 5분); `skip_ticks_until`로 비차단 |
| API 키 마스킹 | `TrackerConfig`과 `GitHubConfig`의 커스텀 `Debug` 구현에서 `api_key`를 `[REDACTED]`로 대체 |
| 재시도 큐 퇴거 | `max_retry_queue_size` (기본값 1000); 가득 차면 가장 오래된 항목 퇴거, 워크스페이스 비동기 정리 |

### 🔲 미구현

| 기능 | 비고 |
|------|------|
| GitHub Projects v2 | GitHub Issues만 지원; Projects v2 커스텀 필드 미매핑 |
| HTTP 대시보드 인증 | 대시보드는 루프백에만 바인드; Bearer 토큰 / UNIX 소켓 옵션 없음 |
| Windows 그레이스풀 셧다운 | SIGTERM 테스트는 `#[cfg(unix)]`; Windows `Ctrl+C` 경로 미테스트 |
| 실제 GitHub CI 게이트 | 통합 테스트는 `MemoryTracker` 사용; 스테이징 스모크 테스트 없음 |
| 설정 핫 리로드 | `ConfigReloaded` 메시지는 존재하지만 WORKFLOW.md를 재파싱하지 않음 |
| 상태별 동시성 제한 | 전역 제한만; 라벨 / 프로젝트별 슬롯 제어 없음 |
| 우선순위 기반 디스패치 | GitHub Issues에는 네이티브 우선순위 필드 없음; 디스패치는 항상 가장 오래된 것 우선 (created_at)으로 대체 |

---

## 원본 Elixir 구현 실행

업스트림 Linear + Codex 레퍼런스 구현은 [elixir/README.md](../../elixir/README.md)를 참조하세요.

---

## 라이선스

이 프로젝트는 [Apache License 2.0](../../LICENSE)에 따라 라이선스가 부여됩니다.

# herdr-jira-worktree

[English](README.md) | **한국어**

Vitalii Rudnykh의 [a2u/herdr-jira](https://github.com/a2u/herdr-jira)를 포크한
플러그인입니다. Jira 이슈를 herdr 프로젝트별 새 git worktree 또는 기존
worktree로 여는 `w` 키와, 이슈 상세 화면의 댓글 보기가 추가되어 있습니다.
버전은 업스트림과 별개로 관리합니다(이 포크는 0.1.0부터 시작).

[herdr](https://herdr.dev) 패널 안에서 동작하는 Jira TUI입니다. 설정한 JQL
필터로 이슈를 둘러보고, 검색하고, 상태를 바꾸고, 키 하나로 이슈를 herdr의 AI
에이전트에게 넘길 수 있습니다. 실행 중인 에이전트를 고르거나, 원하는
디렉터리에서 새 에이전트를 시작할 수 있습니다. 에이전트는 설정 가능한
템플릿(이슈 키, 요약, 설명, 링크 등)으로 만든 프롬프트를 받습니다.

```
╭ Jira — My open issues (23) ─────────────────────────────────────────╮
│ KEY         STATUS        ASSIGNEE          UPDATED          SUMMARY│
│ PROJ-142    In Progress   Vitalii R.        2026-07-14 10:02 Fix …  │
│ PROJ-137    To Do         Vitalii R.        2026-07-13 18:40 Add …  │
╰─────────────────────────────────────────────────────────────────────╯
 Enter open · w worktree · d delegate · s status · f filters · / search · ? help
```

## 기능

- **필터** — 설정 파일에 이름을 붙여 둔 JQL 필터(`f` 또는 `1`–`9`): 내 이슈,
  특정 프로젝트 등 JQL로 표현할 수 있는 무엇이든.
- **검색** — `/`는 `text ~ "…"` 검색(템플릿 변경 가능)을 실행하고, `J`는 직접
  입력한 JQL을 실행합니다. 현재 쿼리가 미리 채워져 있어 조금씩 고치기 쉽습니다.
- **에픽** — 에픽은 자홍색 타입 배지와 `▸` 표시로 구분됩니다. `→`로 펼치면
  하위 이슈가 목록 안에 표시되고(`parent = …`, Server/DC는 `"Epic Link"`로
  대체), `←`로 접습니다.
- **이슈 상세** — `Enter`로 설명을 스크롤해 볼 수 있는 화면을 엽니다(Cloud의
  ADF 문서는 일반 텍스트로 변환).
- **댓글** — 이슈 상세 화면(`Enter`)에서 설명 아래 패널에 댓글이 최신순으로
  표시됩니다. `Tab`으로 설명과 댓글 중 스크롤할 쪽을 바꿉니다. 댓글은 이슈별로
  캐시되며, 다시 불러오려면 이슈 목록에서 `r`을 누른 뒤 이슈를 다시 여세요.
- **상태 전환** — `s`는 이슈에 가능한 전환 목록을 보여 주고 고른 것을 적용합니다.
- **에이전트에게 위임** — `d`는 herdr에서 실행 중인 에이전트(claude, codex,
  grok 등)를 상태와 cwd와 함께 보여 줍니다. 하나를 고르면 `[delegate].prompt`
  템플릿으로 만든 프롬프트가 전송되고 서버 쪽에서 제출됩니다(설정 가능). 또는
  **+ start new agent…**(`n`)로 에이전트 종류와 작업 디렉터리를 고르면 herdr가
  `pane run`으로 실행하고, 에이전트가 준비되는 대로 같은 Jira 프롬프트를
  보냅니다.
- **Worktree** — `w`는 선택한 이슈의 git worktree를 만들거나 다시 엽니다.
  herdr 자체의 worktree 생성처럼 프로젝트 단위로 동작합니다: herdr 프로젝트
  하나를 고르고(현재 프로젝트가 맨 위), **+ new worktree**(이름은 이슈 키로 미리
  채워짐)나 기존 worktree 중 하나를 고른 다음, 원하면 그 안에서 Jira
  프롬프트를 받을 에이전트를 시작합니다.

Jira Cloud(이메일 + API 토큰)와 Jira Server / Data Center(개인 액세스 토큰)를
지원합니다. Cloud에서는 가능하면 새 `/rest/api/2/search/jql` 엔드포인트를 쓰고,
안 되면 기존 `/rest/api/2/search`로 자동 전환합니다.

## 설치

herdr **0.8.0 이상**과, 설치 시 Rust 툴체인(https://rustup.rs)이 필요합니다.

이 포크(플러그인 id `han.jira-worktree`, `w` worktree 키 포함)는 다음처럼
설치합니다:

```sh
herdr plugin install hanbong5938/herdr-jira-worktree
```

로컬 개발용:

```sh
git clone git@github.com:hanbong5938/herdr-jira-worktree.git
herdr plugin link ./herdr-jira-worktree
```

업스트림 플러그인(`a2u/herdr-jira`, id `herdr-jira`)은 별개의 플러그인이라 함께
설치할 수 있습니다.

## 설정

```sh
mkdir -p "$(herdr plugin config-dir han.jira-worktree)"
cp config.example.toml "$(herdr plugin config-dir han.jira-worktree)/config.toml"
```

`config.toml`을 편집합니다:

```toml
[jira]
base_url = "https://yourcompany.atlassian.net"
auth = "basic"                      # Server/DC PAT는 "bearer"
email = "you@company.com"
api_token_cmd = "security find-generic-password -s jira-api-token -w"
default_project = "PROJ"

[[filters]]
name = "My open issues"
jql = "assignee = currentUser() AND resolution = Unresolved ORDER BY updated DESC"

[[filters]]
name = "Project board"
jql = "project = {project} AND statusCategory != Done ORDER BY updated DESC"

[delegate]
prompt = """
You are asked to work on Jira issue {key}: {summary}
Link: {url}

Description:
{description}
"""
submit = true          # 서버 쪽에서 제출; false면 텍스트만 붙여 넣음

# default_cwd = "~/Work"
placement = "tab"      # "tab" | "right" | "down"
focus_new = false
startup_delay_ms = 1500
wait_ready_ms = 30000

# delegate 선택창에서 시작할 수 있는 에이전트("+ start new agent…")
[[delegate.agents]]
name = "claude"
command = ["claude"]

[[delegate.agents]]
name = "codex"
command = ["codex"]

[[delegate.agents]]
name = "grok"
command = ["grok"]
```

Jira Cloud라면
<https://id.atlassian.com/manage-profile/security/api-tokens>에서 API 토큰을
만들고, 설정 파일에 들어가지 않도록 macOS 키체인에 저장하세요:

```sh
security add-generic-password -s jira-api-token -a "$USER" -w '<TOKEN>'
```

실행 중인 패널에서 `R`을 누르면 설정을 다시 읽습니다.

## 패널 열기

herdr 액션 팔레트에서 **Jira: open (split)** 또는 **Jira: open (tab)** —
또는 `~/.config/herdr/config.toml`에 키를 지정합니다:

```toml
[[keys.command]]              # 현재 작업 옆 분할 창으로 열기
key = "prefix+j"
type = "plugin_action"
command = "han.jira-worktree.open-jira"

[[keys.command]]              # …또는 별도 탭으로 열기
key = "prefix+shift+j"
type = "plugin_action"
command = "han.jira-worktree.open-jira-tab"
```

(그다음 `herdr server reload-config`)

## 키

| 키 | 동작 |
| --- | --- |
| `j`/`k`, `↑`/`↓` | 이동 / 스크롤 |
| `PgUp`/`PgDn` | 15칸씩 이동 / 스크롤 |
| `g`/`G`, `Home`/`End` | 맨 위 / 맨 아래 (상세 화면: `g`는 맨 위로 스크롤) |
| `Enter` | 이슈 상세 열기 |
| `Tab` | 이슈 상세: 설명과 댓글 중 스크롤 대상 전환 |
| `→`/`l`, `←`/`h` | 에픽 펼치기 / 접기 (하위 이슈를 목록 안에 표시) |
| `f`, `1`–`9` | 필터 전환 |
| `/` | 검색 |
| `J` | 직접 JQL 실행 (현재 쿼리가 미리 채워짐) |
| `s` | 이슈 상태 변경 |
| `d` | 실행 중인 에이전트에게 이슈 위임, 또는 새 에이전트 시작 |
| `w` | 이슈용 git worktree (프로젝트 → 새/기존 worktree → 에이전트 선택) |
| `n` | delegate 선택창에서: 새 에이전트 시작 |
| `1`–`9` | 팝업(에이전트, 전환, 필터)에서 바로 선택 |
| `o` | 브라우저에서 이슈 열기 |
| `z` | Jira 패널 확대 (전체 화면 토글) |
| `r` | 현재 필터 새로고침 |
| `R` | 설정 다시 읽기 |
| `?` | 도움말 |
| `Esc` | 뒤로 / 취소 |
| `q` | 종료 |

## Delegate 프롬프트 placeholder

`{key}` `{summary}` `{description}` `{url}` `{status}` `{assignee}`
`{reporter}` `{priority}` `{type}` `{labels}`

`submit = true`이면 `herdr agent prompt <pane-id> <text>`가 서버 쪽에서
프롬프트를 전달하고 제출하므로, 붙여 넣은 텍스트와 Enter 사이의 경쟁 상태가
생기지 않습니다. `submit = false`이면 `herdr pane send-text <pane-id> <text>`로
텍스트만 붙여 넣습니다. 예전 `submit_delay_ms` 설정은 받아들이지만 더 이상 쓰지
않습니다. `w`로 시작한 에이전트에는 `[worktree].submit`이 우선합니다(아래 참고).
전달 오류는 raw 입력으로 재시도하지 않고 그대로 표시합니다. 프롬프트가
중복되거나 시작 대화상자에 입력되는 것을 막기 위해서입니다.

### 새 에이전트 시작

delegate 선택창에서 **+ start new agent…**(또는 `n`)를 누르면 짧은 마법사가
열립니다:

1. **에이전트 종류** — `[[delegate.agents]]`에서 선택(name + `command` argv).
2. **Space(workspace)** — 에이전트를 둘 herdr space 선택(현재 space가 미리
   선택됨).
3. **작업 디렉터리** — 실행 중인 에이전트들의 cwd(중복 제거), 선택 사항인
   `default_cwd`, 자주 쓰는 경로; 또는 **type path…**로 경로 직접 입력
   (`~`와 `$HOME/` 확장).

`placement = "tab"`(기본값)이면 이슈 키로 이름 붙은 새 탭을 만들고, 에이전트를
**그 탭의 단일 루트 패널**에서 실행합니다(셸 + 에이전트 분할이 아니라 터미널
하나):

```sh
herdr tab create --workspace <space> --cwd <dir> --label <ISSUE-KEY> --no-focus
herdr pane run <root-pane> '<command...>'
herdr agent rename <root-pane> <issue-agent-id>
```

`placement = "right"` 또는 `"down"`이면 `herdr pane split <parent-pane>
--direction right|down --cwd <dir> --no-focus`를 실행한 뒤 반환된 패널에서
`pane run`을 합니다. 부모 패널은 Jira 패널이 선택한 workspace에 있으면 Jira
패널이고, 아니면 그 workspace의 포커스된 패널(또는 첫 번째 패널)입니다.
`focus_new = true`면 두 방식 모두 `--focus`를 씁니다.

그다음 `startup_delay_ms`만큼 기다리고, 감지될 때까지 `agent get`을 폴링하고,
`agent wait --until idle`을 거친 뒤 렌더링된 Jira 프롬프트를 보냅니다. 감지와
idle 대기는 하나의 `wait_ready_ms` 예산을 공유합니다. 준비에 실패하면
프롬프트는 **전송되지 않고** 오류가 표시됩니다. `wait_ready_ms = 0`은 이 확인을
명시적으로 끄는 설정으로, 에이전트를 새로 띄울 때는 권장하지 않습니다.

## Worktree (`w`)

`w`(이슈 목록 또는 이슈 상세에서)는 이슈용 git worktree를 만들거나 다시
엽니다. herdr 자체의 worktree 생성처럼 프로젝트 단위로 동작하며, `Esc`는 한
단계 뒤로 갑니다:

1. **프로젝트** — 프로젝트 키별 `[worktree.repos]`(`PROJ-1666`이면 `PROJ`),
   없으면 `[worktree].repo`, 그것도 없으면 herdr에 열려 있는 git
   프로젝트(`herdr workspace list`) 중에서 고릅니다: 소스 저장소마다 한 줄이며,
   현재 workspace의 프로젝트가 맨 위(★, 이 패널이 그 프로젝트의 linked
   worktree에서 실행 중일 때도 해당)이고 나머지는 workspace 순서입니다. 각 줄에
   소스 checkout 경로와 열린 worktree workspace 수가 표시됩니다. 마지막 줄
   **type path…**(`/`)로 아무 저장소 디렉터리나 입력할 수 있습니다.
2. **Worktree** — 프로젝트의 checkout 목록(`herdr worktree list`)이며, herdr
   workspace에 이미 열려 있으면 `[open]`이 붙습니다. **+ new worktree**는
   이름을 묻는데, `branch` 템플릿(기본 `{key}`)으로 미리 채워지고 자유롭게
   고칠 수 있습니다(`Ctrl-U`로 지우기). `Enter`를 누르면 유효한 git 브랜치
   이름으로 정리됩니다. 기존 worktree를 고르면 그것을 다시 엽니다. 이슈의
   worktree가 이미 있으면 미리 선택됩니다.
3. **에이전트** — **no agent**는 worktree workspace만 엽니다. 또는
   `[[delegate.agents]]`에서 에이전트를 골라 그 안에서 시작하고 Jira
   프롬프트를 보냅니다. `d`와 똑같이 동작합니다(같은 `[delegate]`
   prompt/준비 대기 설정; `submit`은 `[worktree].submit`을 지정하지 않으면
   `[delegate].submit`을 따름). `[worktree] submit = false`면 프롬프트를
   에이전트 입력창에 붙여 넣기만 하고(토스트: "… pasted into … — review and
   press Enter"), 직접 고친 뒤 Enter를 누르면 됩니다. `d`는 그대로 제출합니다.

`config.toml` 끝(`[[delegate.agents]]` 뒤)에 `[worktree]` 테이블을 추가합니다:

```toml
[worktree]
repo = "~/workspace/platform"          # 기본 저장소; 비우거나 생략하면 herdr 프로젝트 선택
branch = "{key}"                       # 이름 미리 채우기, 예: "{type}/{key}-{slug}"
base = "origin/main"                   # 새 브랜치의 기준 ref (기본: HEAD)
focus = true                           # worktree workspace로 포커스 이동
# path = "~/worktrees/{branch}"        # checkout 경로 (기본: herdr의 worktrees 디렉터리)
# label = "{key}"                      # workspace 라벨
# trust_repository = false             # --trust-repository 전달; 검증한 저장소에만
# submit = false                       # 생략 = [delegate].submit; false = 붙여 넣기만, Enter는 직접

[worktree.repos]                       # Jira 프로젝트 키별
PROJ = "~/workspace/project"
```

### Worktree checkout

저장소 디렉터리는 git 저장소 안에 있어야 합니다. 플러그인은 먼저 해당 브랜치의
기존 worktree를 열어 보고, herdr가 `worktree_not_found`를 보고할 때만 새로
만듭니다:

```sh
herdr worktree open --cwd <repo> --branch <branch> [--label <label>] --focus|--no-focus
herdr worktree create --cwd <repo> --branch <branch> [--base <ref>] [--path <path>] [--label <label>] --focus|--no-focus
```

herdr는 checkout을 저장소 workspace와 묶인 새 workspace로 열고, 에이전트는 그
루트 패널에서 실행됩니다. 그 worktree의 workspace가 이미 열려 있으면, 에이전트는
거기에 이슈 키로 이름 붙은 새 탭에서 실행됩니다. 플러그인은 이미 에이전트가
돌고 있을 수 있는 패널에 절대 입력하지 않습니다. `focus`는
`--focus`/`--no-focus`를 정합니다. `trust_repository = true`면 두 명령 모두에
`--trust-repository`를 붙입니다. 직접 검증한 저장소에만 켜세요.

worktree와 그 workspace를 정리하려면:

```sh
herdr worktree remove --workspace <id>   # 커밋하지 않은 변경이 있으면 --force 추가
```

### Worktree placeholder

`branch`: `{key}` `{slug}` `{type}`. `path`와 `label`은 추가로 `{branch}`도
받습니다. 이름은 유효한 git ref로 정리됩니다. 요약에 ASCII 문자나 숫자가
없으면 `{slug}`가 비므로, `{key}-{slug}`는 키만 남습니다.

## 라이선스

MIT — [LICENSE](LICENSE) 참고. 원저작물 © Vitalii Rudnykh.

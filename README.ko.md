# herdr-jira-worktree

[English](README.md) | **한국어**

Vitalii Rudnykh의 [a2u/herdr-jira](https://github.com/a2u/herdr-jira)를 포크한
플러그인입니다. Jira 이슈를 Git worktree 또는 네이티브 jj workspace로 여는 `w` 키와, 이슈 상세 화면의 댓글 보기가 추가되어 있습니다.
버전은 업스트림과 별개로 관리합니다(이 포크는 0.1.0부터 시작).

[herdr](https://herdr.dev) 패널에서 Jira 이슈를 찾아 Git worktree 또는 jj workspace로 이어 주는
도구입니다. Jira 전체를 대체하지 않습니다. JQL 필터와 검색으로 이슈를 찾고
상세/댓글을 읽은 뒤, 선택적으로 worktree를 열거나 에이전트를 시작하세요.
상태 변경과 에이전트 위임은 명시적으로 선택해야 합니다.

```
╭ Jira — My open issues (23) ─────────────────────────────────────────╮
│ KEY         STATUS        ASSIGNEE          UPDATED          SUMMARY│
│ PROJ-142    In Progress   Vitalii R.        2026-07-14 10:02 Fix …  │
│ PROJ-137    To Do         Vitalii R.        2026-07-13 18:40 Add …  │
╰─────────────────────────────────────────────────────────────────────╯
 Enter open · w worktree · d delegate · s status · f filters · / search · ? help
```

## 이 포크에서 추가된 기능

둘 다 Jira 연결 설정만 있으면 추가 설정 없이 바로 쓸 수 있습니다.

### 이슈 댓글

이슈에서 `Enter`를 누르면 상세 화면이 열리고, 위에는 설명이, 아래에는 이슈
댓글(최신순)이 표시됩니다. `Tab`이나 마우스 클릭으로 `j`/`k`(`PgUp`/`PgDn`,
`g`/`G`)가 스크롤할 쪽을 바꾸고, 마우스 휠은 커서 아래 패널을 스크롤합니다. 내용이
넘치면 스크롤바가 표시되고, 댓글 패널 아래 테두리의 ` … `는 아래에 댓글이 더
있다는 뜻입니다. `Esc`로 목록으로 돌아갑니다.

```
╭ issue ───────────────────────────────────────────────────────────────────╮
│ PROJ-142  Fix login crash on iOS 17                                      │
│                                                                          │
│ Status    In Progress                                                    │
│ Assignee  Vitalii R.                                                     │
│ Link      https://yourcompany.atlassian.net/browse/PROJ-142              │
│                                                                          │
│ Description                                                              │
│ The app crashes right after tapping "Sign in" on iOS 17.                 │
╰──────────────────────────────────────────────────────────────────────────╯
╭ comments · PROJ-142 (2) ─────────────────────────────────────────────────╮
│ Jane Doe  2026-07-14 09:12                                               │
│ Reproduced on 17.5, crash log attached.                                  │
│                                                                          │
│ Vitalii R.  2026-07-13 18:55                                             │
│ Probably the keychain migration.                                         │
╰──────────────────────────────────────────────────────────────────────────╯
j/k scroll  ·  Tab comments  ·  w worktree  ·  d delegate  ·  Esc back
```

### 이슈 → Git worktree 또는 jj workspace (`w`)

이슈에서(목록이든 상세 화면이든) `w`를 누르면 그 이슈 전용 Git worktree 또는 jj workspace를
herdr UI workspace로 열어 작업할 수 있습니다. 세 번만 고르면 되고, `Esc`는 한
단계 뒤로, `1`–`9`는 해당 줄을 바로 선택합니다.

**1. 프로젝트** — herdr에 열려 있는 Git/jj 프로젝트 목록이며, 현재 프로젝트가 맨
위(★)입니다. `type path…`로 다른 저장소 경로를 직접 입력할 수 있습니다.
무관한 jj 후보가 고장 나거나 jj 실행 파일이 없어도 정상 Git 프로젝트는 목록에
남습니다. 실패 후보의 herdr workspace·경로·오류는 프로젝트 선택창에서 스크롤해
확인할 수 있습니다. 직접 지정한 잘못된 저장소는 오류로 처리합니다.

```
╭ worktree for PROJ-142 — project ─────────────────────────────────────╮
│herdr projects (source checkouts) · ★ current workspace               │
│▸ 1. ★ mobile-app  /Users/you/workspace/mobile-app  (1 open worktree) │
│  2.   backend     /Users/you/workspace/backend                       │
│  3. type path…  (/)                                                  │
╰──────────────────────────────────────────────────────────────────────╯
```

**2. Checkout** — `+ new`는 이슈 키 이름의 Git 브랜치/worktree 또는 jj
workspace를 만들고(`Enter` 전에 이름 수정 가능), 기존 checkout은 다시 엽니다.

```
╭ worktree for PROJ-142 — mobile-app ──────────────────────────────────╮
│create a worktree or reopen one of /Users/you/workspace/mobile-app    │
│▸ 1. + new worktree (PROJ-142)                                        │
│  2. main      /Users/you/workspace/mobile-app  (source)  [open]      │
│  3. PROJ-120  /Users/you/.herdr/worktrees/mobile-app/PROJ-120  [open]│
╰──────────────────────────────────────────────────────────────────────╯
```

**3. 에이전트** — 스냅샷을 읽지 않고 worktree만 열거나,
`[[delegate.agents]]`에서 에이전트를 고릅니다. 완전한 스냅샷은 submit
설정에 따라 전송되고, 경고가 있을 때만 제한된 Jira 문맥을 확인한 뒤
진행합니다. 마지막 선택창에는 저장소, checkout 이름, 기존/새 checkout 여부와
설정한 기준 ref 또는 선택한 소스 브랜치의 `HEAD`가 표시됩니다.

```
╭ worktree PROJ-142 — start an agent? ──────╮
│▸ 1. no agent — just open the worktree     │
│  2. claude      claude                    │
│  3. codex       codex                     │
╰───────────────────────────────────────────╯
```

herdr는 checkout을 UI workspace로 엽니다. Git worktree는 프로젝트와
묶이지만 jj checkout은 일반 herdr UI workspace를 사용합니다. 1단계를 건너뛰려면
`[worktree.repos]`에 Jira 프로젝트별 저장소를 지정하세요. 전체 옵션은
[Git worktree와 jj workspace (`w`)](#git-worktree와-jj-workspace-w)를 참고하세요.

## 기능

- **필터** — 설정 파일에 이름을 붙여 둔 JQL 필터(`f` 또는 `1`–`9`): 내 이슈,
  특정 프로젝트 등 JQL로 표현할 수 있는 무엇이든.
- **검색** — `/`는 기본적으로 Jira 전체에서 `text ~ "…"`를 검색합니다.
  `[search].jql`로 프로젝트 범위를 제한할 수 있습니다. `J`는 현재 쿼리를
  미리 채운 raw JQL 입력이며, `r`은 그 쿼리를 다시 불러옵니다.
- **에픽** — 에픽은 자홍색 타입 배지와 `▸` 표시로 구분됩니다. `→`로 펼치면
  하위 이슈가 목록 안에 표시되고(`parent = …`, Server/DC는 `"Epic Link"`로
  대체), `←`로 접습니다.
- **이슈 상세** — `Enter`는 목록 행 변화와 별개로 선택한 이슈를 새로 읽어
  설명을 보여 줍니다(Cloud ADF는 일반 텍스트로 변환).
- **댓글** — 상세 화면에 최신순으로 표시되며 `Tab`이나 클릭으로 스크롤
  패널을 고릅니다. 캐시된 댓글은 목록에서 `r` 후 다시 열어 갱신합니다.
- **상태 전환** — `s`는 이슈에 가능한 전환 목록을 보여 주고 고른 것을 적용합니다.
  상태 전환은 한 번에 하나만 실행하며, 그동안 조회나 다른 이슈의 전환 선택은
  가능합니다. 완료 시 다른 선택창을 닫지 않고 현재 쿼리를 새로 읽습니다.
- **에이전트에게 위임** — `d`는 herdr에서 실행 중인 에이전트(claude, codex,
  grok 등)를 상태와 cwd와 함께 보여 줍니다. 하나를 고르면 인증된 Jira의
  크기가 제한된 스냅샷을 준비하고 전송 전에 프롬프트를 확인합니다. 또는
  **+ start new agent…**(`n`)로 종류와 디렉터리를 고르면 herdr가 `pane run`으로
  에이전트를 시작하고 준비된 뒤 확인한 프롬프트를 보냅니다.
- **Checkout** — `w`는 선택한 이슈의 Git worktree나 네이티브 jj workspace를
  만들거나 다시 엽니다. 프로젝트를 고르고(현재 프로젝트가 맨 위), **+ new**
  (이름은 이슈 키로 미리 채워짐)나 기존 checkout을 고른 다음, 원하면 그 안에서 Jira
  프롬프트를 받을 에이전트를 시작합니다.

Jira Cloud(이메일 + API 토큰)와 Jira Server / Data Center(개인 액세스 토큰)를
지원합니다. Cloud에서는 가능하면 새 `/rest/api/2/search/jql` 엔드포인트를 쓰고,
첫 Cloud 요청이 404/405/410일 때만 기존 `/rest/api/2/search`로 전환합니다.
그 밖의 실패는 오류로 남습니다.

읽기 동작 개선: 목록, 에픽 하위 이슈, 상세 화면 댓글을 제한된 페이지로 읽습니다.
`[jira].max_results`는 해당 작업당 전체 최대 개수이지 페이지 크기가 아닙니다.
위임 스냅샷의 댓글은 별도의 `[delegate].max_comments` 한도를 사용합니다.
나머지가 있으면 일부 결과임을 표시하며 전체 결과인 것처럼 보이지 않습니다.
로딩/오류와 정상적인 빈 결과는 구분되고, 오류 뒤 남은 캐시 데이터는 오래된
결과입니다. `r`은 현재 검색/JQL/필터를 새로 읽고 상세 진입은 이슈를 다시 읽습니다.
각 페이지의 최신 `total`을 사용하며, 값이 생략되면 이전 total도 버립니다.
빈 Cloud 페이지도 토큰이 전진하면 제한된 요청 예산 안에서 계속 읽습니다.
반복 토큰, 필수 후속 페이지 정보 누락, 모순된 total, 중복 issue key/comment ID는
완료 결과가 아니라 오류입니다. 단건 이슈 조회는 `fields` 객체가 필수이며,
그 안의 description이 null이거나 없으면 정상적인 빈 본문입니다.

Handoff 변경 사항: 제한된 조회/오류 복구에 이어, 위임의 기본값은 링크만 보내는
방식에서 검토 가능한 제한된 이슈 스냅샷으로 바뀌었습니다. 위임이나 에이전트
응답은 Jira 상태를 자동 변경하지 않습니다. 직접 `s`로 결정하세요.
후속 신뢰성 수정으로 마법사 identity를 고정하고 상태 쓰기를 분리했으며,
설정을 다시 읽어도 전송되지 않은 인계의 복구 상태를 유지합니다.

## 설치

herdr **0.8.0 이상**과 설치 시 **Rust 1.89 이상** 툴체인(https://rustup.rs)이 필요합니다.

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

## 설정과 첫 이슈 찾기

원격 설치 후 현재 디렉터리에 `config.example.toml`이 생기지 않습니다.
기존 설정 파일이 있다면 사용자 정의 템플릿을 보존하면서 직접 편집하세요.
아래 `cat >` 명령을 다시 실행하면 파일이 덮어써집니다. 새 설정 파일을 만들
때는 herdr가 알려 주는 실제 설정 디렉터리에 최소 설정을 만드세요:

```sh
config_dir="$(herdr plugin config-dir han.jira-worktree)"
mkdir -p "$config_dir"
cat > "$config_dir/config.toml" <<'TOML'
[jira]
base_url = "https://yourcompany.atlassian.net"
auth = "basic"
email = "you@company.com"
api_token_cmd = "security find-generic-password -s jira-api-token -w"
TOML
```

**macOS Jira Cloud**에서는 URL/이메일을 바꾸고
<https://id.atlassian.com/manage-profile/security/api-tokens>에서 API 토큰을
발급받으세요. Keychain에 대화형으로 저장합니다(명령이 토큰 입력을 요청합니다.
토큰을 명령 인자나 셸 기록에 적지 마세요):

```sh
security add-generic-password -s jira-api-token -a "$USER" -w
```

**Linux Jira Server/Data Center**에서는 Jira의 PAT를 발급받으세요.
`secret-tool`과 잠금 해제된 libsecret 호환 Secret Service(예: GNOME
Keyring)가 필요합니다. 같은 설정 파일에서 `base_url =
"https://jira.company.com"`, `auth = "bearer"`로 바꾸고 `email`을 지운 뒤
`api_token_cmd = "secret-tool lookup service jira-api-token"`을 쓰세요.
다음 명령은 PAT를 인자가 아닌 표준 입력으로 대화형으로 받습니다:

```sh
secret-tool store --label='Jira PAT' service jira-api-token
```

**로컬 clone/link**로 설치한 경우에만 체크아웃의 전체 예시를 복사해 편집할
수 있습니다(위 최소 설정 대신). 기존 설정 파일이 있다면 덮어쓰지 말고
직접 편집하세요:

```sh
cp config.example.toml "$(herdr plugin config-dir han.jira-worktree)/config.toml"
```

herdr 액션 팔레트에서 **Jira: open (split)** 또는 **Jira: open (tab)**을
선택하세요. **My open issues**가 비어 있다면 본인에게 할당된 미해결 이슈만
보는 필터이기 때문일 수 있습니다. `J`로 raw JQL(예:
`project = PROJ ORDER BY updated DESC`)을 입력하거나 `/`로 전체 Jira를
검색하세요. `[jira].default_project = "PROJ"`를 설정해 프로젝트 필터를
쓰거나 직접 `[[filters]]`를 정의할 수도 있습니다. 변경 후 패널에서 `R`을
누르세요. `Enter`로 이슈를 읽고, `w`로 **no agent**를 선택해 에이전트 없이
worktree를 열 수 있습니다. 조회에는 저장소 매핑도 에이전트도 필요 없습니다.
`w`에서는 열려 있는 Git/jj 프로젝트를 고르거나 저장소 경로를 입력하세요.
`[worktree.repos]`는 이 선택을 생략하는 선택 설정입니다. 새 Git 브랜치의 기본 기준은 선택한 저장소의 `HEAD`이지 `origin/main`이
아닙니다. jj 기본 기준은 소스 `@`의 부모 커밋입니다.

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
| `g`/`G`, `Home`/`End` | 맨 위 / 맨 아래 (상세 화면: `g`/`G`로 선택된 패널 스크롤) |
| `Enter` | 이슈 상세 열기 |
| `Tab` / 클릭 | 이슈 상세: 설명과 댓글 중 스크롤 대상 전환 |
| `→`/`l`, `←`/`h` | 에픽 펼치기 / 접기 (하위 이슈를 목록 안에 표시) |
| `f`, `1`–`9` | 필터 전환 |
| `/` | 검색 |
| `J` | 직접 JQL 실행 (현재 쿼리가 미리 채워짐) |
| `s` | 이슈 상태 변경 |
| `d` | 실행 중인 에이전트에게 이슈 위임, 또는 새 에이전트 시작 |
| `D` | 준비 실패로 전송되지 않았을 때 남아 있는 같은 에이전트 패널만 재확인·전송 (전달 여부가 불확실한 전송 실패에는 재시도 금지) |
| `w` | Git worktree 또는 jj workspace (프로젝트 → 새/기존 checkout → 에이전트 선택) |
| `n` | delegate 선택창에서: 새 에이전트 시작 |
| `1`–`9` | 팝업(에이전트, 전환, 필터)에서 바로 선택 |
| `o` | 브라우저에서 이슈 열기 |
| `z` | Jira 패널 확대 (전체 화면 토글) |
| `r` | 현재 필터/검색/JQL 새로고침 |
| `R` | 설정 다시 읽기 (상태 전환이나 인계가 실행 중이면 거절) |
| `?` | 도움말 (이슈 상세에서는 상세 화면 단축키) |
| `Esc` | 뒤로 / 취소 |
| `q` | 종료 |

## Delegate 프롬프트 placeholder

`{key}` `{summary}` `{description}` `{comments}` `{updated}` `{snapshot_at}`
`{url}` `{status}` `{assignee}` `{reporter}` `{priority}` `{type}` `{labels}`

기본 프롬프트에는 플러그인의 Jira 인증으로 읽은 설명과 댓글, 이슈 수정 시각과
스냅샷 시각이 들어갑니다. 한도는 `max_description_chars = 6000`,
`max_comments = 50`, `max_comment_chars = 12000`입니다. Jira 본문은 **참고
자료**이지 에이전트 지시가 아닙니다. Jira 본문 안의 placeholder는 그대로
유지됩니다. 토큰, 인증 헤더, 자격 증명 명령의 출력은 프롬프트에 포함하지
않습니다. 한도가 0이면 무제한이 아니라 경고와 함께 해당 내용을 생략합니다.

`d`와 `w`의 선택창 제목과 프롬프트는 마법사 진입 시 선택한 이슈에 고정됩니다.
백그라운드 목록 갱신으로 선택이 바뀌어도 동일합니다. 에이전트를 고르면 완전한
스냅샷은 추가 확인 팝업 없이 설정된 `submit` 값에 따라
전송됩니다. 누락, 잘림, 일부 데이터만 조회된 경우에만 스크롤 가능한
HandoffConfirm 미리 보기(`j`/`k`, Page Up/Down)가 열립니다. 대상, 내용,
경고를 확인하고 `Enter`로 명시적으로 진행하거나 `Esc`로 취소하세요.
불완전한 스냅샷은 **자동 제출되지 않습니다**. 필수 스냅샷 조회 실패는 새
에이전트/worktree 작업 전에 중단되며, 사용자 정의 템플릿에서 사용하지 않는
본문 조회는 건너뜁니다. `w`의 **no agent**는 스냅샷 없이 worktree만
엽니다. 취소하거나 설정을 다시 읽은 오래된 준비 결과는 전송되지 않습니다.

`[delegate].prompt`를 직접 설정할 수도 있습니다. `{description}`과
`{comments}`를 뺀 링크만 있는 전문가용 템플릿은 두 본문 조회를 건너뜁니다.
이 경우 에이전트는 자체적으로 허가된 Jira 접근 권한이 있어야 합니다.
플러그인의 토큰으로 에이전트가 인증되는 것은 아니므로 토큰을 에이전트 환경
또는 프롬프트에 내보내거나 붙여 넣지 마세요.

`submit = true`면 `herdr agent prompt <pane-id> <text>`가 서버에서 전송과
제출을 처리합니다. `submit = false`면 `herdr pane send-text`로 패널에
텍스트만 붙여 넣으므로 직접 검토하고 Enter를 누르세요. `w` 에이전트는
`[worktree].submit`으로 덮어쓸 수 있습니다. focus 설정은 포커스만 바꾸며
제출 여부를 바꾸지 않습니다. 새 에이전트가 준비되지 않은 경우 프롬프트는
전송되지 않았으므로 `D`로 **같은 패널만** 다시 확인하고 보낼 수 있습니다.
전송 실패는 전달 여부가 불확실할 수 있습니다. 패널을 직접 확인하고 **재전송하지
마세요**. raw 입력으로 우회 전송하지 않습니다.

`R`은 전송되지 않은 인계의 복구 상태를 보존합니다. `D`는 설정을 바꿔도
**원래 프롬프트와 제출·준비 대기 정책**을 유지하며, Jira를 다시 읽거나 새
패널/worktree를 만들지 않고 같은 패널에 이어서 전송합니다. 의도적으로 새
인계를 시작하면 이전 복구 상태는 교체됩니다.

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
`agent wait --until idle`을 거친 뒤 확인한 Jira 프롬프트를 보냅니다. 감지와
idle 대기는 하나의 `wait_ready_ms` 예산을 공유합니다. 준비 실패 시 프롬프트는
**전송되지 않으며** `D`는 새 에이전트를 만들지 않고 남아 있는 패널을 다시
확인합니다. `wait_ready_ms = 0`은 확인을 끄므로 콜드 스타트에는 권장하지 않습니다.

## Git worktree와 jj workspace (`w`)

`w`는 이슈 목록·상세에서 Git worktree 또는 네이티브 jj workspace를 만들거나
다시 엽니다. 같은 루트에서 둘 다 사용 가능하면 기본값은 jj 우선이며,
`[worktree].prefer = "git"`는 우선순위만 바꿉니다. jj 실행 파일이 없어도 같은
루트에서 Git을 사용할 수 있으면 사용 가능한 Git을 선택창에 표시하고 선택합니다.
선택 후 발생한 오류에는 다른 백엔드로 전환하지 않습니다. 가장 가까운 상위
저장소 루트를 사용합니다. jj는 0.38 이상 및 workspace 목록/JSON 이스케이프
템플릿 기능이 필요합니다. Git 저장소를 백엔드로 쓰는 jj 저장소라고 해서
곧바로 Git checkout도 사용 가능한 것은 아닙니다. `Esc`는 이전 단계로 돌아갑니다:

1. **프로젝트** — Jira 프로젝트 키별 `[worktree.repos]`, 없으면
   `[worktree].repo`, 없으면 열려 있는 Git/jj 프로젝트(현재 프로젝트 우선)
   또는 **type path…**(`/`)로 저장소 경로를 고릅니다. 공통 저장소를 공유하는
   프로젝트는 중복 표시하지 않습니다. jj workspace는 네이티브 jj checkout이며
   herdr UI workspace와는 서로 다른 개념입니다.
하위 디렉터리에서 열린 UI 패널도 가장 가까운 checkout 루트가 같으면 열린
checkout으로 인식합니다. 중첩 저장소와 서로 다른 sibling checkout 루트는
별도로 취급합니다.
2. **Checkout** — Git은 브랜치 이름, jj는 workspace 이름으로 기존 checkout을
   고르거나 **+ new**에서 이슈 키로 미리 채운 이름을 수정합니다(`Ctrl-U`로
   지우기). 일치하는 기존 항목이 미리 선택되고 `[open]`은 herdr에서 그 경로가
   열린 상태입니다. 오래된 jj workspace 메타데이터에는 최신 jj 실행 파일을
   사용해도 기존 workspace 경로가 없을 수 있습니다. 이때 경로를 입력하세요.
   저장소와 workspace 이름을 검증하며, 동일성을 명확히 확인할 수 없으면 열기를
   거부합니다. 경로를 추측하거나 같은 이름의 checkout을 다시 만들지 않습니다.
3. **에이전트** — **no agent**는 Jira 스냅샷 없이 열고, 에이전트를 고르면 `d`와
   같은 제한된 문맥을 checkout 생성 **전에** 확인합니다. 선택창에는 백엔드,
   이름, 기존/새 checkout, 기준이 표시됩니다. Git 새 브랜치 기본 기준은 소스
   `HEAD`(암묵적 `origin/main` 아님), 설정한 Git `base`는 `{key}` 등을
   치환하지 않는 **그대로의 ref**입니다. jj에서 기준을 비우면 소스 `@`의
   부모 커밋(`@` 자체가 아님)입니다. `[worktree].submit`은
   `[delegate].submit`을 덮어쓰며 `false`면 붙여 넣기만 합니다.

`config.toml`에서 `[[delegate.agents]]` 뒤에 다음 테이블을 추가하세요:

```toml
[worktree]
# repo = "~/workspace/platform"      # 생략하면 herdr 프로젝트 선택
prefer = "jj"                         # 기본값; "git"은 Git 우선
branch = "{key}"                      # Git 브랜치 입력, 예: "{type}/{key}-{slug}"
# base = "origin/main"               # Git 전용 literal ref; 기본: 선택한 저장소 HEAD
# path = "~/worktrees/{branch}"      # Git 전용; 기본: herdr worktrees 디렉터리
focus = true
# label = "{key}"                    # herdr UI workspace 라벨
# trust_repository = false           # Git 전용; 직접 확인한 저장소만
# submit = false                     # 생략하면 [delegate].submit

[worktree.jj]
name = "{key}"                        # 네이티브 jj workspace 이름
# base = "main@origin"               # 생략하면 소스 @의 부모 커밋
# path = "~/jj/{name}"               # 생략하면 ~/.herdr/jj-workspaces/<repo-name>-<stable repo-key digest>/<name-slug>

[worktree.repos]
PROJ = "~/workspace/project"
```

### Git checkout

**+ new**에서 새 Git 이름을 선택했을 때에만 먼저 `worktree open`을 시도하고,
herdr가 구조화된 `worktree_not_found`를 보고하면 `worktree create`를 호출합니다.
기존 항목을 선택하면 검증된 정확한 경로로 herdr UI workspace를 열거나 재사용하며,
checkout이 사라졌더라도 다시 만들지 않습니다:

```sh
herdr worktree open --cwd <repo> --branch <branch> [--label <label>] --focus|--no-focus
herdr worktree create --cwd <repo> --branch <branch> [--base <ref>] [--path <path>] [--label <label>] --focus|--no-focus
```

Git worktree는 저장소와 묶인 herdr UI workspace로 열립니다.
`trust_repository = true`는 검증한 Git 저장소에 한해 두 명령에
`--trust-repository`를 전달합니다. Git 정리:

```sh
herdr worktree remove --workspace <id>   # 미커밋 변경이 있으면 --force 추가
```

### 네이티브 jj checkout

플러그인은 bookmark를 만들거나 소스 checkout을 이동하지 않고 jj 명령으로
workspace를 생성합니다. jj `name`은 bookmark가 아닌 workspace 이름입니다:

```sh
jj -R <source> workspace add --name <name> [-r <base>] <destination>
```

jj의 기본 네이티브 colocation 동작을 그대로 사용하므로 jj 자체가 연관된 Git
worktree를 만들 수도 있습니다. jj 생성에 `herdr worktree create`를 사용하지
않습니다. 기본 경로는 herdr의 Git worktrees 디렉터리와 무관한
`~/.herdr/jj-workspaces/<repo-name>-<stable repo-key digest>/<name-slug>`입니다.
생성 후 네이티브 이름과 경로를 확인하며 충돌한 경로를 덮어쓰지 않습니다.
herdr의 일반 UI workspace로 열기 때문에 Git의 내장 프로젝트 묶음과 같지
않습니다. 이미 열린 UI workspace는 재사용하되 에이전트는 사용 중인 패널에
입력하지 않고 새 탭에서 시작합니다. 일반 workspace 생성 시 herdr의 기존
신뢰 검사를 따르며 jj에는 `trust_repository`로 이를 건너뛸 수 없습니다.

jj 생성은 `~/.herdr/jj-workspaces/.native-receipts-v1`의 jj 전용 기록을
사용합니다. native add 전에 `Pending`을 영속 기록하고, 종료 코드가 0이며
새 native 동일성을 검증하고 `Ready`를 영속 기록한 뒤에만 herdr UI에 연결합니다.
add가 실패하면 부분 생성 파일을 보존하고 항상 오류를 반환합니다(`Failed` 기록이
실패하면 `Pending`이 남을 수 있습니다). `Ready` 이후 UI 열기가 실패했다면
같은 이름·경로·기준의 **New** 요청으로 다시 열 수 있습니다. 앱 재시작 뒤에도
현재 native 동일성과 일치하는 `Ready`를 확인한 경우에만 add를 건너뜁니다.
`Pending`/`Failed` 또는 Ready 확정 전 중단 상태에서는 성공을 추측하거나
자동 재시도·삭제·reset하지 않습니다. 경로와 native 상태를 직접 점검한 뒤
**Existing**을 선택하고 확인 화면에서 대문자 `I`로 이번 요청의 복구 열기만
승인하세요(`Enter`, 숫자 `1`, 소문자 `i`는 승인이 아닙니다). 이 승인은 기록을
`Ready`로 바꾸지 않습니다. 외부에서 정상 생성된 Existing은 이 승인이 필요
없습니다. 같은 이름의 다른 checkout, 경로 교체, 모호한 jj 이름은 거부됩니다.
정리 전 상태를 검토하고 jj의 네이티브 workspace 명령을 사용하세요. jj에
`herdr worktree remove`를 사용하지 마세요.

### Checkout placeholder

두 이름은 `{key}`, `{slug}`, `{type}`을 받습니다. Git `branch`는 유효한 Git
ref로 정리되고 jj `name`은 네이티브 workspace 이름입니다. `label`, Git
`path`, jj `path`에는 중립적인 `{name}`을 사용할 수 있습니다. `{branch}`는
Git 전용이며 jj 라벨에서는 오류가 납니다. 요약에 ASCII 영문자·숫자가 없으면
`{slug}`는 빈 값이므로 `{key}-{slug}`는 키만 남습니다.

### 수동 확인 체크리스트

다음은 권장 관찰 항목이며 실제 Jira 운영 검증이나 사용자 연구 완료 기록이
아닙니다. 전체 세션 수가 아니라 **실제 기회 수**를 분모로 기록하세요.

- macOS Cloud 또는 Linux Server/DC의 실제 인증 계정으로 온보딩: 토큰을
  설정 파일·명령 인자·셸 기록에 남기지 않고 비밀 저장소 조회가 되는지 확인.
  패널에서 빈 My open issues 대신 `J`/`/`로 찾고 댓글 포함 상세를 열어 보기.
  첫 이슈 문맥 확인 성공 / 접근 가능한 이슈 열기 실제 시도 수 기록.
- 첫 근거 기반 handoff: 적절한 이슈에서 완전한 스냅샷의 `d` 전송을 확인하거나,
  문맥이 불완전하면 HandoffConfirm 미리 보기에서 설명, 제한된 댓글,
  수정·스냅샷 시각과 경고를 보고 적절할 때만 확정. 에이전트의 답변이 실제
  이슈 문맥을 근거로 삼는지 확인. 근거 있는 handoff / 실제 handoff 기회 수 기록.
- 짝을 이룬 작업: 적절한 저장소에서 `w`의 no agent와 에이전트 시작을 각각
  시도. 표시된 저장소/백엔드/이름/기준 또는 기존 checkout과 `submit = false`/
  `submit = true`, focus 덮어쓰기를 확인. 성공한 짝 작업 / 실제 짝 작업 기회
  수 기록.
- 다음 관련 이슈에서 안내 없이 `d`/`w`를 다시 선택하는지 관찰하고 자발적
  재사용 / 이후 실제 관련 기회 수 기록. 기회가 없음을 실패로 세거나 이
  체크리스트로 성공률을 주장하지 마세요. 상태 `s`는 에이전트 출력으로 자동
  변경되지 않으므로 직접 확인·선택하세요. 불필요한 Git worktree는 미커밋 변경을 검토한 후
  `herdr worktree remove --workspace <id>`로 직접 정리하고, jj는 jj의
  네이티브 workspace 정리 명령을 사용하세요.

## 라이선스

MIT — [LICENSE](LICENSE) 참고. 원저작물 © Vitalii Rudnykh.

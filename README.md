# herdr-jira-worktree

**English** | [한국어](README.ko.md)

Fork of [a2u/herdr-jira](https://github.com/a2u/herdr-jira) by Vitalii Rudnykh,
adding a `w` key that checks out a Jira issue into a new or existing git
worktree per herdr project, and issue comments in the details view. Versioned
independently of upstream (this fork starts at 0.1.0).

A Jira TUI that lives in a [herdr](https://herdr.dev) pane: browse issues through
configurable JQL filters, search, change issue status, and delegate an issue to
any AI agent in herdr with one key — pick a running agent, or start a new one
in a chosen directory. The agent receives a prompt built from a configurable
template (issue key, summary, description, link, …).

```
╭ Jira — My open issues (23) ─────────────────────────────────────────╮
│ KEY         STATUS        ASSIGNEE          UPDATED          SUMMARY│
│ PROJ-142    In Progress   Vitalii R.        2026-07-14 10:02 Fix …  │
│ PROJ-137    To Do         Vitalii R.        2026-07-13 18:40 Add …  │
╰─────────────────────────────────────────────────────────────────────╯
 Enter open · w worktree · d delegate · s status · f filters · / search · ? help
```

## Features

- **Filters** — named JQL filters from the config (`f` or `1`–`9`): my issues,
  a specific project, anything JQL can express.
- **Search** — `/` runs a `text ~ "…"` search (template configurable), and `J`
  runs any raw JQL you type, prefilled with the current query for quick tweaks.
- **Epics** — epics stand out with a magenta type badge and a `▸` marker;
  `→` expands one inline to show its child issues (`parent = …`, with a
  `"Epic Link"` fallback for Server/DC), `←` collapses it.
- **Issue details** — `Enter` opens a scrollable view with the description
  (Cloud ADF documents are flattened to plain text).
- **Comments** — the issue details view (`Enter`) shows the issue's comments
  (newest first) in a pane under the description; `Tab` switches scrolling
  between the description and the comments. Comments are cached per issue;
  press `r` in the issue list, then reopen the issue to fetch them again.
- **Status transitions** — `s` lists the transitions available for the issue
  and applies the one you pick.
- **Delegate to an agent** — `d` lists agents currently running in herdr
  (claude, codex, grok, …) with status and cwd; pick one and the issue is sent
  as a prompt from your `[delegate].prompt` template and submitted server-side
  (configurable). Or choose **+ start new agent…** (`n`) to pick an agent type
  and working directory — herdr runs it via `pane run` and the same Jira
  prompt is sent as soon as the agent is ready.
- **Worktree** — `w` creates or reopens a git worktree for the selected issue,
  per project like herdr's own worktree creation: pick one of herdr's projects
  (current first), then **+ new worktree** (name prefilled with the issue key)
  or one of its existing worktrees, then optionally start an agent in it that
  receives the Jira prompt.

Works with Jira Cloud (email + API token) and Jira Server / Data Center
(personal access token). Cloud's newer `/rest/api/2/search/jql` endpoint is
used when available, with automatic fallback to the classic `/rest/api/2/search`.

## Install

Requires herdr **0.8.0+** and a Rust toolchain (https://rustup.rs) at install time.

This fork (plugin id `han.jira-worktree`, with the `w` worktree key) installs with:

```sh
herdr plugin install hanbong5938/herdr-jira-worktree
```

or for local development:

```sh
git clone git@github.com:hanbong5938/herdr-jira-worktree.git
herdr plugin link ./herdr-jira-worktree
```

The upstream plugin (`a2u/herdr-jira`, id `herdr-jira`) is a separate plugin; both
can be installed side by side.

## Configure

```sh
mkdir -p "$(herdr plugin config-dir han.jira-worktree)"
cp config.example.toml "$(herdr plugin config-dir han.jira-worktree)/config.toml"
```

Edit `config.toml`:

```toml
[jira]
base_url = "https://yourcompany.atlassian.net"
auth = "basic"                      # "bearer" for Server/DC PAT
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
submit = true          # submit server-side; false only pastes the text

# default_cwd = "~/Work"
placement = "tab"      # "tab" | "right" | "down"
focus_new = false
startup_delay_ms = 1500
wait_ready_ms = 30000

# Agents you can spawn from the delegate picker ("+ start new agent…")
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

For Jira Cloud, create an API token at
<https://id.atlassian.com/manage-profile/security/api-tokens> and store it in
the macOS Keychain so it never touches the config file:

```sh
security add-generic-password -s jira-api-token -a "$USER" -w '<TOKEN>'
```

The running pane reloads the config on `R`.

## Open the pane

From the herdr action palette: **Jira: open (split)** or **Jira: open (tab)** —
or bind a key in `~/.config/herdr/config.toml`:

```toml
[[keys.command]]              # open in a split beside your work
key = "prefix+j"
type = "plugin_action"
command = "han.jira-worktree.open-jira"

[[keys.command]]              # …or in its own tab
key = "prefix+shift+j"
type = "plugin_action"
command = "han.jira-worktree.open-jira-tab"
```

(then `herdr server reload-config`)

## Keys

| Key | Action |
| --- | --- |
| `j`/`k`, `↑`/`↓` | move / scroll |
| `PgUp`/`PgDn` | move / scroll by 15 |
| `g`/`G`, `Home`/`End` | top / bottom (details: `g` scrolls to top) |
| `Enter` | open issue details |
| `Tab` | issue details: switch scrolling between description and comments |
| `→`/`l`, `←`/`h` | expand / collapse an epic (shows its child issues inline) |
| `f`, `1`–`9` | switch filter |
| `/` | search |
| `J` | run a custom JQL query (prefilled with the current one) |
| `s` | change issue status |
| `d` | delegate issue to a running agent, or start a new one |
| `w` | git worktree for the issue (project → new/existing worktree → optional agent) |
| `n` | in the delegate picker: start a new agent |
| `1`–`9` | quick pick inside any popup (agents, transitions, filters) |
| `o` | open issue in the browser |
| `z` | zoom the Jira pane (fullscreen toggle) |
| `r` | refresh current filter |
| `R` | reload config |
| `?` | help |
| `Esc` | back / cancel |
| `q` | quit |

## Delegate prompt placeholders

`{key}` `{summary}` `{description}` `{url}` `{status}` `{assignee}`
`{reporter}` `{priority}` `{type}` `{labels}`

With `submit = true`, `herdr agent prompt <pane-id> <text>` delivers and
submits the prompt server-side, avoiding a race between pasted text and Enter.
With `submit = false`, `herdr pane send-text <pane-id> <text>` only pastes the
text. The legacy `submit_delay_ms` setting is accepted but no longer used.
`[worktree].submit` overrides this for agents started by `w` (see below).
Delivery errors are shown without retrying through raw input, to avoid duplicate
prompts or typing into a startup dialog.

### Starting a new agent

From the delegate picker, **+ start new agent…** (or `n`) opens a short wizard:

1. **Agent type** — from `[[delegate.agents]]` (name + `command` argv).
2. **Space (workspace)** — pick which herdr space gets the agent (current space
   is pre-selected).
3. **Working directory** — unique cwds from running agents, optional
   `default_cwd`, plus common paths; or **type path…** for a free-text path
   (`~` and `$HOME/` expand).

With `placement = "tab"` (default) the plugin creates a new tab labelled with
the issue key and runs the agent **in that tab’s single root pane** (so you get
one terminal, not a shell + agent split):

```sh
herdr tab create --workspace <space> --cwd <dir> --label <ISSUE-KEY> --no-focus
herdr pane run <root-pane> '<command...>'
herdr agent rename <root-pane> <issue-agent-id>
```

With `placement = "right"` or `"down"` it uses `herdr pane split <parent-pane>
--direction right|down --cwd <dir> --no-focus`, then `pane run` in the returned
pane. The parent is Jira's pane if it belongs to the selected workspace;
otherwise the focused pane (or first available pane) in that workspace.
`focus_new = true` uses `--focus` for either placement.

Then it waits `startup_delay_ms`, polls `agent get` until detection, and uses
`agent wait --until idle` before sending the rendered Jira prompt. Detection
and idle share a single `wait_ready_ms` budget. If readiness fails, the prompt
is **not sent** and an error is shown. `wait_ready_ms = 0` explicitly disables
this check; it is not recommended for cold agent starts.

## Worktrees (`w`)

`w` (from the issue list or the issue details) creates — or reopens — a git
worktree for the issue, per project like herdr's own worktree creation; `Esc`
goes back one step:

1. **Project** — `[worktree.repos]` by project key (`PROJ` for `PROJ-1666`),
   else `[worktree].repo`, else you pick one of the git projects herdr has open
   (`herdr workspace list`): one row per source repository, the current
   workspace's project first (★, also when this pane runs in one of its linked
   worktrees), then the rest in workspace order, each with its source checkout
   path and number of open worktree workspaces. The last row, **type path…**
   (`/`), takes any repo directory.
2. **Worktree** — the project's checkouts (`herdr worktree list`), `[open]` when
   a herdr workspace already shows them. **+ new worktree** asks for the name,
   prefilled from the `branch` template (default `{key}`); edit it freely
   (`Ctrl-U` clears) — on `Enter` it is sanitized into a valid git branch name.
   Picking an existing worktree reopens it; the issue's worktree is
   preselected when it already exists.
3. **Agent** — **no agent** just opens the worktree workspace; or pick an agent
   from `[[delegate.agents]]` to start it there and send the Jira prompt,
   exactly like `d` (same `[delegate]` prompt/readiness settings; `submit`
   follows `[delegate].submit` unless `[worktree].submit` is set). With
   `[worktree] submit = false` the prompt is only pasted into the agent's input
   — the toast reads "… pasted into … — review and press Enter" — so you can
   edit it and press Enter yourself, while `d` keeps submitting.

Add a `[worktree]` table at the end of `config.toml` (after `[[delegate.agents]]`):

```toml
[worktree]
repo = "~/workspace/platform"          # default repo; empty/unset = pick a herdr project
branch = "{key}"                       # name prefill, e.g. "{type}/{key}-{slug}"
base = "origin/main"                   # base ref for new branches (default: HEAD)
focus = true                           # focus the worktree workspace
# path = "~/worktrees/{branch}"        # checkout path (default: herdr's worktrees dir)
# label = "{key}"                      # workspace label
# trust_repository = false             # passes --trust-repository; verified repos only
# submit = false                       # unset = [delegate].submit; false = paste only, you press Enter

[worktree.repos]                       # per Jira project key
PROJ = "~/workspace/project"
```

### Worktree checkout

The repo directory must be inside a git repo. The plugin first tries to open an
existing worktree for the branch, and only creates one if herdr reports
`worktree_not_found`:

```sh
herdr worktree open --cwd <repo> --branch <branch> [--label <label>] --focus|--no-focus
herdr worktree create --cwd <repo> --branch <branch> [--base <ref>] [--path <path>] [--label <label>] --focus|--no-focus
```

herdr opens the checkout as a new workspace grouped with the repo's workspace,
and an agent runs in its root pane. If that worktree's workspace is already
open, the agent gets a new tab labelled with the issue key there instead — the
plugin never types into a pane that may already be running an agent.
`focus` picks `--focus`/`--no-focus`. `trust_repository = true` adds
`--trust-repository` to both commands — only enable it for repositories you
have verified.

To clean up a worktree and its workspace:

```sh
herdr worktree remove --workspace <id>   # add --force for uncommitted changes
```

### Worktree placeholders

`branch`: `{key}` `{slug}` `{type}`. `path` and `label` additionally accept
`{branch}`. The name is sanitized into a valid git ref; a summary with no ASCII
letters or digits yields an empty `{slug}`, so `{key}-{slug}` falls back to
just the key.

## License

MIT — see [LICENSE](LICENSE). Original work © Vitalii Rudnykh.

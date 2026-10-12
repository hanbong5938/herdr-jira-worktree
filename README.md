# herdr-jira-worktree

**English** | [한국어](README.ko.md)

Fork of [a2u/herdr-jira](https://github.com/a2u/herdr-jira) by Vitalii Rudnykh,
adding a `w` key that checks out a Jira issue into a new or existing git
worktree per herdr project, and issue comments in the details view. Versioned
independently of upstream (this fork starts at 0.1.0).

A focused Jira issue → git worktree launcher in a [herdr](https://herdr.dev)
pane, not a replacement for Jira. Browse JQL filters, search, read issue details
and comments, then optionally open a worktree or start an agent. Status
transitions and agent delegation are explicit actions.

```
╭ Jira — My open issues (23) ─────────────────────────────────────────╮
│ KEY         STATUS        ASSIGNEE          UPDATED          SUMMARY│
│ PROJ-142    In Progress   Vitalii R.        2026-07-14 10:02 Fix …  │
│ PROJ-137    To Do         Vitalii R.        2026-07-13 18:40 Add …  │
╰─────────────────────────────────────────────────────────────────────╯
 Enter open · w worktree · d delegate · s status · f filters · / search · ? help
```

## What this fork adds

Both work out of the box — no extra config needed beyond the Jira connection.

### Issue comments

Press `Enter` on an issue. The details view shows the description on top and
the issue's comments (newest first) below it. `Tab` or a mouse click switches
which pane `j`/`k` (`PgUp`/`PgDn`, `g`/`G`) scrolls; the mouse wheel scrolls the
pane under the cursor. A scrollbar appears when a pane overflows, and ` … ` on
the comments pane's bottom border means more comments are below. `Esc` goes
back to the list.

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

### Issue → git worktree (`w`)

Press `w` on an issue (in the list or the details view) to work on it in its
own git worktree, opened as a herdr workspace. Three quick picks; `Esc` goes
back one step, `1`–`9` picks a row directly.

**1. Project** — the git projects open in herdr, the current one first (★).
`type path…` takes any other repo.

```
╭ worktree for PROJ-142 — project ─────────────────────────────────────╮
│herdr projects (source checkouts) · ★ current workspace               │
│▸ 1. ★ mobile-app  /Users/you/workspace/mobile-app  (1 open worktree) │
│  2.   backend     /Users/you/workspace/backend                       │
│  3. type path…  (/)                                                  │
╰──────────────────────────────────────────────────────────────────────╯
```

**2. Worktree** — `+ new worktree` creates a branch named after the issue (you
can edit the name before `Enter`), or pick an existing worktree to reopen it.

```
╭ worktree for PROJ-142 — mobile-app ──────────────────────────────────╮
│create a worktree or reopen one of /Users/you/workspace/mobile-app    │
│▸ 1. + new worktree (PROJ-142)                                        │
│  2. main      /Users/you/workspace/mobile-app  (source)  [open]      │
│  3. PROJ-120  /Users/you/.herdr/worktrees/mobile-app/PROJ-120  [open]│
╰──────────────────────────────────────────────────────────────────────╯
```

**3. Agent** — just open the worktree without fetching a handoff snapshot, or
pick an agent from `[[delegate.agents]]`. A complete snapshot dispatches per
the submit setting; only warnings require reviewing the bounded Jira context
before proceeding. The final picker shows repo, branch, existing checkout vs
new checkout, and the configured base or selected source branch `HEAD`.

```
╭ worktree PROJ-142 — start an agent? ──────╮
│▸ 1. no agent — just open the worktree     │
│  2. claude      claude                    │
│  3. codex       codex                     │
╰───────────────────────────────────────────╯
```

herdr opens the worktree as a new workspace grouped with the project. To skip
step 1, map Jira projects to repos in `[worktree.repos]`; see
[Worktrees (`w`)](#worktrees-w) for all options.

## Features

- **Filters** — named JQL filters from the config (`f` or `1`–`9`): my issues,
  a specific project, anything JQL can express.
- **Search** — `/` searches across Jira by default (`text ~ "…"`); configure
  `[search].jql` to restrict it to a project. `J` runs raw JQL, prefilled
  with the current query for quick tweaks; `r` refreshes that same query.
- **Epics** — epics stand out with a magenta type badge and a `▸` marker;
  `→` expands one inline to show its child issues (`parent = …`, with a
  `"Epic Link"` fallback for Server/DC), `←` collapses it.
- **Issue details** — `Enter` fetches the selected issue afresh, independently
  of changing list rows, and opens a scrollable description (Cloud ADF is
  flattened to plain text).
- **Comments** — the details pane shows newest-first comments; `Tab` or click
  selects the scrolling pane. Reopen after `r` to refresh cached comments.
- **Status transitions** — `s` lists the transitions available for the issue
  and applies the one you pick. Only one status change may be in flight; browsing
  and another issue's picker remain usable. Completion refreshes the active query
  without closing that picker.
- **Delegate to an agent** — `d` lists agents currently running in herdr
  (claude, codex, grok, …) with status and cwd; pick one to prepare a bounded
  authenticated Jira snapshot and review its prompt before delivery. Or choose
  **+ start new agent…** (`n`) to pick an agent type and working directory;
  herdr runs it via `pane run` and sends the reviewed prompt once ready.
- **Worktree** — `w` creates or reopens a git worktree for the selected issue,
  per project like herdr's own worktree creation: pick one of herdr's projects
  (current first), then **+ new worktree** (name prefilled with the issue key)
  or one of its existing worktrees, then optionally start an agent in it that
  receives the Jira prompt.

Works with Jira Cloud (email + API token) and Jira Server / Data Center
(personal access token). Cloud's newer `/rest/api/2/search/jql` endpoint is
used when available. Only an initial Cloud 404/405/410 switches to the classic
`/rest/api/2/search`; other failures remain errors.

Read reliability: lists, epic children and detail-view comments fetch in bounded
pages; `[jira].max_results` is their **total cap per operation**, not a page
size. Handoff comments use `[delegate].max_comments` independently. When more
results exist, the UI marks the result partial rather than implying
it is complete. Loading and errors are shown separately from a successful
empty result; cached data retained after a failure is stale, not current.
`r` refreshes the active search/JQL/filter; opening details fetches the issue
again.
Each page's latest `total` replaces earlier values, including an omitted total.
Empty Cloud pages continue when their token advances, within a bounded request
budget. Repeated tokens, missing required continuation metadata, inconsistent
totals, and duplicate issue keys/comment IDs are errors, not complete results.
Fresh issue reads require a `fields` object; a null/missing description in that
object is valid empty content.

Handoff change note: after bounded read/recovery behavior, delegation now
prepares a reviewed, bounded issue snapshot instead of a link-only default.
Follow-up reliability fixes pin wizard identity, isolate status writes, and
preserve known-not-sent recovery across config reload.
Neither handoff nor an agent response changes Jira status automatically;
use `s` to make that decision yourself.

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

## Configure and find your first issue

Remote install does not put `config.example.toml` in your current directory.
If you already have a config, edit it in place and preserve your custom templates;
do not rerun the `cat >` command below, which overwrites the file. For a new
config, create a minimal one in the actual herdr-managed directory:

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

On **macOS Jira Cloud**, replace URL/email, create an API token at
<https://id.atlassian.com/manage-profile/security/api-tokens>, then store it
interactively in Keychain (the command prompts for the token; do not put it
in command arguments or shell history):

```sh
security add-generic-password -s jira-api-token -a "$USER" -w
```

On **Linux Jira Server/Data Center**, obtain a PAT from your Jira instance.
Install `secret-tool` and a working libsecret-compatible Secret Service
(for example GNOME Keyring, unlocked in your session). Change the same config
to `base_url = "https://jira.company.com"`, `auth = "bearer"` (remove `email`),
and `api_token_cmd = "secret-tool lookup service jira-api-token"`. Store the PAT
interactively; the command reads it from standard input, not argv:

```sh
secret-tool store --label='Jira PAT' service jira-api-token
```

For a **local clone/link** only, instead of the minimal config above you may
copy the full example from the checkout and edit it. If a config already exists,
edit it instead of copying over it:

```sh
cp config.example.toml "$(herdr plugin config-dir han.jira-worktree)/config.toml"
```

Open **Jira: open (split)** or **Jira: open (tab)** from the herdr action palette.
If **My open issues** is empty, that filter only shows unresolved issues assigned
to you: press `J` for raw JQL (for example
`project = PROJ ORDER BY updated DESC`), or `/` for global text search. You
can also set `[jira].default_project = "PROJ"` and use the project filter, or
define your own `[[filters]]` in config. Press `R` in the pane after edits.
Select an issue with `Enter` to read it; `w` can open a worktree with **no
agent**, and neither a repository mapping nor an AI agent is needed to browse.
For `w`, choose an open herdr git project or type a repo path; optional
`[worktree.repos]` mappings only skip that choice. New branches start from the
selected repo's `HEAD` by default, **not** implicitly `origin/main`.

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
| `g`/`G`, `Home`/`End` | top / bottom (details: `g`/`G` scroll the focused pane) |
| `Enter` | open issue details |
| `Tab` / click | issue details: switch scrolling between description and comments |
| `→`/`l`, `←`/`h` | expand / collapse an epic (shows its child issues inline) |
| `f`, `1`–`9` | switch filter |
| `/` | search |
| `J` | run a custom JQL query (prefilled with the current one) |
| `s` | change issue status |
| `d` | delegate issue to a running agent, or start a new one |
| `D` | after a known-not-sent readiness failure: recheck and send to the retained same agent pane only (never retry an uncertain send failure) |
| `w` | git worktree for the issue (project → new/existing worktree → optional agent) |
| `n` | in the delegate picker: start a new agent |
| `1`–`9` | quick pick inside any popup (agents, transitions, filters) |
| `o` | open issue in the browser |
| `z` | zoom the Jira pane (fullscreen toggle) |
| `r` | refresh current filter, search or JQL |
| `R` | reload config (refused while a status change or handoff is dispatched) |
| `?` | help (in issue details: the details view's keys) |
| `Esc` | back / cancel |
| `q` | quit |

## Delegate prompt placeholders

`{key}` `{summary}` `{description}` `{comments}` `{updated}` `{snapshot_at}`
`{url}` `{status}` `{assignee}` `{reporter}` `{priority}` `{type}` `{labels}`

The default prompt includes the issue description and comments fetched with
the plugin's Jira credentials, alongside the issue update time and snapshot
time. It is bounded by `max_description_chars = 6000`, `max_comments = 50`,
and `max_comment_chars = 12000`. Jira text is **reference data**, not agent
instructions; placeholders inside Jira content stay literal. Tokens, auth
headers, and credential-command output are not part of the prompt. A zero
limit omits that content with a warning; it never means unlimited.

`d` and `w` keep picker headings and the prepared prompt tied to the issue
selected at wizard entry, even if a background refresh changes the list.
When an agent is selected, a complete snapshot dispatches without an extra
confirmation popup, using the configured `submit` setting. Only omission,
truncation, or partial-data warnings open a scrollable HandoffConfirm preview
(`j`/`k`, Page Up/Down). Check its destination, context, and warnings; `Enter`
explicitly proceeds, `Esc` cancels. An incomplete snapshot is **never
auto-submitted**. Failed required snapshot reads abort before new agent/worktree
side effects; unused reads for a custom template are skipped. `w` with **no
agent** opens the worktree without a snapshot. A cancelled or reloaded
preparation cannot send a stale prompt.

You may explicitly replace `[delegate].prompt` with a custom template. For
example, an expert link-only template omits `{description}` and `{comments}`,
so those reads are skipped; the agent must independently have authorized Jira
access to obtain the context. The plugin's token does **not** authorize agents:
do not export or paste it into their environment or prompts.

With `submit = true`, `herdr agent prompt <pane-id> <text>` delivers and
submits server-side, avoiding the pasted-text/Enter race. With `submit = false`,
`herdr pane send-text <pane-id> <text>` only pastes the prompt for review and
manual Enter. `[worktree].submit` overrides this for `w` agents; focus settings
affect only which pane/workspace gets focus, not submission. If a newly started
agent does not become ready, the prompt was not sent: `D` rechecks and sends
only to that retained same pane. A send failure can have unknown delivery:
inspect the pane manually, **do not retry** the prompt. No raw-input fallback
is used.

`R` preserves known-not-sent recovery. `D` retains the **original prompt and
submission/readiness policy**, even after config edits, and resumes the same
pane without rereading Jira or creating another pane/worktree. Starting a new
intentional handoff replaces that recovery record.

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
`agent wait --until idle` before sending the reviewed Jira prompt. Detection
and idle share a single `wait_ready_ms` budget. If readiness fails, the prompt
is **not sent**; `D` rechecks the retained pane without starting another agent.
`wait_ready_ms = 0` explicitly disables the check and is not recommended for
cold agent starts.

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
3. **Agent** — **no agent** just opens the worktree without reading a Jira
   snapshot; or pick an agent from `[[delegate.agents]]` and review the same
   bounded context as `d` before opening the worktree/starting the agent.
   The picker displays the selected repo and branch, whether checkout exists
   or will be created, and the configured base ref or source branch `HEAD`
   (not an assumed `origin/main`). `submit` follows `[delegate].submit` unless
   `[worktree].submit` is set. With `[worktree] submit = false`, the prompt
   is pasted only; edit/review and press Enter in the agent yourself.

Add a `[worktree]` table at the end of `config.toml` (after `[[delegate.agents]]`):

```toml
[worktree]
# repo = "~/workspace/platform"        # optional; unset = pick a herdr project
branch = "{key}"                       # name prefill, e.g. "{type}/{key}-{slug}"
# base = "origin/main"                 # optional override; default: selected repo's HEAD
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

### Manual validation checklist

These are suggested observations, not completed live-Jira or user-study
results. Record outcomes against **actual opportunities**, not total sessions:

- Onboard with a real authorized Jira account on macOS Cloud or Linux
  Server/DC: verify the secret-store command works without a token in config,
  argv or shell history; open the pane, use `J` or `/` if My open issues is
  empty, and open a detail with comments. Record successful first grounded
  issue opens / actual attempts to open an accessible issue.
- For a first grounded handoff, pick a relevant issue and observe the `d`
  dispatch for a complete snapshot, or review a HandoffConfirm preview
  (description, bounded comments, updated/snapshot times and warnings) when
  context is incomplete; confirm only if appropriate. Check that the agent
  refers to the issue's actual context. Record grounded handoffs / actual
  handoff opportunities.
- For paired work, try `w` with no agent, then an agent on a suitable repo;
  verify the repo/branch/base or existing checkout displayed, compare
  `submit = false` with `submit = true`, and observe focus override behavior.
  Record successful paired workflows / actual paired-work opportunities.
- On a later relevant issue, observe whether someone chooses `d`/`w` without
  prompting; record unprompted reuses / actual later relevant opportunities.
  Do not count absent opportunities as failures or assert a success rate from
  this checklist. Status `s` is manual, not inferred from agent output;
  remove unwanted worktrees explicitly with
  `herdr worktree remove --workspace <id>` after reviewing uncommitted changes.

## License

MIT — see [LICENSE](LICENSE). Original work © Vitalii Rudnykh.

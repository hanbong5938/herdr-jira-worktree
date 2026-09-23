//! Talk to the herdr server through its CLI (`HERDR_BIN_PATH`), as the plugin
//! docs recommend. Used to list running agents, start a new agent, and hand an
//! issue prompt to one.

use serde_json::Value;
use std::process::Command;
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub struct HerdrAgent {
    /// Pane id: terminal ids are not valid `agent *` targets on herdr 0.8+.
    pub target: String,
    pub label: String,
    pub status: String,
    pub cwd: String,
    pub pane_id: String,
}

/// How to place a newly started agent pane.
#[derive(Debug, Clone)]
pub struct StartAgentOpts {
    pub name: String,
    pub cwd: String,
    pub argv: Vec<String>,
    /// "tab" | "right" | "down".
    pub placement: String,
    pub focus: bool,
    /// Workspace for tab/split placement; required there. Ignored (may be
    /// empty) when `worktree` is Some: herdr opens the worktree in its own
    /// workspace.
    pub workspace_id: String,
    /// Optional label for a newly created tab (usually the issue key).
    pub tab_label: String,
    /// Some = run the agent in a git worktree of `cwd` (the repo dir); `placement` is ignored.
    pub worktree: Option<WorktreeOpts>,
}

/// How to open or create the git worktree a new agent runs in.
#[derive(Debug, Clone, Default)]
pub struct WorktreeOpts {
    /// Branch name; already rendered and sanitized, non-empty.
    pub branch: String,
    /// Base ref for a new branch; empty = omit `--base` (herdr default).
    pub base: String,
    /// Checkout path, already expanded; empty = omit `--path` (herdr default).
    pub path: String,
    /// Workspace label; empty = omit `--label`.
    pub label: String,
    /// Append `--trust-repository` to both `worktree open` and `worktree create`.
    pub trust_repository: bool,
}

/// A worktree workspace returned by `herdr worktree open|create`.
#[derive(Debug, Clone)]
pub struct OpenedWorktree {
    pub workspace_id: String,
    /// Root pane of the worktree workspace.
    pub pane_id: String,
    pub checkout_path: String,
    /// The workspace was already open; its root pane may be running an agent.
    pub already_open: bool,
}

#[derive(Debug, Clone)]
pub struct HerdrWorkspace {
    pub id: String,
    pub label: String,
    pub number: u64,
    pub focused: bool,
    pub tab_count: u64,
    pub pane_count: u64,
    pub agent_status: String,
    /// Git checkout of the workspace; None for non-git workspaces.
    pub repo: Option<WorkspaceRepo>,
}

/// The `worktree` object of a `workspace list` entry.
#[derive(Debug, Clone)]
pub struct WorkspaceRepo {
    /// Shared by the source checkout and all its linked worktrees (`…/.git`).
    pub repo_key: String,
    pub repo_name: String,
    /// Source checkout root, also for linked worktrees.
    pub repo_root: String,
    #[allow(dead_code)]
    pub checkout_path: String,
    pub is_linked_worktree: bool,
}

/// A source repository ("project") herdr has open in at least one workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoProject {
    pub name: String,
    /// Source checkout root; the `--cwd` for `herdr worktree` commands.
    pub root: String,
    pub repo_key: String,
    /// Workspaces open on linked worktrees of this repo.
    pub open_worktrees: usize,
    /// The workspace hosting this Jira pane belongs to this repo.
    pub current: bool,
}

/// One checkout from `herdr worktree list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoWorktree {
    pub branch: String,
    pub path: String,
    /// A herdr workspace is open on it.
    pub open: bool,
    /// The source checkout, not a linked worktree.
    pub is_main: bool,
}

fn herdr_bin() -> String {
    std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into())
}

#[derive(Debug)]
struct CliError {
    code: Option<String>,
    message: String,
}

impl From<CliError> for String {
    fn from(error: CliError) -> Self {
        error.message
    }
}

fn run(args: &[&str]) -> Result<Value, CliError> {
    let out = Command::new(herdr_bin())
        .args(args)
        .output()
        .map_err(|e| CliError {
            code: None,
            message: format!("failed to run herdr: {e}"),
        })?;
    parse_response(
        args,
        out.status.success(),
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    )
}

fn parse_response(
    args: &[&str],
    success: bool,
    stdout: &str,
    stderr: &str,
) -> Result<Value, CliError> {
    // Do not include command arguments: they may contain the entire Jira prompt.
    let command = args.iter().take(2).copied().collect::<Vec<_>>().join(" ");
    let parsed = serde_json::from_str::<Value>(stdout);
    // API failures are JSON on stdout (older herdr) or stderr (herdr 0.9.1),
    // even when the process exits non-zero.
    let stderr_json = serde_json::from_str::<Value>(stderr.trim()).ok();
    let api_error = [parsed.as_ref().ok(), stderr_json.as_ref()]
        .into_iter()
        .flatten()
        .map(|v| &v["error"])
        .find(|e| !e.is_null());
    if let Some(error) = api_error {
        return Err(CliError {
            code: error["code"].as_str().map(str::to_string),
            message: format!("herdr {command} failed: {error}"),
        });
    }
    if !success {
        let detail = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        return Err(CliError {
            code: None,
            message: format!("herdr {command} failed: {}", detail.trim()),
        });
    }
    // Some commands print nothing on success (e.g. mirror-pane send-keys).
    if stdout.trim().is_empty() {
        return Ok(Value::Null);
    }
    parsed.map_err(|e| CliError {
        code: None,
        message: format!("herdr {command}: bad JSON: {e}"),
    })
}

fn run_owned(args: &[String]) -> Result<Value, String> {
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    run(&refs).map_err(String::from)
}

/// List agent panes visible to herdr, excluding our own pane.
pub fn list_agents() -> Result<Vec<HerdrAgent>, String> {
    let v = run(&["agent", "list"])?;
    let own_pane = std::env::var("HERDR_PANE_ID").unwrap_or_default();
    let agents = v["result"]["agents"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|a| parse_agent(a, &own_pane))
        .collect();
    Ok(agents)
}

fn parse_agent(a: &Value, own_pane: &str) -> Option<HerdrAgent> {
    let pane_id = a["pane_id"].as_str().unwrap_or("").to_string();
    if pane_id.is_empty() || pane_id == own_pane {
        return None;
    }
    // `agent list` also returns plain panes with no detected agent
    // (no label, status "unknown") — those are not delegable.
    let label = a["display_agent"]
        .as_str()
        .or_else(|| a["agent"].as_str())?
        .to_string();
    let status = a["agent_status"].as_str().unwrap_or("unknown").to_string();
    if status == "unknown" {
        return None;
    }
    Some(HerdrAgent {
        target: pane_id.clone(),
        label,
        status,
        cwd: a["cwd"].as_str().unwrap_or("").to_string(),
        pane_id,
    })
}

/// List herdr workspaces ("spaces" in the UI).
pub fn list_workspaces() -> Result<Vec<HerdrWorkspace>, String> {
    let v = run(&["workspace", "list"])?;
    Ok(v["result"]["workspaces"]
        .as_array()
        .map(|list| list.iter().filter_map(parse_workspace).collect())
        .unwrap_or_default())
}

/// One `workspace list` entry; None without a workspace id.
fn parse_workspace(w: &Value) -> Option<HerdrWorkspace> {
    let id = w["workspace_id"].as_str().filter(|s| !s.is_empty())?;
    let text = |v: &Value| v.as_str().unwrap_or("").to_string();
    let wt = &w["worktree"];
    let repo = wt.is_object().then(|| WorkspaceRepo {
        repo_key: text(&wt["repo_key"]),
        repo_name: text(&wt["repo_name"]),
        repo_root: text(&wt["repo_root"]),
        checkout_path: text(&wt["checkout_path"]),
        is_linked_worktree: wt["is_linked_worktree"].as_bool().unwrap_or(false),
    });
    Some(HerdrWorkspace {
        id: id.to_string(),
        label: w["label"].as_str().unwrap_or("(unnamed)").to_string(),
        number: w["number"].as_u64().unwrap_or(0),
        focused: w["focused"].as_bool().unwrap_or(false),
        tab_count: w["tab_count"].as_u64().unwrap_or(0),
        pane_count: w["pane_count"].as_u64().unwrap_or(0),
        agent_status: text(&w["agent_status"]),
        repo,
    })
}

/// Source repositories herdr has open, current one first (see
/// `projects_from_workspaces`). "Current" is the workspace hosting this pane
/// (`HERDR_WORKSPACE_ID`), or the focused one outside herdr.
pub fn list_projects() -> Result<Vec<RepoProject>, String> {
    let mut ws = list_workspaces()?;
    // `workspace list` reports `"worktree": null` for some git workspaces;
    // `worktree list --workspace` still knows their repo. Non-git workspaces
    // fail the probe and stay without a repo.
    for w in ws.iter_mut().filter(|w| w.repo.is_none()) {
        if let Ok(v) = run(&["worktree", "list", "--workspace", w.id.as_str()]) {
            w.repo = repo_from_worktree_list(&v["result"], &w.id);
        }
    }
    let current = std::env::var("HERDR_WORKSPACE_ID")
        .ok()
        .filter(|id| !id.is_empty())
        .or_else(|| ws.iter().find(|w| w.focused).map(|w| w.id.clone()))
        .unwrap_or_default();
    Ok(projects_from_workspaces(&ws, &current))
}

/// The repo of `workspace_id` from its `worktree list --workspace` result:
/// `source` names the repo; the `worktrees[]` entry open in that workspace
/// gives its checkout and whether it is a linked worktree (default: the source).
fn repo_from_worktree_list(result: &Value, workspace_id: &str) -> Option<WorkspaceRepo> {
    let text = |v: &Value| v.as_str().unwrap_or("").to_string();
    let source = &result["source"];
    let repo_key = text(&source["repo_key"]);
    let repo_root = text(&source["repo_root"]);
    if repo_key.is_empty() || repo_root.is_empty() {
        return None;
    }
    let own = result["worktrees"].as_array().and_then(|list| {
        list.iter()
            .find(|w| w["open_workspace_id"].as_str() == Some(workspace_id))
    });
    Some(WorkspaceRepo {
        repo_key,
        repo_name: text(&source["repo_name"]),
        checkout_path: own
            .and_then(|w| w["path"].as_str())
            .map_or_else(|| text(&source["source_checkout_path"]), str::to_string),
        is_linked_worktree: own
            .and_then(|w| w["is_linked_worktree"].as_bool())
            .unwrap_or(false),
        repo_root,
    })
}

/// One project per repo (`repo_key`): linked-worktree workspaces fold into
/// their source repo. The repo of `current_ws_id` comes first (even when that
/// workspace is a linked worktree); the rest follow their lowest workspace number.
pub fn projects_from_workspaces(ws: &[HerdrWorkspace], current_ws_id: &str) -> Vec<RepoProject> {
    // (lowest workspace number, project)
    let mut out: Vec<(u64, RepoProject)> = Vec::new();
    for w in ws {
        let Some(r) = &w.repo else { continue };
        if r.repo_key.is_empty() || r.repo_root.is_empty() {
            continue;
        }
        let idx = match out.iter().position(|(_, p)| p.repo_key == r.repo_key) {
            Some(idx) => idx,
            None => {
                let name = if r.repo_name.is_empty() {
                    std::path::Path::new(&r.repo_root)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| r.repo_root.clone())
                } else {
                    r.repo_name.clone()
                };
                out.push((
                    w.number,
                    RepoProject {
                        name,
                        root: r.repo_root.clone(),
                        repo_key: r.repo_key.clone(),
                        open_worktrees: 0,
                        current: false,
                    },
                ));
                out.len() - 1
            }
        };
        let (first, project) = &mut out[idx];
        *first = (*first).min(w.number);
        if r.is_linked_worktree {
            project.open_worktrees += 1;
        }
        if !current_ws_id.is_empty() && w.id == current_ws_id {
            project.current = true;
        }
    }
    out.sort_by_key(|(first, p)| (!p.current, *first));
    out.into_iter().map(|(_, p)| p).collect()
}

/// Checkouts of the repo at `repo_root`: the source checkout and its linked worktrees.
pub fn list_worktrees(repo_root: &str) -> Result<Vec<RepoWorktree>, String> {
    let v = run(&["worktree", "list", "--cwd", repo_root])?;
    Ok(parse_worktrees(&v["result"]))
}

/// `worktree list` result → openable checkouts; bare, prunable and
/// branchless (detached) entries are skipped: `worktree open` needs a branch.
fn parse_worktrees(result: &Value) -> Vec<RepoWorktree> {
    let Some(list) = result["worktrees"].as_array() else {
        return vec![];
    };
    list.iter()
        .filter_map(|w| {
            let flag = |k: &str| w[k].as_bool().unwrap_or(false);
            if flag("is_bare") || flag("is_prunable") {
                return None;
            }
            let branch = w["branch"].as_str().unwrap_or("");
            let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);
            if branch.is_empty() {
                return None;
            }
            Some(RepoWorktree {
                branch: branch.to_string(),
                path: w["path"].as_str().unwrap_or("").to_string(),
                open: w["open_workspace_id"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty()),
                is_main: !flag("is_linked_worktree"),
            })
        })
        .collect()
}

/// Result of creating a tab: the tab id plus its single root shell pane.
#[derive(Debug, Clone)]
pub struct CreatedTab {
    #[allow(dead_code)]
    pub tab_id: String,
    pub pane_id: String,
}

/// Create a new tab in a workspace. Returns the tab and its root pane
/// (one terminal, reused for the configured agent command).
pub fn create_tab(
    workspace_id: &str,
    cwd: &str,
    label: &str,
    focus: bool,
) -> Result<CreatedTab, String> {
    let mut args: Vec<String> = vec!["tab".into(), "create".into()];
    if !workspace_id.is_empty() {
        args.push("--workspace".into());
        args.push(workspace_id.into());
    }
    if !cwd.is_empty() {
        args.push("--cwd".into());
        args.push(cwd.into());
    }
    if !label.is_empty() {
        args.push("--label".into());
        args.push(label.into());
    }
    if focus {
        args.push("--focus".into());
    } else {
        args.push("--no-focus".into());
    }
    let v = run_owned(&args)?;
    let result = &v["result"];
    let tab = &result["tab"];
    let root = &result["root_pane"];

    let tab_id = tab["tab_id"]
        .as_str()
        .or_else(|| result["tab_id"].as_str())
        .unwrap_or("")
        .to_string();
    let pane_id = root["pane_id"].as_str().unwrap_or("").to_string();

    if tab_id.is_empty() {
        return Err(format!("tab create: no tab_id in response: {result}"));
    }
    if pane_id.is_empty() {
        // Older herdr: fall back to pane list for this tab.
        if let Ok(pid) = first_pane_in_tab(workspace_id, &tab_id) {
            return Ok(CreatedTab {
                tab_id,
                pane_id: pid,
            });
        }
        return Err(format!(
            "tab create: no root_pane.pane_id in response: {result}"
        ));
    }
    Ok(CreatedTab { tab_id, pane_id })
}

fn first_pane_in_tab(workspace_id: &str, tab_id: &str) -> Result<String, String> {
    let v = if workspace_id.is_empty() {
        run(&["pane", "list"])?
    } else {
        run(&["pane", "list", "--workspace", workspace_id])?
    };
    let panes = v["result"]["panes"].as_array().cloned().unwrap_or_default();
    panes
        .iter()
        .find(|p| p["tab_id"].as_str() == Some(tab_id))
        .and_then(|p| p["pane_id"].as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("no pane found in tab {tab_id}"))
}

/// Shell-quote argv into a single command line for `pane run`.
fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            if a.is_empty() {
                "''".to_string()
            } else if a
                .chars()
                .any(|c| c.is_whitespace() || "\"'\\$`!&|;<>(){}[]".contains(c))
            {
                format!("'{}'", a.replace('\'', "'\\''"))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Run the configured agent command in an existing shell pane.
fn run_agent_in_pane(
    pane_id: &str,
    name: &str,
    argv: &[String],
    cwd: &str,
) -> Result<HerdrAgent, String> {
    let cmd = shell_join(argv);
    if cmd.is_empty() {
        return Err("agent command is empty".into());
    }
    // pane run submits text + Enter atomically into the existing pane.
    run(&["pane", "run", pane_id, &cmd])?;
    // Give the process a moment to become foreground before rename.
    std::thread::sleep(std::time::Duration::from_millis(300));
    // Naming is best-effort; send/wait always use the pane id.
    let _ = run(&["agent", "rename", pane_id, name]);

    let label = argv.first().cloned().unwrap_or_else(|| name.to_string());
    Ok(HerdrAgent {
        target: pane_id.to_string(),
        label,
        status: "unknown".into(),
        cwd: cwd.to_string(),
        pane_id: pane_id.to_string(),
    })
}

/// Open the worktree for `wt.branch` in the repo at `repo_cwd`, creating it
/// (and the branch, if needed) only when herdr reports `worktree_not_found`.
/// Any other error is returned as-is; nothing is retried with extra trust.
pub fn open_or_create_worktree(
    repo_cwd: &str,
    wt: &WorktreeOpts,
    focus: bool,
) -> Result<OpenedWorktree, String> {
    if repo_cwd.trim().is_empty() {
        return Err("worktree: repository directory is empty".into());
    }
    if wt.branch.trim().is_empty() {
        return Err("worktree: branch name is empty".into());
    }
    let worktree_args = |action: &str| -> Vec<String> {
        let mut args: Vec<String> = vec![
            "worktree".into(),
            action.into(),
            "--cwd".into(),
            repo_cwd.into(),
            "--branch".into(),
            wt.branch.clone(),
        ];
        if action == "create" {
            for (flag, value) in [("--base", &wt.base), ("--path", &wt.path)] {
                if !value.is_empty() {
                    args.push(flag.into());
                    args.push(value.clone());
                }
            }
        }
        if !wt.label.is_empty() {
            args.push("--label".into());
            args.push(wt.label.clone());
        }
        args.push(if focus { "--focus" } else { "--no-focus" }.into());
        if wt.trust_repository {
            args.push("--trust-repository".into());
        }
        args
    };
    let open = worktree_args("open");
    let open_refs: Vec<&str> = open.iter().map(|s| s.as_str()).collect();
    let v = match run(&open_refs) {
        Ok(v) => v,
        Err(e) if e.code.as_deref() == Some("worktree_not_found") => {
            run_owned(&worktree_args("create"))?
        }
        Err(e) => return Err(e.into()),
    };
    parse_opened_worktree(&v["result"])
}

fn parse_opened_worktree(result: &Value) -> Result<OpenedWorktree, String> {
    let non_empty = |v: &Value| v.as_str().filter(|s| !s.is_empty()).map(str::to_string);
    let root = &result["root_pane"];
    let pane_id = non_empty(&root["pane_id"])
        .ok_or_else(|| format!("worktree: no root_pane.pane_id in response: {result}"))?;
    let workspace_id = non_empty(&result["workspace"]["workspace_id"])
        .or_else(|| non_empty(&root["workspace_id"]))
        .ok_or_else(|| format!("worktree: no workspace_id in response: {result}"))?;
    let checkout_path = non_empty(&result["worktree"]["path"])
        .or_else(|| non_empty(&result["workspace"]["worktree"]["checkout_path"]))
        .or_else(|| non_empty(&root["cwd"]))
        .unwrap_or_default();
    Ok(OpenedWorktree {
        workspace_id,
        pane_id,
        checkout_path,
        already_open: result["already_open"].as_bool().unwrap_or(false),
    })
}

/// Spawn a new agent. Returns the agent identity we can later send to.
///
/// - `placement = "tab"`: create a new tab and run the agent **in its root
///   pane** (one terminal).
/// - `placement = "right"|"down"`: split a pane in the selected workspace,
///   then run the command in the new shell pane.
/// - `worktree = Some(..)`: `placement` is ignored. Open (or create) the git
///   worktree of `cwd` as its own herdr workspace and run the agent in its
///   fresh root pane; if that workspace was already open, run it in a new tab
///   there instead of touching the existing root pane. The agent's cwd is the
///   checkout path. `workspace_id` is ignored and may be empty.
pub fn start_agent(opts: &StartAgentOpts) -> Result<HerdrAgent, String> {
    if opts.argv.is_empty() {
        return Err("agent command is empty".into());
    }
    if opts.name.trim().is_empty() {
        return Err("agent name is empty".into());
    }

    let tab_label = if opts.tab_label.is_empty() {
        opts.name.as_str()
    } else {
        opts.tab_label.as_str()
    };

    if let Some(wt_opts) = &opts.worktree {
        let wt = open_or_create_worktree(&opts.cwd, wt_opts, opts.focus)?;
        let pane_id = if wt.already_open {
            create_tab(&wt.workspace_id, &wt.checkout_path, tab_label, opts.focus)?.pane_id
        } else {
            wt.pane_id
        };
        return run_agent_in_pane(&pane_id, &opts.name, &opts.argv, &wt.checkout_path);
    }

    if opts.workspace_id.trim().is_empty() {
        return Err("workspace is empty".into());
    }
    let placement = opts.placement.trim().to_ascii_lowercase();

    if placement == "tab" || placement.is_empty() {
        let tab = create_tab(&opts.workspace_id, &opts.cwd, tab_label, opts.focus)?;
        return run_agent_in_pane(&tab.pane_id, &opts.name, &opts.argv, &opts.cwd);
    }

    if placement != "right" && placement != "down" {
        return Err(format!("unsupported agent placement: {placement}"));
    }
    // `pane split` has no --workspace. Resolve an explicit parent so the
    // workspace picked in the wizard is honored even when Jira is elsewhere.
    let pane_list = run(&["pane", "list", "--workspace", &opts.workspace_id])?;
    let own_pane = std::env::var("HERDR_PANE_ID").unwrap_or_default();
    let parent = split_parent(&pane_list, &opts.workspace_id, &own_pane)
        .ok_or_else(|| format!("no pane to split in workspace {}", opts.workspace_id))?;
    let mut args = vec!["pane", "split", parent, "--direction", &placement];
    if !opts.cwd.is_empty() {
        args.extend(["--cwd", &opts.cwd]);
    }
    args.push(if opts.focus { "--focus" } else { "--no-focus" });
    let split_result = run(&args)?;
    let pane_id = split_result["result"]["pane"]["pane_id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            format!(
                "pane split: no pane.pane_id in response: {}",
                split_result["result"]
            )
        })?;
    run_agent_in_pane(pane_id, &opts.name, &opts.argv, &opts.cwd)
}

fn split_parent<'a>(v: &'a Value, workspace_id: &str, own_pane: &str) -> Option<&'a str> {
    let panes = v["result"]["panes"].as_array()?;
    panes
        .iter()
        .filter(|p| p["workspace_id"].as_str() == Some(workspace_id))
        .filter(|p| p["pane_id"].as_str().is_some_and(|id| !id.is_empty()))
        .min_by_key(|p| {
            if p["pane_id"].as_str() == Some(own_pane) {
                0
            } else if p["focused"].as_bool() == Some(true) {
                1
            } else {
                2
            }
        })
        .and_then(|p| p["pane_id"].as_str())
}

/// Wait for detection, then readiness, sharing one timeout budget. `pane run`
/// returns before herdr detects the agent; `agent wait` cannot wait for that.
pub fn wait_agent_status(target: &str, status: &str, timeout_ms: u64) -> Result<(), String> {
    let started = Instant::now();
    let budget = Duration::from_millis(timeout_ms);
    loop {
        let remaining = budget.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(format!(
                "agent {target} was not detected within {timeout_ms}ms"
            ));
        }
        match run(&["agent", "get", target]) {
            Ok(_) => break,
            Err(e) if e.code.as_deref() == Some("agent_not_found") => {
                std::thread::sleep(
                    Duration::from_millis(100).min(budget.saturating_sub(started.elapsed())),
                );
            }
            Err(e) => return Err(e.into()),
        }
    }
    let remaining_ms = budget.saturating_sub(started.elapsed()).as_millis() as u64;
    if remaining_ms == 0 {
        return Err(format!(
            "agent {target} was not ready within {timeout_ms}ms"
        ));
    }
    run(&[
        "agent",
        "wait",
        target,
        "--until",
        status,
        "--timeout",
        &remaining_ms.to_string(),
    ])?;
    Ok(())
}

/// Submit through herdr's server-side prompt operation, avoiding the race
/// between bracketed paste and a separate Enter. Never retry via raw input on
/// failure: that could duplicate a prompt or type it into a startup dialog.
/// `submit_delay_ms` is retained for config/API compatibility but is unused.
pub fn send_to_agent(
    agent: &HerdrAgent,
    text: &str,
    submit: bool,
    _submit_delay_ms: u64,
) -> Result<(), String> {
    if submit {
        run(&["agent", "prompt", &agent.target, text])?;
    } else {
        run(&["pane", "send-text", &agent.pane_id, text])?;
    }
    Ok(())
}

/// Start a new agent, wait until it looks ready, then send the prompt.
pub fn start_and_delegate(
    opts: &StartAgentOpts,
    text: &str,
    submit: bool,
    submit_delay_ms: u64,
    startup_delay_ms: u64,
    wait_ready_ms: u64,
) -> Result<HerdrAgent, String> {
    let agent = start_agent(opts)?;
    if startup_delay_ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(startup_delay_ms));
    }
    if wait_ready_ms > 0 {
        wait_agent_status(&agent.target, "idle", wait_ready_ms)
            .map_err(|e| format!("agent {} not ready; prompt not sent: {e}", agent.pane_id))?;
    }
    send_to_agent(&agent, text, submit, submit_delay_ms)?;
    Ok(agent)
}

/// Toggle fullscreen (zoom) for our own pane. Only works when running inside
/// a herdr pane (HERDR_PANE_ID is set by herdr for plugin panes).
pub fn zoom_toggle() -> Result<(), String> {
    let pane = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| "not running inside a herdr pane".to_string())?;
    run(&["pane", "zoom", &pane, "--toggle"])?;
    Ok(())
}

/// Show a herdr toast notification (best effort).
pub fn notify(message: &str) {
    let _ = Command::new(herdr_bin())
        .args(["notification", "show", message])
        .output();
}

/// Expand `~` / `$HOME` at the start of a path.
pub fn expand_path(path: &str) -> String {
    let p = path.trim();
    if p == "~" {
        return std::env::var("HOME").unwrap_or_else(|_| p.to_string());
    }
    if let Some(rest) = p.strip_prefix("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            return format!("{home}/{rest}");
        }
    }
    if let Some(rest) = p.strip_prefix("$HOME/") {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            return format!("{home}/{rest}");
        }
    }
    p.to_string()
}

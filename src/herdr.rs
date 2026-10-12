//! Talk to the herdr server through its CLI (`HERDR_BIN_PATH`), as the plugin
//! docs recommend. Used to list running agents, start a new agent, and hand an
//! issue prompt to one.

use crate::checkout::{self, Backend, Entry, Listing, Repository, Request, Selection};
use serde_json::Value;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
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

/// An unsuccessful handoff retains the pane when one was created or selected.
/// Only readiness failures are known not to have delivered the prompt.
#[derive(Debug, Clone)]
pub struct DelegationFailure {
    pub message: String,
    pub agent: Option<HerdrAgent>,
    pub retry_safe: bool,
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
    /// Workspace for tab/split placement. Checkout placement ignores it.
    pub workspace_id: String,
    /// Optional label for a newly created tab (usually the issue key).
    pub tab_label: String,
    /// Frozen selected checkout shared across handoff worker clones; ordinary launches pass None.
    pub checkout: Option<Arc<Request>>,
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
    /// Git checkout of the workspace, if herdr exposed Git metadata.
    pub repo: Option<WorkspaceRepo>,
}

/// The `worktree` object of a `workspace list` entry.
#[derive(Debug, Clone)]
pub struct WorkspaceRepo {
    /// Source checkout root, also for linked worktrees.
    pub repo_root: String,
    #[allow(dead_code)]
    pub checkout_path: String,
    pub is_linked_worktree: bool,
}

/// A source repository herdr has open in at least one workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoProject {
    pub name: String,
    pub root: String,
    pub repo_key: String,
    pub backend: Backend,
    pub open_worktrees: usize,
    pub current: bool,
}

#[derive(Debug, Clone)]
pub struct ProjectInventory {
    pub projects: Vec<RepoProject>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct OpenedCheckout {
    pub workspace_id: String,
    pub pane_id: String,
    pub checkout_path: String,
    /// The UI workspace was reused; its root pane may be busy.
    pub already_open: bool,
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

fn discover_candidate(path: &str, prefer: Backend) -> Result<Option<Repository>, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|e| format!("cannot inspect workspace directory {path}: {e}"))?;
    if !metadata.is_dir() {
        return Err(format!(
            "workspace checkout candidate is not a directory: {path}"
        ));
    }
    match checkout::resolve_repository(path, prefer) {
        Ok(repo) => Ok(Some(repo)),
        Err(error) if error.starts_with("no Git or jj checkout contains ") => Ok(None),
        Err(error) => Err(error),
    }
}

/// Discover native checkouts from stable pane cwd (never foreground_cwd).
/// Git metadata remains a fallback for herdr workspaces without a shell pane.
/// A failing candidate does not hide healthy projects from other workspaces.
pub fn list_projects(prefer: Backend) -> Result<ProjectInventory, String> {
    let mut ws = list_workspaces()?;
    let current = std::env::var("HERDR_WORKSPACE_ID")
        .ok()
        .filter(|id| !id.is_empty())
        .or_else(|| ws.iter().find(|w| w.focused).map(|w| w.id.clone()))
        .unwrap_or_default();
    let mut out: Vec<(u64, RepoProject)> = Vec::new();
    let mut warnings = Vec::new();
    for w in &mut ws {
        let mut found = Vec::new();
        match run(&["pane", "list", "--workspace", &w.id]) {
            Ok(panes) => {
                if let Some(panes) = panes["result"]["panes"].as_array() {
                    for pane in panes {
                        if let Some(cwd) = pane["cwd"].as_str().filter(|s| !s.is_empty()) {
                            match discover_candidate(cwd, prefer) {
                                Ok(Some(repo)) => {
                                    if !found.iter().any(|r: &Repository| r.key == repo.key) {
                                        found.push(repo);
                                    }
                                }
                                Ok(None) => {}
                                Err(error) => warnings.push(format!(
                                    "workspace '{}' ({}) pane cwd {cwd}: {error}",
                                    w.label, w.id
                                )),
                            }
                        } else {
                            warnings.push(format!(
                                "workspace '{}' ({}) pane {} has no stable cwd",
                                w.label,
                                w.id,
                                pane["pane_id"].as_str().unwrap_or("(unknown)")
                            ));
                        }
                    }
                } else {
                    warnings.push(format!(
                        "workspace '{}' ({}) has no readable pane inventory",
                        w.label, w.id
                    ));
                }
            }
            Err(error) => warnings.push(format!(
                "workspace '{}' ({}) pane inventory: {}",
                w.label, w.id, error.message
            )),
        }
        if found.is_empty() && w.repo.is_none() {
            match run(&["worktree", "list", "--workspace", &w.id]) {
                Ok(v) => w.repo = repo_from_worktree_list(&v["result"], &w.id),
                Err(error) => warnings.push(format!(
                    "workspace '{}' ({}) worktree metadata: {}",
                    w.label, w.id, error.message
                )),
            }
        }
        if let Some(meta) = &w.repo {
            let candidate = if meta.checkout_path.is_empty() {
                &meta.repo_root
            } else {
                &meta.checkout_path
            };
            match discover_candidate(candidate, prefer) {
                Ok(Some(repo)) => {
                    if !found.iter().any(|r: &Repository| r.key == repo.key) {
                        found.push(repo);
                    }
                }
                Ok(None) => {}
                Err(error) => warnings.push(format!(
                    "workspace '{}' ({}) checkout {candidate}: {error}",
                    w.label, w.id
                )),
            }
        }
        for repo in found {
            let idx = out
                .iter()
                .position(|(_, project)| project.repo_key == repo.key);
            if let Some(idx) = idx {
                let (first, project) = &mut out[idx];
                *first = (*first).min(w.number);
                if repo.root != project.root {
                    project.open_worktrees += 1;
                }
                if w.id == current {
                    project.current = true;
                    project.root = repo.root.clone();
                }
            } else {
                out.push((
                    w.number,
                    RepoProject {
                        name: repo.name,
                        root: repo.root,
                        repo_key: repo.key,
                        backend: repo.backend,
                        open_worktrees: usize::from(
                            repo.backend == Backend::Git
                                && w.repo.as_ref().is_some_and(|r| r.is_linked_worktree),
                        ),
                        current: w.id == current,
                    },
                ));
            }
        }
    }
    out.sort_by_key(|(first, project)| (!project.current, *first));
    let projects = out
        .into_iter()
        .map(|(_, project)| project)
        .collect::<Vec<_>>();
    if projects.is_empty() && !warnings.is_empty() {
        return Err(format!("project discovery failed: {}", warnings.join("; ")));
    }
    Ok(ProjectInventory { projects, warnings })
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
        checkout_path: own
            .and_then(|w| w["path"].as_str())
            .map_or_else(|| text(&source["source_checkout_path"]), str::to_string),
        is_linked_worktree: own
            .and_then(|w| w["is_linked_worktree"].as_bool())
            .unwrap_or(false),
        repo_root,
    })
}

/// Worktree metadata fallback when herdr does not expose a stable pane cwd.
fn git_entries(repository: &Repository) -> Result<Vec<Entry>, String> {
    let v = run(&["worktree", "list", "--cwd", &repository.root])?;
    let list = v["result"]["worktrees"]
        .as_array()
        .ok_or_else(|| "herdr worktree list did not return worktrees".to_string())?;
    Ok(list
        .iter()
        .filter_map(|w| {
            let flag = |key: &str| w[key].as_bool().unwrap_or(false);
            if flag("is_bare") || flag("is_prunable") {
                return None;
            }
            let path = w["path"].as_str().filter(|s| !s.is_empty())?.to_string();
            let branch = w["branch"].as_str().unwrap_or("");
            let branch = branch.strip_prefix("refs/heads/").unwrap_or(branch);
            let name = if branch.is_empty() {
                Path::new(&path).file_name()?.to_string_lossy().into_owned()
            } else {
                branch.to_string()
            };
            Some(Entry {
                name,
                path: Some(path),
                open: false,
                is_source: !flag("is_linked_worktree"),
            })
        })
        .collect())
}

pub fn resolve_and_list(path: &str, prefer: Backend) -> Result<Listing, String> {
    let repository = checkout::resolve_repository(path, prefer)?;
    let mut entries = match repository.backend {
        Backend::Jj => checkout::list(&repository)?,
        Backend::Git => git_entries(&repository)?,
    };
    let paths = workspace_checkout_paths(&repository)?;
    for entry in &mut entries {
        if let Some(path) = &entry.path {
            let Ok(actual) = std::fs::canonicalize(path) else {
                continue;
            };
            let owners = paths.iter().filter(|(_, root)| root == &actual).count();
            if owners > 1 {
                return Err(format!(
                    "multiple herdr workspaces use {path}; select or close one manually"
                ));
            }
            entry.open |= owners == 1;
        }
    }
    Ok(Listing {
        repository,
        entries,
    })
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

fn git_new_checkout(
    request: &Request,
    focus: bool,
    name: &str,
    base: &str,
    path: &str,
) -> Result<OpenedCheckout, String> {
    let worktree_args = |action: &str| {
        let mut args = vec![
            "worktree".to_string(),
            action.to_string(),
            "--cwd".into(),
            request.repository.root.clone(),
            "--branch".into(),
            name.to_string(),
        ];
        if action == "create" {
            for (flag, value) in [("--base", base), ("--path", path)] {
                if !value.is_empty() {
                    args.extend([flag.to_string(), value.to_string()]);
                }
            }
        }
        if !request.label.is_empty() {
            args.extend(["--label".into(), request.label.clone()]);
        }
        args.push(if focus { "--focus" } else { "--no-focus" }.into());
        if request.trust_repository {
            args.push("--trust-repository".into());
        }
        args
    };
    let open = worktree_args("open");
    let refs = open.iter().map(String::as_str).collect::<Vec<_>>();
    let response = match run(&refs) {
        Ok(value) => value,
        Err(error) if error.code.as_deref() == Some("worktree_not_found") => {
            run_owned(&worktree_args("create"))?
        }
        Err(error) => return Err(error.into()),
    };
    let result = &response["result"];
    let pane_id = result["root_pane"]["pane_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("worktree: no root_pane.pane_id in response: {result}"))?;
    let workspace_id = result["workspace"]["workspace_id"]
        .as_str()
        .or_else(|| result["root_pane"]["workspace_id"].as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("worktree: no workspace_id in response: {result}"))?;
    let checkout_path = result["worktree"]["path"]
        .as_str()
        .or_else(|| result["workspace"]["worktree"]["checkout_path"].as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("worktree: no checkout path in response: {result}"))?;
    let actual = checkout::validate_existing(&request.repository, name, checkout_path)
        .map_err(|error| format!("{error}; herdr checkout at {checkout_path} was retained"))?;
    let actual_path = actual.path.unwrap();
    if let Some(cwd) = result["root_pane"]["cwd"]
        .as_str()
        .filter(|s| !s.is_empty())
    {
        if std::fs::canonicalize(cwd).ok() != std::fs::canonicalize(&actual_path).ok() {
            return Err(format!(
                "herdr root pane cwd {cwd} differs from checkout {actual_path}; checkout retained"
            ));
        }
    }
    Ok(OpenedCheckout {
        workspace_id: workspace_id.into(),
        pane_id: pane_id.into(),
        checkout_path: actual_path,
        already_open: result["already_open"].as_bool().unwrap_or(false),
    })
}

/// A pane's configured cwd is stable; foreground_cwd reflects navigation in
/// an interactive shell and is not evidence of checkout ownership. Only the
/// closest filesystem checkout boundary can own a selected native checkout.
fn workspace_checkout_paths(
    repository: &Repository,
) -> Result<Vec<(String, std::path::PathBuf)>, String> {
    let mut paths = Vec::new();
    for workspace in list_workspaces()? {
        let mut candidates = Vec::new();
        if let Some(repo) = &workspace.repo {
            let path = if repo.checkout_path.is_empty() {
                &repo.repo_root
            } else {
                &repo.checkout_path
            };
            if path.is_empty() {
                return Err(format!(
                    "workspace '{}' ({}) has checkout metadata without a path",
                    workspace.label, workspace.id
                ));
            }
            candidates.push(path.to_string());
        }
        let panes = run(&["pane", "list", "--workspace", &workspace.id]).map_err(|error| {
            format!(
                "workspace '{}' ({}) pane inventory: {}",
                workspace.label, workspace.id, error.message
            )
        })?;
        let panes = panes["result"]["panes"].as_array().ok_or_else(|| {
            format!(
                "workspace '{}' ({}) has no readable pane inventory",
                workspace.label, workspace.id
            )
        })?;
        for pane in panes {
            let cwd = pane["cwd"]
                .as_str()
                .filter(|cwd| !cwd.is_empty())
                .ok_or_else(|| {
                    format!(
                        "workspace '{}' ({}) has a pane without a stable cwd",
                        workspace.label, workspace.id
                    )
                })?;
            candidates.push(cwd.to_string());
        }
        for candidate in candidates {
            let boundary =
                checkout::nearest_checkout_boundary(Path::new(&candidate)).map_err(|error| {
                    format!(
                        "workspace '{}' ({}) checkout path {candidate}: {error}",
                        workspace.label, workspace.id
                    )
                })?;
            if let Some(boundary) = boundary {
                let matching_backend = match repository.backend {
                    Backend::Git => boundary.has_git,
                    Backend::Jj => boundary.has_jj,
                };
                if matching_backend
                    && !paths
                        .iter()
                        .any(|(id, root)| id == &workspace.id && root == &boundary.root)
                {
                    paths.push((workspace.id.clone(), boundary.root));
                }
            }
        }
    }
    Ok(paths)
}

fn attach_checkout(
    request: &Request,
    checkout_path: &str,
    focus: bool,
) -> Result<OpenedCheckout, String> {
    let paths = workspace_checkout_paths(&request.repository)
        .map_err(|e| format!("{e}; checkout retained at {checkout_path}"))?;
    let root = std::fs::canonicalize(checkout_path).map_err(|e| format!("{checkout_path}: {e}"))?;
    let mut matches = paths
        .iter()
        .filter(|(_, path)| path == &root)
        .map(|(id, _)| id.as_str())
        .collect::<Vec<_>>();
    matches.sort_unstable();
    matches.dedup();
    if matches.len() > 1 {
        return Err(format!(
            "multiple herdr workspaces use {checkout_path}; select or close one manually"
        ));
    }
    if let Some(id) = matches.first() {
        if focus {
            run(&["workspace", "focus", id])
                .map_err(|e| format!("{}; checkout retained at {}", e.message, root.display()))?;
        }
        return Ok(OpenedCheckout {
            workspace_id: (*id).into(),
            pane_id: String::new(),
            checkout_path: root.to_string_lossy().into_owned(),
            already_open: true,
        });
    }
    let mut args = vec![
        "workspace".to_string(),
        "create".into(),
        "--cwd".into(),
        root.to_string_lossy().into_owned(),
    ];
    if !request.label.is_empty() {
        args.extend(["--label".into(), request.label.clone()]);
    }
    args.push(if focus { "--focus" } else { "--no-focus" }.into());
    let result =
        run_owned(&args).map_err(|e| format!("{e}; checkout retained at {}", root.display()))?;
    let value = &result["result"];
    let ws = value["workspace"]["workspace_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            format!(
                "workspace create: missing workspace_id; checkout retained at {}",
                root.display()
            )
        })?;
    let tab = value["tab"]["tab_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            format!(
                "workspace create: missing tab_id; checkout retained at {}",
                root.display()
            )
        })?;
    let pane = &value["root_pane"];
    let pane_id = pane["pane_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            format!(
                "workspace create: missing root pane; checkout retained at {}",
                root.display()
            )
        })?;
    if pane["workspace_id"].as_str().is_some_and(|id| id != ws)
        || pane["tab_id"].as_str().is_some_and(|id| id != tab)
    {
        return Err(format!(
            "workspace create: mismatched root pane; checkout retained at {}",
            root.display()
        ));
    }
    if pane["cwd"]
        .as_str()
        .is_some_and(|cwd| std::fs::canonicalize(cwd).ok().as_ref() != Some(&root))
    {
        return Err(format!(
            "workspace create: root pane cwd differs from {}; checkout retained",
            root.display()
        ));
    }
    Ok(OpenedCheckout {
        workspace_id: ws.into(),
        pane_id: pane_id.into(),
        checkout_path: root.to_string_lossy().into_owned(),
        already_open: false,
    })
}

/// Existing never recreates a missing selection. Native jj addition and UI
/// attachment are separate durable steps: on UI failure the checkout survives.
pub fn open_or_create_checkout(request: &Request, focus: bool) -> Result<OpenedCheckout, String> {
    if request.repository.root.is_empty() || request.name().trim().is_empty() {
        return Err("checkout: repository and name must not be empty".into());
    }
    let current =
        checkout::resolve_repository(&request.repository.root, request.repository.backend)?;
    if current.backend != request.repository.backend
        || current.key != request.repository.key
        || current.root != request.repository.root
    {
        return Err("checkout source repository has changed; reselect the project".into());
    }
    match (&request.repository.backend, &request.selection) {
        (Backend::Git, Selection::New { name, base, path }) => {
            git_new_checkout(request, focus, name, base, path)
        }
        (Backend::Jj, _) => {
            let prepared = checkout::ensure_jj(request)?;
            attach_checkout(request, prepared.entry.path.as_deref().unwrap(), focus)
        }
        (Backend::Git, Selection::Existing(entry)) => {
            let path = entry
                .path
                .as_deref()
                .ok_or_else(|| format!("Git checkout '{}' has no path", entry.name))?;
            let verified = checkout::validate_existing(&request.repository, &entry.name, path)?;
            attach_checkout(request, verified.path.as_deref().unwrap(), focus)
        }
    }
}

/// Spawn an agent into a fresh checkout root pane, or a fresh tab when its UI
/// workspace is reused; never overwrite a possibly busy existing root pane.
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

    if let Some(request) = &opts.checkout {
        let opened = open_or_create_checkout(request, opts.focus)?;
        let pane_id = if opened.already_open {
            create_tab(
                &opened.workspace_id,
                &opened.checkout_path,
                tab_label,
                opts.focus,
            )
            .map_err(|e| format!("{e}; checkout retained at {}", opened.checkout_path))?
            .pane_id
        } else {
            opened.pane_id
        };
        return run_agent_in_pane(&pane_id, &opts.name, &opts.argv, &opened.checkout_path)
            .map_err(|e| format!("{e}; checkout retained at {}", opened.checkout_path));
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
pub fn send_to_agent(agent: &HerdrAgent, text: &str, submit: bool) -> Result<(), String> {
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
    startup_delay_ms: u64,
    wait_ready_ms: u64,
) -> Result<HerdrAgent, DelegationFailure> {
    let agent = start_agent(opts).map_err(|message| DelegationFailure {
        message,
        agent: None,
        retry_safe: false,
    })?;
    if startup_delay_ms > 0 {
        std::thread::sleep(Duration::from_millis(startup_delay_ms));
    }
    resume_delegation(&agent, text, submit, wait_ready_ms)
}

/// Deliver to the same pane; never start an agent or open another worktree.
/// A zero readiness budget explicitly skips the wait.
pub fn resume_delegation(
    agent: &HerdrAgent,
    text: &str,
    submit: bool,
    wait_ready_ms: u64,
) -> Result<HerdrAgent, DelegationFailure> {
    if wait_ready_ms > 0 {
        wait_agent_status(&agent.target, "idle", wait_ready_ms).map_err(|message| {
            DelegationFailure {
                message: format!(
                    "agent {} not ready; prompt not sent: {message}",
                    agent.pane_id
                ),
                agent: Some(agent.clone()),
                retry_safe: true,
            }
        })?;
    }
    send_to_agent(agent, text, submit).map_err(|message| DelegationFailure {
        message,
        agent: Some(agent.clone()),
        retry_safe: false,
    })?;
    Ok(agent.clone())
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

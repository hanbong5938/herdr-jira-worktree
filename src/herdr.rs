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
    pub workspace_id: String,
    /// Optional label for a newly created tab (usually the issue key).
    pub tab_label: String,
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
    // API failures are JSON on stdout, even when the process exits non-zero.
    if let Ok(v) = &parsed {
        if !v["error"].is_null() {
            let error = &v["error"];
            return Err(CliError {
                code: error["code"].as_str().map(str::to_string),
                message: format!("herdr {command} failed: {error}"),
            });
        }
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
    let list = v["result"]["workspaces"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    Ok(list
        .iter()
        .map(|w| HerdrWorkspace {
            id: w["workspace_id"].as_str().unwrap_or("").to_string(),
            label: w["label"].as_str().unwrap_or("(unnamed)").to_string(),
            number: w["number"].as_u64().unwrap_or(0),
            focused: w["focused"].as_bool().unwrap_or(false),
            tab_count: w["tab_count"].as_u64().unwrap_or(0),
            pane_count: w["pane_count"].as_u64().unwrap_or(0),
            agent_status: w["agent_status"].as_str().unwrap_or("").to_string(),
        })
        .filter(|w| !w.id.is_empty())
        .collect())
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

/// Spawn a new agent. Returns the agent identity we can later send to.
///
/// - `placement = "tab"`: create a new tab and run the agent **in its root
///   pane** (one terminal).
/// - `placement = "right"|"down"`: split a pane in the selected workspace,
///   then run the command in the new shell pane.
pub fn start_agent(opts: &StartAgentOpts) -> Result<HerdrAgent, String> {
    if opts.argv.is_empty() {
        return Err("agent command is empty".into());
    }
    if opts.name.trim().is_empty() {
        return Err("agent name is empty".into());
    }
    if opts.workspace_id.trim().is_empty() {
        return Err("workspace is empty".into());
    }

    let placement = opts.placement.trim().to_ascii_lowercase();

    if placement == "tab" || placement.is_empty() {
        let label = if opts.tab_label.is_empty() {
            opts.name.as_str()
        } else {
            opts.tab_label.as_str()
        };
        let tab = create_tab(&opts.workspace_id, &opts.cwd, label, opts.focus)?;
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

//! Plugin configuration: loaded from the herdr-managed plugin config dir
//! (`HERDR_PLUGIN_CONFIG_DIR`, falling back to
//! `~/.config/herdr/plugins/config/han.jira-worktree/config.toml` for standalone runs).

use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub jira: JiraConfig,
    #[serde(default)]
    pub filters: Vec<Filter>,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub delegate: DelegateConfig,
    /// Settings for `w` (create a git worktree for the selected issue);
    /// TOML table `[worktree]`.
    #[serde(default)]
    pub worktree: WorktreeConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JiraConfig {
    pub base_url: String,
    #[serde(default = "default_auth")]
    pub auth: String, // "basic" | "bearer"
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub api_token: String,
    #[serde(default)]
    pub api_token_cmd: String,
    #[serde(default)]
    pub default_project: String,
    #[serde(default = "default_max_results")]
    pub max_results: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Filter {
    pub name: String,
    pub jql: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchConfig {
    #[serde(default = "default_search_jql")]
    pub jql: String,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            jql: default_search_jql(),
        }
    }
}

/// One agent binary that can be run in a new pane when delegating.
#[derive(Debug, Clone, Deserialize)]
pub struct SpawnAgent {
    /// Display label and default herdr agent name prefix (e.g. "claude").
    pub name: String,
    /// Argv shell-quoted for `herdr pane run` (e.g. `["claude"]`).
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DelegateConfig {
    #[serde(default = "default_prompt")]
    pub prompt: String,
    #[serde(default = "default_true")]
    pub submit: bool,
    /// Legacy setting, ignored: `agent prompt` handles submission server-side.
    #[serde(default = "default_submit_delay")]
    pub submit_delay_ms: u64,
    #[serde(default = "default_max_desc")]
    pub max_description_chars: usize,
    /// Agents offered when starting a new one (not only listing running).
    #[serde(default = "default_spawn_agents")]
    pub agents: Vec<SpawnAgent>,
    /// Preferred cwd prefilled / listed first when starting a new agent.
    #[serde(default)]
    pub default_cwd: String,
    /// Where to put a newly started agent:
    ///   "tab"   — new tab in the chosen workspace (default)
    ///   "right" — split right in the chosen workspace
    ///   "down"  — split down in the chosen workspace
    /// Legacy alias: `split` is accepted with the same values.
    #[serde(default = "default_placement", alias = "split")]
    pub placement: String,
    /// Focus the new agent pane / tab after start.
    #[serde(default)]
    pub focus_new: bool,
    /// Always wait this long after `pane run` before sending the prompt
    /// (gives the CLI time to paint its input). Milliseconds.
    #[serde(default = "default_startup_delay")]
    pub startup_delay_ms: u64,
    /// After the startup delay, wait up to this many ms for detection and
    /// `idle`. Failure aborts delivery; 0 explicitly skips the readiness check.
    #[serde(default = "default_wait_ready")]
    pub wait_ready_ms: u64,
}

/// Settings for `w`: create (or reopen) a git worktree for an issue, then
/// optionally start an agent in it (TOML table `[worktree]`).
#[derive(Debug, Clone, Deserialize)]
pub struct WorktreeConfig {
    /// Default repo for `w` when no per-project entry matches; empty = ask.
    #[serde(default)]
    pub repo: String,
    /// Per Jira project key repo dirs, e.g. PROJ = "~/workspace/project".
    #[serde(default)]
    pub repos: HashMap<String, String>,
    /// Prefill for the worktree name input. Placeholders {key} {slug} {type}. Default "{key}".
    #[serde(default = "default_worktree_branch")]
    pub branch: String,
    /// Base ref for NEW branches; empty = herdr default (HEAD of the repo).
    #[serde(default)]
    pub base: String,
    /// Checkout path template ({key} {slug} {type} {branch}); empty = herdr default
    /// (`<worktrees.directory>/<repo>/<branch-slug>`). `~`/$HOME expanded.
    #[serde(default)]
    pub path: String,
    /// Workspace label template ({key} {slug} {type} {branch}). Default "{key}".
    #[serde(default = "default_worktree_label")]
    pub label: String,
    /// Pass --trust-repository. Only enable for repositories you have verified.
    #[serde(default)]
    pub trust_repository: bool,
    /// Focus the worktree workspace after opening it. Default true.
    #[serde(default = "default_true")]
    pub focus: bool,
}

impl Default for WorktreeConfig {
    fn default() -> Self {
        Self {
            repo: String::new(),
            repos: HashMap::new(),
            branch: default_worktree_branch(),
            base: String::new(),
            path: String::new(),
            label: default_worktree_label(),
            trust_repository: false,
            focus: true,
        }
    }
}

impl Default for DelegateConfig {
    fn default() -> Self {
        Self {
            prompt: default_prompt(),
            submit: true,
            submit_delay_ms: default_submit_delay(),
            max_description_chars: default_max_desc(),
            agents: default_spawn_agents(),
            default_cwd: String::new(),
            placement: default_placement(),
            focus_new: false,
            startup_delay_ms: default_startup_delay(),
            wait_ready_ms: default_wait_ready(),
        }
    }
}

fn default_auth() -> String {
    "basic".into()
}
fn default_max_results() -> u32 {
    50
}
fn default_search_jql() -> String {
    r#"text ~ "{query}" ORDER BY updated DESC"#.into()
}
fn default_true() -> bool {
    true
}
fn default_submit_delay() -> u64 {
    500
}
fn default_max_desc() -> usize {
    6000
}
fn default_placement() -> String {
    "tab".into()
}
fn default_startup_delay() -> u64 {
    1500
}
fn default_wait_ready() -> u64 {
    30_000
}
fn default_worktree_branch() -> String {
    "{key}".into()
}
fn default_worktree_label() -> String {
    "{key}".into()
}
fn default_spawn_agents() -> Vec<SpawnAgent> {
    ["claude", "codex", "grok", "cursor", "opencode"]
        .into_iter()
        .map(|name| SpawnAgent {
            name: name.into(),
            command: vec![name.into()],
        })
        .collect()
}
fn default_prompt() -> String {
    "You are asked to work on Jira issue {key}: {summary}\n\n\
     Link: {url}\n\nDescription:\n{description}\n\n\
     Please analyze the issue, implement what it describes, and summarize \
     what you changed when you are done."
        .into()
}

pub fn config_path() -> PathBuf {
    if let Ok(dir) = std::env::var("HERDR_PLUGIN_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("config.toml");
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".config/herdr/plugins/config/han.jira-worktree/config.toml")
}

impl Config {
    pub fn load() -> Result<Self, String> {
        let path = config_path();
        let raw = std::fs::read_to_string(&path).map_err(|e| {
            format!(
                "cannot read config {}: {e}\n\ncopy config.example.toml there and fill in your Jira credentials",
                path.display()
            )
        })?;
        let mut cfg: Config =
            toml::from_str(&raw).map_err(|e| format!("invalid config {}: {e}", path.display()))?;
        cfg.jira.base_url = cfg.jira.base_url.trim_end_matches('/').to_string();
        if cfg.filters.is_empty() {
            cfg.filters.push(Filter {
                name: "My open issues".into(),
                jql: "assignee = currentUser() AND resolution = Unresolved ORDER BY updated DESC"
                    .into(),
            });
            if !cfg.jira.default_project.is_empty() {
                cfg.filters.push(Filter {
                    name: format!("Project {}", cfg.jira.default_project),
                    jql: "project = {project} ORDER BY updated DESC".into(),
                });
            }
        }
        Ok(cfg)
    }

    /// Expand {project} in a JQL template.
    pub fn expand_jql(&self, template: &str) -> String {
        template.replace("{project}", &self.jira.default_project)
    }

    /// Resolve the API token: inline value wins, else run `api_token_cmd`.
    pub fn resolve_token(&self) -> Result<String, String> {
        let inline = self.jira.api_token.trim();
        if !inline.is_empty() {
            return Ok(inline.to_string());
        }
        let cmd = self.jira.api_token_cmd.trim();
        if cmd.is_empty() {
            return Err("no api_token or api_token_cmd set in [jira] config".into());
        }
        let out = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .map_err(|e| format!("api_token_cmd failed to start: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "api_token_cmd exited with {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if token.is_empty() {
            return Err("api_token_cmd produced no output".into());
        }
        Ok(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_delegate_settings_are_not_nested_under_last_agent() {
        let raw = include_str!("../config.example.toml")
            .replace("placement = \"tab\"", "placement = \"right\"")
            .replace("focus_new = false", "focus_new = true")
            .replace("startup_delay_ms = 1500", "startup_delay_ms = 123")
            .replace("wait_ready_ms = 30000", "wait_ready_ms = 456")
            .replace("submit_delay_ms = 500", "submit_delay_ms = 789")
            .replace("# repo = \"~/workspace/project\"", "repo = \"~/src/app\"")
            .replace("trust_repository = false", "trust_repository = true");
        let cfg: Config = toml::from_str(&raw).unwrap();
        assert_eq!(cfg.delegate.placement, "right");
        assert!(cfg.delegate.focus_new);
        assert_eq!(cfg.delegate.startup_delay_ms, 123);
        assert_eq!(cfg.delegate.wait_ready_ms, 456);
        // Old configs remain parseable even though this setting is ignored.
        assert_eq!(cfg.delegate.submit_delay_ms, 789);
        assert_eq!(cfg.delegate.agents.len(), 3);
        // [worktree] after the agents is its own top-level table.
        assert_eq!(cfg.worktree.repo, "~/src/app");
        assert!(cfg.worktree.trust_repository);
        assert_eq!(cfg.worktree.branch, "{key}");
        assert!(cfg.worktree.focus);
    }

    const MINIMAL: &str = r#"
[jira]
base_url = "https://example.atlassian.net"

[delegate]
placement = "tab"
"#;

    #[test]
    fn missing_worktree_table_uses_defaults() {
        let cfg: Config = toml::from_str(MINIMAL).unwrap();
        let wt = &cfg.worktree;
        assert_eq!(wt.repo, "");
        assert!(wt.repos.is_empty());
        assert_eq!(wt.branch, "{key}");
        assert_eq!(wt.label, "{key}");
        assert_eq!(wt.base, "");
        assert_eq!(wt.path, "");
        assert!(!wt.trust_repository);
        assert!(wt.focus);
    }

    #[test]
    fn partial_worktree_table_keeps_other_defaults() {
        let raw = format!(
            "{MINIMAL}\n[worktree]\nrepo = \"~/src/app\"\nbranch = \"feature/{{key}}-{{slug}}\"\n\n[worktree.repos]\nPROJ = \"~/w/backend\"\n"
        );
        let cfg: Config = toml::from_str(&raw).unwrap();
        let wt = &cfg.worktree;
        assert_eq!(wt.repo, "~/src/app");
        assert_eq!(wt.branch, "feature/{key}-{slug}");
        assert_eq!(wt.repos.len(), 1);
        assert_eq!(
            wt.repos.get("PROJ").map(String::as_str),
            Some("~/w/backend")
        );
        assert!(wt.focus);
        assert!(!wt.trust_repository);
        assert_eq!(wt.label, "{key}");
        assert_eq!(wt.base, "");
        assert_eq!(wt.path, "");
        assert_eq!(cfg.delegate.placement, "tab");
    }

    #[test]
    fn legacy_delegate_worktree_table_is_ignored() {
        let raw = format!(
            "{MINIMAL}\n[delegate.worktree]\nenabled = true\npreselect = true\nbranch = \"legacy/{{key}}\"\n"
        );
        let cfg: Config = toml::from_str(&raw).unwrap();
        assert_eq!(cfg.delegate.placement, "tab");
        assert_eq!(cfg.worktree.branch, "{key}");
    }
}

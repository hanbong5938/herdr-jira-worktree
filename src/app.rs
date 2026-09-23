//! Application state and key handling. Network and herdr-CLI work runs on
//! background threads; results come back over an mpsc channel as `Resp`.

use crate::config::{Config, SpawnAgent};
use crate::herdr::{self, HerdrAgent, HerdrWorkspace, StartAgentOpts};
use crate::jira::{Comment, Issue, JiraClient, Transition};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    List,
    Detail,
    FilterPicker,
    TransitionPicker,
    AgentPicker,
    /// Pick which agent binary to spawn (from `[delegate].agents`).
    NewAgentTypePicker,
    /// Pick which herdr workspace ("space") to place the new agent in.
    NewAgentWorkspacePicker,
    /// Pick a working directory for the new agent.
    NewAgentCwdPicker,
    /// Free-text cwd entry (from "type path…" in the cwd picker).
    NewAgentCwdInput,
    /// `w`: pick the herdr project (source repo) for the worktree (no `[worktree]` repo configured).
    WorktreeProjectPicker,
    /// `w`: free-text repo path (from "type path…" in the project picker).
    WorktreeRepoInput,
    /// `w`: the repo's worktrees; row 0 = "+ new worktree".
    WorktreeListPicker,
    /// `w`: new worktree (branch) name, prefilled from `[worktree].branch`.
    WorktreeNameInput,
    /// `w`: optionally start an agent in the worktree.
    WorktreeAgentPicker,
    SearchInput,
    JqlInput,
    Help,
}

pub enum Resp {
    Issues {
        title: String,
        result: Result<Vec<Issue>, String>,
    },
    Transitions {
        key: String,
        result: Result<Vec<Transition>, String>,
    },
    Transitioned {
        key: String,
        name: String,
        result: Result<(), String>,
    },
    Agents(Result<Vec<HerdrAgent>, String>),
    Workspaces(Result<Vec<HerdrWorkspace>, String>),
    Delegated {
        key: String,
        label: String,
        /// Enter was sent; false = the prompt was only pasted for review.
        submitted: bool,
        result: Result<(), String>,
    },
    /// `w` without an agent: `result` is the checkout path.
    WorktreeOpened {
        branch: String,
        result: Result<String, String>,
    },
    /// `w`: herdr projects for the project picker.
    Projects(Result<Vec<herdr::RepoProject>, String>),
    /// `w`: checkouts of the repo at `repo`.
    RepoWorktrees {
        repo: String,
        result: Result<Vec<herdr::RepoWorktree>, String>,
    },
    Children {
        epic: String,
        result: Result<Vec<Issue>, String>,
    },
    Comments {
        key: String,
        result: Result<Vec<Comment>, String>,
    },
}

pub fn is_epic(issue: &Issue) -> bool {
    issue.issue_type.eq_ignore_ascii_case("epic")
}

pub struct App {
    pub cfg: Config,
    pub client: Option<Arc<JiraClient>>,
    pub tx: Sender<Resp>,

    pub view: View,
    pub should_quit: bool,

    pub issues: Vec<Issue>,
    /// Epic expansion state: children keyed by epic key, plus which epics are
    /// currently open and which are still fetching.
    pub children: HashMap<String, Vec<Issue>>,
    pub expanded: HashSet<String>,
    pub loading_children: HashSet<String>,
    pub selected: usize,
    pub current_title: String,
    pub loading: bool,
    pub fatal: Option<String>, // config/auth error shown full-screen

    pub filter_idx: usize,
    pub picker_sel: usize, // shared selection index for popup pickers

    pub transitions: Vec<Transition>,
    pub transitions_for: String,
    pub transitions_loading: bool,

    pub agents: Vec<HerdrAgent>,
    pub agents_loading: bool,

    /// Selected spawn agent while walking the "start new" wizard.
    pub pending_spawn: Option<SpawnAgent>,
    /// Selected workspace for the new agent.
    pub pending_workspace: Option<HerdrWorkspace>,
    pub workspaces: Vec<HerdrWorkspace>,
    pub workspaces_loading: bool,
    /// Unique cwd candidates for the new-agent cwd picker.
    pub cwd_choices: Vec<String>,
    pub cwd_input: String,

    /// `w` flow: validated repo dir the worktree is created from.
    pub wt_repo: String,
    /// Display name of `wt_repo` (project name or directory name).
    pub wt_repo_name: String,
    /// The project picker was shown (Esc on the worktree list returns there).
    pub wt_project_asked: bool,
    /// herdr projects for the project picker, current one first.
    pub wt_projects: Vec<herdr::RepoProject>,
    pub wt_projects_loading: bool,
    /// Checkouts of `wt_repo` for the worktree list.
    pub wt_worktrees: Vec<herdr::RepoWorktree>,
    pub wt_worktrees_loading: bool,
    /// Branch rendered from `[worktree].branch` for the selected issue.
    pub wt_default_branch: String,
    /// The agent picker was reached from an existing worktree (not the name input).
    pub wt_existing: bool,
    /// Worktree name being edited.
    pub wt_name_input: String,
    /// Branch the agent picker opens: sanitized new name or an existing worktree's branch.
    pub wt_branch: String,

    pub search_input: String,
    pub jql_input: String,
    pub last_jql: String,
    pub detail_scroll: u16,

    /// Comments per issue key (newest first), in-flight fetches and failures.
    pub comments: HashMap<String, Vec<Comment>>,
    pub comments_loading: HashSet<String>,
    pub comments_err: HashMap<String, String>,
    pub comment_scroll: u16,
    /// Detail view: j/k scroll the comments pane instead of the description.
    pub detail_focus_comments: bool,

    pub toast: Option<(String, bool, Instant)>, // message, is_error, shown_at
}

impl App {
    pub fn new(tx: Sender<Resp>) -> Self {
        let mut app = Self::unloaded(tx);
        app.reload_config();
        app
    }

    /// Empty state with the built-in default config; `new` then loads the real one.
    fn unloaded(tx: Sender<Resp>) -> Self {
        Self {
            cfg: Config {
                jira: crate::config::JiraConfig {
                    base_url: String::new(),
                    auth: "basic".into(),
                    email: String::new(),
                    api_token: String::new(),
                    api_token_cmd: String::new(),
                    default_project: String::new(),
                    max_results: 50,
                },
                filters: vec![],
                search: Default::default(),
                delegate: Default::default(),
                worktree: Default::default(),
            },
            client: None,
            tx,
            view: View::List,
            should_quit: false,
            issues: vec![],
            children: HashMap::new(),
            expanded: HashSet::new(),
            loading_children: HashSet::new(),
            selected: 0,
            current_title: String::new(),
            loading: false,
            fatal: None,
            filter_idx: 0,
            picker_sel: 0,
            transitions: vec![],
            transitions_for: String::new(),
            transitions_loading: false,
            agents: vec![],
            agents_loading: false,
            pending_spawn: None,
            pending_workspace: None,
            workspaces: vec![],
            workspaces_loading: false,
            cwd_choices: vec![],
            cwd_input: String::new(),
            wt_repo: String::new(),
            wt_repo_name: String::new(),
            wt_project_asked: false,
            wt_projects: vec![],
            wt_projects_loading: false,
            wt_worktrees: vec![],
            wt_worktrees_loading: false,
            wt_default_branch: String::new(),
            wt_existing: false,
            wt_name_input: String::new(),
            wt_branch: String::new(),
            search_input: String::new(),
            jql_input: String::new(),
            last_jql: String::new(),
            detail_scroll: 0,
            comments: HashMap::new(),
            comments_loading: HashSet::new(),
            comments_err: HashMap::new(),
            comment_scroll: 0,
            detail_focus_comments: false,
            toast: None,
        }
    }

    pub fn reload_config(&mut self) {
        match Config::load() {
            Ok(cfg) => match JiraClient::new(&cfg) {
                Ok(client) => {
                    self.cfg = cfg;
                    self.client = Some(Arc::new(client));
                    self.fatal = None;
                    self.filter_idx = self
                        .filter_idx
                        .min(self.cfg.filters.len().saturating_sub(1));
                    self.load_filter(self.filter_idx);
                }
                Err(e) => {
                    self.cfg = cfg;
                    self.fatal = Some(e);
                }
            },
            Err(e) => self.fatal = Some(e),
        }
    }

    /// Rows currently on screen: top-level issues, with the children of every
    /// expanded epic inlined right below it. `u8` is the indent depth.
    pub fn visible(&self) -> Vec<(&Issue, u8)> {
        let mut rows = Vec::with_capacity(self.issues.len());
        for issue in &self.issues {
            rows.push((issue, 0));
            if is_epic(issue) && self.expanded.contains(&issue.key) {
                if let Some(kids) = self.children.get(&issue.key) {
                    rows.extend(kids.iter().map(|k| (k, 1)));
                }
            }
        }
        rows
    }

    pub fn selected_issue(&self) -> Option<&Issue> {
        self.visible().get(self.selected).map(|(i, _)| *i)
    }

    fn toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toast = Some((msg.into(), is_error, Instant::now()));
    }

    // ---- background requests ----

    fn spawn_search(&mut self, jql: String, title: String) {
        let Some(client) = self.client.clone() else {
            return;
        };
        self.last_jql = jql.clone();
        self.loading = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = client.search(&jql);
            let _ = tx.send(Resp::Issues { title, result });
        });
    }

    pub fn load_filter(&mut self, idx: usize) {
        let Some(filter) = self.cfg.filters.get(idx).cloned() else {
            return;
        };
        self.filter_idx = idx;
        let jql = self.cfg.expand_jql(&filter.jql);
        self.spawn_search(jql, filter.name);
    }

    fn run_search(&mut self, query: String) {
        let jql = self
            .cfg
            .expand_jql(&self.cfg.search.jql.clone())
            .replace("{query}", &query.replace('"', "\\\""));
        self.spawn_search(jql, format!("search: {query}"));
    }

    fn request_transitions(&mut self) {
        let Some(issue) = self.selected_issue() else {
            return;
        };
        let Some(client) = self.client.clone() else {
            return;
        };
        let key = issue.key.clone();
        self.transitions_for = key.clone();
        self.transitions.clear();
        self.transitions_loading = true;
        self.picker_sel = 0;
        self.view = View::TransitionPicker;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = client.transitions(&key);
            let _ = tx.send(Resp::Transitions { key, result });
        });
    }

    fn apply_transition(&mut self, t: Transition) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let key = self.transitions_for.clone();
        let tx = self.tx.clone();
        self.toast(format!("{key}: applying \"{}\"…", t.name), false);
        std::thread::spawn(move || {
            let result = client.apply_transition(&key, &t.id);
            let _ = tx.send(Resp::Transitioned {
                key,
                name: t.to_status,
                result,
            });
        });
    }

    /// Fetch comments for `key` unless cached, failed, or already in flight.
    fn fetch_comments(&mut self, key: String) {
        if self.comments.contains_key(&key)
            || self.comments_err.contains_key(&key)
            || self.comments_loading.contains(&key)
        {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        self.comments_loading.insert(key.clone());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = client.comments(&key);
            let _ = tx.send(Resp::Comments { key, result });
        });
    }

    /// Expand the selected epic (fetching its children on first open).
    fn expand_epic(&mut self) {
        let Some(issue) = self.selected_issue() else {
            return;
        };
        if !is_epic(issue) {
            return;
        }
        let key = issue.key.clone();
        if self.expanded.contains(&key) {
            return;
        }
        if self.children.contains_key(&key) {
            self.expanded.insert(key);
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        if !self.loading_children.insert(key.clone()) {
            return; // fetch already in flight
        }
        self.toast(format!("loading issues in {key}…"), false);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            // Jira Cloud links children via `parent`; classic Server/DC epics
            // use the "Epic Link" field. Try both.
            let result = client
                .search(&format!("parent = {key} ORDER BY created ASC"))
                .or_else(|_| client.search(&format!("\"Epic Link\" = {key} ORDER BY created ASC")));
            let _ = tx.send(Resp::Children { epic: key, result });
        });
    }

    /// Collapse the selected epic — or, on a child row, collapse its parent
    /// epic and move the selection onto it.
    fn collapse_epic(&mut self) {
        let vis = self.visible();
        let Some(&(issue, depth)) = vis.get(self.selected) else {
            return;
        };
        let epic_key = if depth == 0 {
            if !(is_epic(issue) && self.expanded.contains(&issue.key)) {
                return;
            }
            issue.key.clone()
        } else {
            // Walk back to the nearest top-level row: that's the parent epic.
            match vis[..self.selected]
                .iter()
                .rev()
                .find(|(_, d)| *d == 0)
                .map(|(i, _)| i.key.clone())
            {
                Some(k) => k,
                None => return,
            }
        };
        self.expanded.remove(&epic_key);
        // Land the selection on the epic row itself.
        if let Some(idx) = self.visible().iter().position(|(i, _)| i.key == epic_key) {
            self.selected = idx;
        }
    }

    fn request_agents(&mut self) {
        if self.selected_issue().is_none() {
            return;
        }
        self.agents.clear();
        self.agents_loading = true;
        self.picker_sel = 0;
        self.pending_spawn = None;
        self.view = View::AgentPicker;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Resp::Agents(herdr::list_agents()));
        });
    }

    /// Rows in the agent picker: optional "start new" at index 0, then running agents.
    pub fn can_start_new_agent(&self) -> bool {
        !self.cfg.delegate.agents.is_empty()
    }

    /// Index of the first running-agent row in AgentPicker (0 if no "start new").
    pub fn agent_list_offset(&self) -> usize {
        if self.can_start_new_agent() {
            1
        } else {
            0
        }
    }

    pub fn agent_picker_len(&self) -> usize {
        self.agent_list_offset() + self.agents.len()
    }

    fn open_new_agent_type_picker(&mut self) {
        if self.cfg.delegate.agents.is_empty() {
            self.toast(
                "no [[delegate.agents]] configured — add agents in config.toml",
                true,
            );
            return;
        }
        self.picker_sel = 0;
        self.pending_spawn = None;
        self.pending_workspace = None;
        self.view = View::NewAgentTypePicker;
    }

    /// After agent type: load workspaces and open the space picker.
    fn open_new_agent_workspace_picker(&mut self, spawn: SpawnAgent) {
        self.pending_spawn = Some(spawn);
        self.pending_workspace = None;
        self.workspaces.clear();
        self.workspaces_loading = true;
        self.picker_sel = 0;
        self.view = View::NewAgentWorkspacePicker;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Resp::Workspaces(herdr::list_workspaces()));
        });
    }

    fn open_new_agent_cwd_picker(&mut self) {
        self.cwd_choices = collect_cwd_choices(&self.cfg, &self.agents);
        self.picker_sel = 0;
        self.view = View::NewAgentCwdPicker;
    }

    fn open_cwd_input(&mut self) {
        let pref = if !self.cfg.delegate.default_cwd.trim().is_empty() {
            self.cfg.delegate.default_cwd.clone()
        } else if let Some(first) = self.cwd_choices.first() {
            first.clone()
        } else {
            std::env::var("HOME").unwrap_or_default()
        };
        self.cwd_input = pref;
        self.view = View::NewAgentCwdInput;
    }

    fn delegate_to(&mut self, agent: HerdrAgent) {
        let Some(issue) = self.selected_issue() else {
            return;
        };
        let text = build_prompt(&self.cfg, issue);
        let key = issue.key.clone();
        let submit = self.cfg.delegate.submit;
        let delay = self.cfg.delegate.submit_delay_ms;
        let tx = self.tx.clone();
        self.toast(format!("sending {key} to {}…", agent.label), false);
        std::thread::spawn(move || {
            let result = herdr::send_to_agent(&agent, &text, submit, delay);
            let _ = tx.send(Resp::Delegated {
                key,
                label: agent.label,
                submitted: submit,
                result,
            });
        });
    }

    /// Spawn a fresh agent in the chosen workspace + cwd, wait until ready,
    /// send the Jira prompt.
    fn start_new_and_delegate(&mut self, cwd_raw: String) {
        let Some(issue) = self.selected_issue() else {
            return;
        };
        let Some(spawn) = self.pending_spawn.clone() else {
            self.toast("no agent selected", true);
            return;
        };
        let Some(ws) = self.pending_workspace.clone() else {
            self.toast("no workspace selected", true);
            return;
        };
        let cwd = herdr::expand_path(&cwd_raw);
        if cwd.is_empty() {
            self.toast("cwd is empty", true);
            return;
        }
        if !Path::new(&cwd).is_dir() {
            self.toast(format!("not a directory: {cwd}"), true);
            return;
        }
        if spawn.command.is_empty() {
            self.toast(format!("{}: empty command", spawn.name), true);
            return;
        }

        let text = build_prompt(&self.cfg, issue);
        let key = issue.key.clone();
        let submit = self.cfg.delegate.submit;
        let delay = self.cfg.delegate.submit_delay_ms;
        let startup = self.cfg.delegate.startup_delay_ms;
        let wait_ready = self.cfg.delegate.wait_ready_ms;
        let placement = self.cfg.delegate.placement.clone();
        let focus = self.cfg.delegate.focus_new;
        let name = unique_agent_name(&key, &spawn.name);
        let agent_label = spawn.name.clone();
        let ws_label = ws.label.clone();
        let opts = StartAgentOpts {
            name,
            cwd: cwd.clone(),
            argv: spawn.command.clone(),
            placement: placement.clone(),
            focus,
            workspace_id: ws.id.clone(),
            tab_label: key.clone(),
            worktree: None,
        };
        let place_hint = if placement.eq_ignore_ascii_case("tab") || placement.is_empty() {
            format!("new tab in {ws_label}")
        } else {
            format!("split {placement} in {ws_label}")
        };
        let tx = self.tx.clone();
        self.toast(
            format!(
                "starting {agent_label} for {key} ({place_hint}, {})…",
                short_home(&cwd)
            ),
            false,
        );
        std::thread::spawn(move || {
            let result =
                herdr::start_and_delegate(&opts, &text, submit, delay, startup, wait_ready)
                    .map(|a| a.label);
            let label = match &result {
                Ok(l) => l.clone(),
                Err(_) => agent_label,
            };
            let result = result.map(|_| ());
            let _ = tx.send(Resp::Delegated {
                key,
                label,
                submitted: submit,
                result,
            });
        });
    }

    // ---- `w`: git worktree for the selected issue ----

    /// Resolve the repo from `[worktree]` (per project key, then the default)
    /// or let the user pick a herdr project; then show the repo's worktrees.
    fn open_worktree(&mut self) {
        let Some(issue) = self.selected_issue() else {
            return;
        };
        let wcfg = &self.cfg.worktree;
        let default_branch = render_branch_name(&wcfg.branch, issue);
        let configured = wcfg
            .repos
            .get(project_key(&issue.key))
            .filter(|r| !r.trim().is_empty())
            .or(Some(&wcfg.repo).filter(|r| !r.trim().is_empty()))
            .cloned();
        self.wt_default_branch = default_branch;
        self.wt_existing = false;
        let Some(raw) = configured else {
            self.wt_project_asked = true;
            self.open_worktree_project_picker();
            return;
        };
        self.wt_project_asked = false;
        match resolve_cwd(&raw) {
            Ok(repo) => {
                let name = dir_name(&repo);
                self.open_worktree_list(repo, name);
            }
            Err(e) => self.toast(format!("worktree repo: {e}"), true),
        }
    }

    /// Step A: fetch herdr's projects (source repos); `Resp::Projects` fills the picker.
    fn open_worktree_project_picker(&mut self) {
        self.wt_projects.clear();
        self.wt_projects_loading = true;
        self.picker_sel = 0;
        self.view = View::WorktreeProjectPicker;
        let tx = self.tx.clone();
        spawn_herdr(move || {
            let _ = tx.send(Resp::Projects(herdr::list_projects()));
        });
    }

    /// Back to the project picker with the chosen project (or "type path…") selected.
    fn return_to_project_picker(&mut self) {
        self.picker_sel = self
            .wt_projects
            .iter()
            .position(|p| herdr::expand_path(&p.root) == self.wt_repo)
            .unwrap_or(self.wt_projects.len());
        self.view = View::WorktreeProjectPicker;
    }

    /// Step B: fetch the worktrees of `repo`; `Resp::RepoWorktrees` fills the list.
    fn open_worktree_list(&mut self, repo: String, name: String) {
        self.wt_repo = repo.clone();
        self.wt_repo_name = name;
        self.wt_worktrees.clear();
        self.wt_worktrees_loading = true;
        self.picker_sel = 0;
        self.view = View::WorktreeListPicker;
        let tx = self.tx.clone();
        spawn_herdr(move || {
            let result = herdr::list_worktrees(&repo);
            let _ = tx.send(Resp::RepoWorktrees { repo, result });
        });
    }

    /// Back to the worktree list with `branch`'s row (or "+ new worktree") selected.
    fn return_to_worktree_list(&mut self, branch: Option<&str>) {
        self.picker_sel = branch
            .and_then(|b| self.wt_worktrees.iter().position(|w| w.branch == b))
            .map_or(0, |i| i + 1);
        self.view = View::WorktreeListPicker;
    }

    /// "+ new worktree": the name input, prefilled from `[worktree].branch`.
    fn open_worktree_name_input(&mut self) {
        self.wt_name_input = self.wt_default_branch.clone();
        self.view = View::WorktreeNameInput;
    }

    fn open_worktree_repo_input(&mut self) {
        let pref = self
            .wt_projects
            .get(self.picker_sel)
            .or(self.wt_projects.first())
            .map(|p| p.root.clone())
            .or_else(|| std::env::var("HOME").ok())
            .unwrap_or_default();
        self.cwd_input = pref;
        self.view = View::WorktreeRepoInput;
    }

    /// Any step of the `w` flow is on screen.
    fn in_worktree_flow(&self) -> bool {
        matches!(
            self.view,
            View::WorktreeProjectPicker
                | View::WorktreeRepoInput
                | View::WorktreeListPicker
                | View::WorktreeNameInput
                | View::WorktreeAgentPicker
        )
    }

    fn on_projects(&mut self, result: Result<Vec<herdr::RepoProject>, String>) {
        // Stale: `w` was left, or this answers an older request.
        if !self.wt_projects_loading || !self.in_worktree_flow() {
            return;
        }
        self.wt_projects_loading = false;
        match result {
            Ok(projects) => {
                self.wt_projects = projects;
                if self.view == View::WorktreeProjectPicker {
                    self.picker_sel = self.wt_projects.iter().position(|p| p.current).unwrap_or(0);
                }
            }
            // "type path…" stays available.
            Err(e) => self.toast(format!("herdr projects: {e}"), true),
        }
    }

    fn on_repo_worktrees(
        &mut self,
        repo: String,
        result: Result<Vec<herdr::RepoWorktree>, String>,
    ) {
        // Stale: another repo was picked, `w` was left, or already answered.
        if !self.wt_worktrees_loading || repo != self.wt_repo || !self.in_worktree_flow() {
            return;
        }
        self.wt_worktrees_loading = false;
        match result {
            Ok(worktrees) => {
                self.wt_worktrees = worktrees;
                // The issue's branch already has a worktree: preselect reopening it.
                if self.view == View::WorktreeListPicker {
                    let branch = self.wt_default_branch.clone();
                    self.return_to_worktree_list(Some(branch.as_str()));
                }
            }
            // "+ new worktree" stays available.
            Err(e) => self.toast(format!("worktrees: {e}"), true),
        }
    }

    /// Open (or create) the worktree without an agent; reported as `WorktreeOpened`.
    fn open_worktree_only(&mut self, opts: herdr::WorktreeOpts) {
        let repo = self.wt_repo.clone();
        let focus = self.cfg.worktree.focus;
        let branch = opts.branch.clone();
        let tx = self.tx.clone();
        let verb = if self.wt_existing {
            "opening"
        } else {
            "creating"
        };
        self.toast(format!("{verb} worktree {branch}…"), false);
        std::thread::spawn(move || {
            let result =
                herdr::open_or_create_worktree(&repo, &opts, focus).map(|wt| wt.checkout_path);
            let _ = tx.send(Resp::WorktreeOpened { branch, result });
        });
    }

    /// Start `spawn` inside the worktree, wait until ready, send the Jira prompt.
    fn start_worktree_agent(&mut self, spawn: SpawnAgent, wt: herdr::WorktreeOpts) {
        if spawn.command.is_empty() {
            self.toast(format!("{}: empty command", spawn.name), true);
            return;
        }
        let Some(issue) = self.selected_issue() else {
            return;
        };
        let text = build_prompt(&self.cfg, issue);
        let key = issue.key.clone();
        let submit = self.cfg.worktree.submit.unwrap_or(self.cfg.delegate.submit);
        let delay = self.cfg.delegate.submit_delay_ms;
        let startup = self.cfg.delegate.startup_delay_ms;
        let wait_ready = self.cfg.delegate.wait_ready_ms;
        let agent_label = spawn.name.clone();
        let hint = format!("worktree {} of {}", wt.branch, short_home(&self.wt_repo));
        let opts = StartAgentOpts {
            name: unique_agent_name(&key, &spawn.name),
            cwd: self.wt_repo.clone(),
            argv: spawn.command,
            placement: self.cfg.delegate.placement.clone(),
            focus: self.cfg.worktree.focus,
            workspace_id: String::new(),
            tab_label: key.clone(),
            worktree: Some(wt),
        };
        let tx = self.tx.clone();
        self.toast(format!("starting {agent_label} for {key} ({hint})…"), false);
        std::thread::spawn(move || {
            let result =
                herdr::start_and_delegate(&opts, &text, submit, delay, startup, wait_ready)
                    .map(|a| a.label);
            let label = match &result {
                Ok(l) => l.clone(),
                Err(_) => agent_label,
            };
            let result = result.map(|_| ());
            let _ = tx.send(Resp::Delegated {
                key,
                label,
                submitted: submit,
                result,
            });
        });
    }

    fn zoom_toggle(&mut self) {
        if let Err(e) = herdr::zoom_toggle() {
            self.toast(format!("zoom: {e}"), true);
        }
    }

    fn open_in_browser(&mut self) {
        let Some(issue) = self.selected_issue() else {
            return;
        };
        let url = issue.url.clone();
        #[cfg(target_os = "macos")]
        let opener = "open";
        #[cfg(not(target_os = "macos"))]
        let opener = "xdg-open";
        let _ = std::process::Command::new(opener).arg(&url).spawn();
        self.toast(format!("opened {}", url), false);
    }

    // ---- responses ----

    pub fn on_resp(&mut self, resp: Resp) {
        match resp {
            Resp::Issues { title, result } => {
                self.loading = false;
                match result {
                    Ok(issues) => {
                        self.current_title = title;
                        self.issues = issues;
                        self.children.clear();
                        self.expanded.clear();
                        self.loading_children.clear();
                        self.selected = self.selected.min(self.issues.len().saturating_sub(1));
                    }
                    Err(e) => self.toast(format!("Jira: {e}"), true),
                }
            }
            Resp::Transitions { key, result } => {
                if key != self.transitions_for {
                    return;
                }
                self.transitions_loading = false;
                match result {
                    Ok(ts) => self.transitions = ts,
                    Err(e) => {
                        self.view = View::List;
                        self.toast(format!("{key}: transitions: {e}"), true);
                    }
                }
            }
            Resp::Transitioned { key, name, result } => match result {
                Ok(()) => {
                    self.comments.remove(&key);
                    self.comments_err.remove(&key);
                    self.toast(format!("{key} → {name}"), false);
                    self.load_filter(self.filter_idx);
                }
                Err(e) => self.toast(format!("{key}: {e}"), true),
            },
            Resp::Agents(result) => {
                self.agents_loading = false;
                match result {
                    Ok(agents) => {
                        self.agents = agents;
                        if self.agents.is_empty() && !self.can_start_new_agent() {
                            self.view = View::List;
                            self.toast(
                                "no running agents and no [[delegate.agents]] to start",
                                true,
                            );
                        }
                    }
                    Err(e) => {
                        // Still allow starting a new agent if list failed.
                        if self.can_start_new_agent() {
                            self.agents.clear();
                            self.toast(
                                format!("list agents failed ({e}); start new is available"),
                                true,
                            );
                        } else {
                            self.view = View::List;
                            self.toast(format!("agents: {e}"), true);
                        }
                    }
                }
            }
            Resp::Workspaces(result) => {
                self.workspaces_loading = false;
                if self.view != View::NewAgentWorkspacePicker {
                    return;
                }
                match result {
                    Ok(list) if list.is_empty() => {
                        self.view = View::NewAgentTypePicker;
                        self.toast("no workspaces found in herdr", true);
                    }
                    Ok(list) => {
                        // Prefer the workspace that hosts this Jira pane.
                        let current = std::env::var("HERDR_WORKSPACE_ID").unwrap_or_default();
                        self.picker_sel = list
                            .iter()
                            .position(|w| !current.is_empty() && w.id == current)
                            .or_else(|| list.iter().position(|w| w.focused))
                            .unwrap_or(0);
                        self.workspaces = list;
                    }
                    Err(e) => {
                        self.view = View::NewAgentTypePicker;
                        self.toast(format!("workspaces: {e}"), true);
                    }
                }
            }
            Resp::Delegated {
                key,
                label,
                submitted,
                result,
            } => match result {
                Ok(()) if submitted => {
                    self.toast(format!("{key} delegated to {label}"), false);
                    herdr::notify(&format!("Jira {key} delegated to {label}"));
                }
                Ok(()) => {
                    self.toast(
                        format!("{key} pasted into {label} — review and press Enter"),
                        false,
                    );
                    herdr::notify(&format!("Jira {key} pasted into {label}"));
                }
                Err(e) => self.toast(format!("delegate {key}: {e}"), true),
            },
            Resp::WorktreeOpened { branch, result } => match result {
                Ok(path) => self.toast(
                    format!("worktree {branch} ready: {}", short_home(&path)),
                    false,
                ),
                Err(e) => self.toast(format!("worktree {branch}: {e}"), true),
            },
            Resp::Projects(result) => self.on_projects(result),
            Resp::RepoWorktrees { repo, result } => self.on_repo_worktrees(repo, result),
            Resp::Children { epic, result } => {
                self.loading_children.remove(&epic);
                match result {
                    Ok(kids) if kids.is_empty() => {
                        self.toast(format!("{epic}: no issues in this epic"), false)
                    }
                    Ok(kids) => {
                        self.children.insert(epic.clone(), kids);
                        self.expanded.insert(epic);
                    }
                    Err(e) => self.toast(format!("{epic}: children: {e}"), true),
                }
            }
            Resp::Comments { key, result } => {
                self.comments_loading.remove(&key);
                match result {
                    Ok(list) => {
                        self.comments_err.remove(&key);
                        self.comments.insert(key, list);
                    }
                    Err(e) => {
                        self.comments_err.insert(key, e);
                    }
                }
            }
        }
    }

    // ---- keys ----

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }
        if self.fatal.is_some() {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
                KeyCode::Char('R') | KeyCode::Char('r') => self.reload_config(),
                _ => {}
            }
            return;
        }
        match self.view {
            View::List => self.keys_list(key),
            View::Detail => self.keys_detail(key),
            View::FilterPicker => self.keys_filter_picker(key),
            View::TransitionPicker => self.keys_transition_picker(key),
            View::AgentPicker => self.keys_agent_picker(key),
            View::NewAgentTypePicker => self.keys_new_agent_type(key),
            View::NewAgentWorkspacePicker => self.keys_new_agent_workspace(key),
            View::NewAgentCwdPicker => self.keys_new_agent_cwd(key),
            View::NewAgentCwdInput => self.keys_cwd_input(key),
            View::WorktreeProjectPicker => self.keys_worktree_project(key),
            View::WorktreeRepoInput => self.keys_worktree_repo_input(key),
            View::WorktreeListPicker => self.keys_worktree_list(key),
            View::WorktreeNameInput => self.keys_worktree_name(key),
            View::WorktreeAgentPicker => self.keys_worktree_agent(key),
            View::SearchInput => self.keys_search(key),
            View::JqlInput => self.keys_jql(key),
            View::Help => match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => self.view = View::List,
                _ => {}
            },
        }
    }

    fn move_sel(len: usize, sel: usize, delta: i32) -> usize {
        if len == 0 {
            return 0;
        }
        let max = len as i32 - 1;
        (sel as i32 + delta).clamp(0, max) as usize
    }

    fn keys_list(&mut self, key: KeyEvent) {
        let vis_len = self.visible().len();
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('j') | KeyCode::Down => {
                self.selected = Self::move_sel(vis_len, self.selected, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected = Self::move_sel(vis_len, self.selected, -1)
            }
            KeyCode::PageDown => self.selected = Self::move_sel(vis_len, self.selected, 15),
            KeyCode::PageUp => self.selected = Self::move_sel(vis_len, self.selected, -15),
            KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
            KeyCode::Char('G') | KeyCode::End => self.selected = vis_len.saturating_sub(1),
            KeyCode::Char('l') | KeyCode::Right => self.expand_epic(),
            KeyCode::Char('h') | KeyCode::Left => self.collapse_epic(),
            KeyCode::Enter => {
                if let Some(key) = self.selected_issue().map(|i| i.key.clone()) {
                    self.detail_scroll = 0;
                    self.detail_focus_comments = false;
                    self.view = View::Detail;
                    self.comment_scroll = 0;
                    self.fetch_comments(key);
                }
            }
            KeyCode::Char('f') => {
                self.picker_sel = self.filter_idx;
                self.view = View::FilterPicker;
            }
            KeyCode::Char(c @ '1'..='9') => {
                let idx = (c as u8 - b'1') as usize;
                if idx < self.cfg.filters.len() {
                    self.load_filter(idx);
                }
            }
            KeyCode::Char('/') => {
                self.search_input.clear();
                self.view = View::SearchInput;
            }
            KeyCode::Char('J') => {
                self.jql_input = self.last_jql.clone();
                self.view = View::JqlInput;
            }
            KeyCode::Char('r') => {
                self.comments.clear();
                self.comments_err.clear();
                self.load_filter(self.filter_idx);
            }
            KeyCode::Char('R') => {
                self.reload_config();
                self.toast("config reloaded", false);
            }
            KeyCode::Char('s') => self.request_transitions(),
            KeyCode::Char('d') => self.request_agents(),
            KeyCode::Char('w') => self.open_worktree(),
            KeyCode::Char('o') => self.open_in_browser(),
            KeyCode::Char('z') => self.zoom_toggle(),
            KeyCode::Char('?') => self.view = View::Help,
            _ => {}
        }
    }

    fn keys_detail(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.view = View::List,
            KeyCode::Tab => self.detail_focus_comments = !self.detail_focus_comments,
            KeyCode::Char('j') | KeyCode::Down => {
                let s = self.focused_scroll();
                *s = s.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let s = self.focused_scroll();
                *s = s.saturating_sub(1)
            }
            KeyCode::PageDown => {
                let s = self.focused_scroll();
                *s = s.saturating_add(15)
            }
            KeyCode::PageUp => {
                let s = self.focused_scroll();
                *s = s.saturating_sub(15)
            }
            KeyCode::Char('g') => *self.focused_scroll() = 0,
            KeyCode::Char('s') => self.request_transitions(),
            KeyCode::Char('d') => self.request_agents(),
            KeyCode::Char('w') => self.open_worktree(),
            KeyCode::Char('o') => self.open_in_browser(),
            KeyCode::Char('z') => self.zoom_toggle(),
            _ => {}
        }
    }

    /// Detail view: scroll offset of the pane that has focus.
    fn focused_scroll(&mut self) -> &mut u16 {
        if self.detail_focus_comments {
            &mut self.comment_scroll
        } else {
            &mut self.detail_scroll
        }
    }

    /// Number hotkeys shared by all popup pickers: `1`-`9` picks that row.
    fn picker_number(key: &KeyEvent, len: usize) -> Option<usize> {
        if let KeyCode::Char(c @ '1'..='9') = key.code {
            let idx = (c as u8 - b'1') as usize;
            if idx < len {
                return Some(idx);
            }
        }
        None
    }

    fn keys_filter_picker(&mut self, key: KeyEvent) {
        if let Some(idx) = Self::picker_number(&key, self.cfg.filters.len()) {
            self.view = View::List;
            self.load_filter(idx);
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.view = View::List,
            KeyCode::Char('j') | KeyCode::Down => {
                self.picker_sel = Self::move_sel(self.cfg.filters.len(), self.picker_sel, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_sel = Self::move_sel(self.cfg.filters.len(), self.picker_sel, -1)
            }
            KeyCode::Enter => {
                self.view = View::List;
                self.load_filter(self.picker_sel);
            }
            _ => {}
        }
    }

    fn keys_transition_picker(&mut self, key: KeyEvent) {
        if let Some(idx) = Self::picker_number(&key, self.transitions.len()) {
            if let Some(t) = self.transitions.get(idx).cloned() {
                self.view = View::List;
                self.apply_transition(t);
            }
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.view = View::List,
            KeyCode::Char('j') | KeyCode::Down => {
                self.picker_sel = Self::move_sel(self.transitions.len(), self.picker_sel, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_sel = Self::move_sel(self.transitions.len(), self.picker_sel, -1)
            }
            KeyCode::Enter => {
                if let Some(t) = self.transitions.get(self.picker_sel).cloned() {
                    self.view = View::List;
                    self.apply_transition(t);
                }
            }
            _ => {}
        }
    }

    fn pick_agent_row(&mut self, row: usize) {
        let offset = self.agent_list_offset();
        if offset > 0 && row == 0 {
            self.open_new_agent_type_picker();
            return;
        }
        let agent_idx = row.saturating_sub(offset);
        if let Some(agent) = self.agents.get(agent_idx).cloned() {
            self.view = View::List;
            self.delegate_to(agent);
        }
    }

    fn keys_agent_picker(&mut self, key: KeyEvent) {
        let len = self.agent_picker_len();
        if let Some(idx) = Self::picker_number(&key, len) {
            self.pick_agent_row(idx);
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.view = View::List,
            KeyCode::Char('n') => self.open_new_agent_type_picker(),
            KeyCode::Char('j') | KeyCode::Down => {
                self.picker_sel = Self::move_sel(len, self.picker_sel, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_sel = Self::move_sel(len, self.picker_sel, -1)
            }
            KeyCode::Enter => self.pick_agent_row(self.picker_sel),
            _ => {}
        }
    }

    fn keys_new_agent_type(&mut self, key: KeyEvent) {
        let agents = &self.cfg.delegate.agents;
        if let Some(idx) = Self::picker_number(&key, agents.len()) {
            if let Some(spawn) = agents.get(idx).cloned() {
                self.open_new_agent_workspace_picker(spawn);
            }
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                // Back to the running-agents list (not all the way to List).
                self.picker_sel = 0;
                self.view = View::AgentPicker;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.picker_sel = Self::move_sel(agents.len(), self.picker_sel, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_sel = Self::move_sel(agents.len(), self.picker_sel, -1)
            }
            KeyCode::Enter => {
                if let Some(spawn) = agents.get(self.picker_sel).cloned() {
                    self.open_new_agent_workspace_picker(spawn);
                }
            }
            _ => {}
        }
    }

    fn keys_new_agent_workspace(&mut self, key: KeyEvent) {
        if self.workspaces_loading {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                self.view = View::NewAgentTypePicker;
            }
            return;
        }
        let n = self.workspaces.len();
        if let Some(idx) = Self::picker_number(&key, n) {
            self.pick_workspace_row(idx);
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.picker_sel = 0;
                self.view = View::NewAgentTypePicker;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.picker_sel = Self::move_sel(n, self.picker_sel, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_sel = Self::move_sel(n, self.picker_sel, -1)
            }
            KeyCode::Enter => self.pick_workspace_row(self.picker_sel),
            _ => {}
        }
    }

    fn pick_workspace_row(&mut self, row: usize) {
        let Some(ws) = self.workspaces.get(row).cloned() else {
            return;
        };
        self.pending_workspace = Some(ws);
        self.open_new_agent_cwd_picker();
    }

    fn keys_new_agent_cwd(&mut self, key: KeyEvent) {
        // Last row is always "type path…"; rows above are concrete cwds.
        let n = self.cwd_choices.len() + 1;
        if let Some(idx) = Self::picker_number(&key, n) {
            self.pick_cwd_row(idx);
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.picker_sel = self
                    .pending_workspace
                    .as_ref()
                    .and_then(|pw| self.workspaces.iter().position(|w| w.id == pw.id))
                    .unwrap_or(0);
                self.view = View::NewAgentWorkspacePicker;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.picker_sel = Self::move_sel(n, self.picker_sel, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_sel = Self::move_sel(n, self.picker_sel, -1)
            }
            KeyCode::Char('/') | KeyCode::Char('e') => self.open_cwd_input(),
            KeyCode::Enter => self.pick_cwd_row(self.picker_sel),
            _ => {}
        }
    }

    fn pick_cwd_row(&mut self, row: usize) {
        if row >= self.cwd_choices.len() {
            self.open_cwd_input();
            return;
        }
        let cwd = self.cwd_choices[row].clone();
        self.view = View::List;
        self.start_new_and_delegate(cwd);
    }

    fn keys_cwd_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.view = View::NewAgentCwdPicker;
            }
            KeyCode::Enter => {
                let cwd = self.cwd_input.trim().to_string();
                if cwd.is_empty() {
                    self.toast("cwd is empty", true);
                    return;
                }
                self.view = View::List;
                self.start_new_and_delegate(cwd);
            }
            KeyCode::Backspace => {
                self.cwd_input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cwd_input.clear();
            }
            KeyCode::Char(c) => self.cwd_input.push(c),
            _ => {}
        }
    }

    fn keys_worktree_project(&mut self, key: KeyEvent) {
        // Last row is always "type path…"; rows above are herdr projects.
        let n = self.wt_projects.len() + 1;
        if let Some(idx) = Self::picker_number(&key, n) {
            self.pick_worktree_project_row(idx);
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.view = View::List,
            KeyCode::Char('j') | KeyCode::Down => {
                self.picker_sel = Self::move_sel(n, self.picker_sel, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_sel = Self::move_sel(n, self.picker_sel, -1)
            }
            KeyCode::Char('/') | KeyCode::Char('e') => self.open_worktree_repo_input(),
            KeyCode::Enter => self.pick_worktree_project_row(self.picker_sel),
            _ => {}
        }
    }

    fn pick_worktree_project_row(&mut self, row: usize) {
        let Some(project) = self.wt_projects.get(row).cloned() else {
            self.open_worktree_repo_input();
            return;
        };
        match resolve_cwd(&project.root) {
            Ok(repo) => self.open_worktree_list(repo, project.name),
            Err(e) => self.toast(format!("repo: {e}"), true),
        }
    }

    fn keys_worktree_repo_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                // Back to "type path…".
                self.picker_sel = self.wt_projects.len();
                self.view = View::WorktreeProjectPicker;
            }
            KeyCode::Enter => match resolve_cwd(&self.cwd_input) {
                Ok(repo) => {
                    let name = dir_name(&repo);
                    self.open_worktree_list(repo, name);
                }
                Err(e) => self.toast(format!("repo: {e}"), true),
            },
            KeyCode::Backspace => {
                self.cwd_input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.cwd_input.clear();
            }
            KeyCode::Char(c) => self.cwd_input.push(c),
            _ => {}
        }
    }

    /// Row 0 = "+ new worktree"; rows 1.. = the repo's existing worktrees.
    fn keys_worktree_list(&mut self, key: KeyEvent) {
        let n = self.wt_worktrees.len() + 1;
        if let Some(idx) = Self::picker_number(&key, n) {
            self.pick_worktree_list_row(idx);
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                if self.wt_project_asked {
                    self.return_to_project_picker();
                } else {
                    self.view = View::List;
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.picker_sel = Self::move_sel(n, self.picker_sel, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_sel = Self::move_sel(n, self.picker_sel, -1)
            }
            KeyCode::Enter => self.pick_worktree_list_row(self.picker_sel),
            _ => {}
        }
    }

    fn pick_worktree_list_row(&mut self, row: usize) {
        let Some(idx) = row.checked_sub(1) else {
            self.wt_existing = false;
            self.open_worktree_name_input();
            return;
        };
        let Some(wt) = self.wt_worktrees.get(idx) else {
            return;
        };
        // `open_or_create_worktree` reopens an existing branch's worktree.
        self.wt_branch = wt.branch.clone();
        self.wt_existing = true;
        self.picker_sel = 0;
        self.view = View::WorktreeAgentPicker;
    }

    fn keys_worktree_name(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.return_to_worktree_list(None),
            KeyCode::Enter => {
                let branch = sanitize_branch_name(&self.wt_name_input);
                if branch.is_empty() {
                    self.toast("worktree name is not a valid git branch name", true);
                    return;
                }
                self.wt_branch = branch;
                self.picker_sel = 0;
                self.view = View::WorktreeAgentPicker;
            }
            KeyCode::Backspace => {
                self.wt_name_input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.wt_name_input.clear();
            }
            KeyCode::Char(c) => self.wt_name_input.push(c),
            _ => {}
        }
    }

    /// Row 0 = no agent (just open the worktree); rows 1.. = `[delegate].agents`.
    fn keys_worktree_agent(&mut self, key: KeyEvent) {
        let n = self.cfg.delegate.agents.len() + 1;
        if let Some(idx) = Self::picker_number(&key, n) {
            self.pick_worktree_agent_row(idx);
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                if self.wt_existing {
                    let branch = self.wt_branch.clone();
                    self.return_to_worktree_list(Some(branch.as_str()));
                } else {
                    self.view = View::WorktreeNameInput;
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.picker_sel = Self::move_sel(n, self.picker_sel, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_sel = Self::move_sel(n, self.picker_sel, -1)
            }
            KeyCode::Enter => self.pick_worktree_agent_row(self.picker_sel),
            _ => {}
        }
    }

    fn pick_worktree_agent_row(&mut self, row: usize) {
        let spawn = match row.checked_sub(1) {
            None => None,
            Some(idx) => match self.cfg.delegate.agents.get(idx).cloned() {
                Some(spawn) => Some(spawn),
                None => return,
            },
        };
        let Some(issue) = self.selected_issue() else {
            return;
        };
        let opts = worktree_opts(&self.cfg, issue, &self.wt_branch);
        self.view = View::List;
        match spawn {
            None => self.open_worktree_only(opts),
            Some(spawn) => self.start_worktree_agent(spawn, opts),
        }
    }

    fn keys_search(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.view = View::List,
            KeyCode::Enter => {
                let q = self.search_input.trim().to_string();
                self.view = View::List;
                if !q.is_empty() {
                    self.run_search(q);
                }
            }
            KeyCode::Backspace => {
                self.search_input.pop();
            }
            KeyCode::Char(c) => self.search_input.push(c),
            _ => {}
        }
    }

    fn keys_jql(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.view = View::List,
            KeyCode::Enter => {
                let jql = self.jql_input.trim().to_string();
                self.view = View::List;
                if !jql.is_empty() {
                    let expanded = self.cfg.expand_jql(&jql);
                    self.spawn_search(expanded, "custom JQL".into());
                }
            }
            KeyCode::Backspace => {
                self.jql_input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.jql_input.clear();
            }
            KeyCode::Char(c) => self.jql_input.push(c),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_issue() -> Issue {
        Issue {
            key: "PROJ-7".into(),
            summary: "Fix login".into(),
            status: "To Do".into(),
            status_category: "new".into(),
            issue_type: "Bug".into(),
            priority: "High".into(),
            assignee: "Vitalii".into(),
            reporter: "Someone".into(),
            updated: "2026-07-14 10:00".into(),
            labels: vec!["auth".into(), "urgent".into()],
            description: "Steps to reproduce…".into(),
            url: "https://x.atlassian.net/browse/PROJ-7".into(),
        }
    }

    fn test_cfg(prompt: &str, max_desc: usize) -> Config {
        let mut cfg: Config =
            toml::from_str("[jira]\nbase_url = \"https://x.atlassian.net\"\n").unwrap();
        cfg.delegate.prompt = prompt.into();
        cfg.delegate.max_description_chars = max_desc;
        cfg
    }

    #[test]
    fn prompt_placeholders_are_filled() {
        let cfg = test_cfg(
            "{key}: {summary} [{status}/{priority}] {labels}\n{description}\n{url}",
            0,
        );
        let p = build_prompt(&cfg, &test_issue());
        assert_eq!(
            p,
            "PROJ-7: Fix login [To Do/High] auth, urgent\nSteps to reproduce…\nhttps://x.atlassian.net/browse/PROJ-7"
        );
    }

    #[test]
    fn long_descriptions_are_truncated() {
        let cfg = test_cfg("{description}", 10);
        let mut issue = test_issue();
        issue.description = "x".repeat(50);
        let p = build_prompt(&cfg, &issue);
        assert!(p.starts_with("xxxxxxxxxx\n[… description truncated]"));
    }

    #[test]
    fn empty_description_gets_placeholder() {
        let cfg = test_cfg("{description}", 0);
        let mut issue = test_issue();
        issue.description = "  ".into();
        assert_eq!(build_prompt(&cfg, &issue), "(no description)");
    }

    fn branch_for(template: &str, summary: &str) -> String {
        let mut issue = test_issue();
        issue.summary = summary.into();
        render_branch_name(template, &issue)
    }

    #[test]
    fn branch_default_template_is_the_key() {
        assert_eq!(branch_for("{key}", "Fix login"), "PROJ-7");
        assert_eq!(branch_for("{type}/{key}", "Fix login"), "bug/PROJ-7");
    }

    #[test]
    fn branch_slug_collapses_punctuation() {
        assert_eq!(
            branch_for("{key}-{slug}", "Fix: login  crash (on iOS 17)!"),
            "PROJ-7-fix-login-crash-on-ios-17"
        );
    }

    #[test]
    fn branch_non_ascii_summary_collapses_to_key() {
        assert_eq!(branch_for("{key}-{slug}", "로그인 버그 수정"), "PROJ-7");
    }

    #[test]
    fn branch_hostile_templates_become_valid_refs() {
        assert_eq!(branch_for("-feature/..x @{y}.lock/", ""), "feature");
        assert_eq!(branch_for("{key}~^:?*[\\ name.lock", ""), "PROJ-7-name");
        assert_eq!(branch_for("a.lock/b..c//d.", ""), "a/b.c/d");
        assert_eq!(branch_for("@", ""), "");
        assert_eq!(branch_for("{slug}", "!!!"), "");
    }

    #[test]
    fn branch_long_slug_is_cut_at_a_word_boundary() {
        assert_eq!(
            branch_for(
                "{key}-{slug}",
                "Implement the extremely comprehensive authentication refactoring plan"
            ),
            "PROJ-7-implement-the-extremely-comprehensive"
        );
    }

    /// App on the issue list with `test_issue` selected; no herdr calls.
    fn list_app() -> App {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut app = App::unloaded(tx);
        app.cfg = test_cfg("{key}", 0);
        app.cfg.delegate.agents = vec![SpawnAgent {
            name: "claude".into(),
            command: vec!["claude".into()],
        }];
        app.issues = vec![test_issue()];
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn temp_dir() -> String {
        std::env::temp_dir().to_string_lossy().into_owned()
    }

    #[test]
    fn project_key_is_everything_before_the_last_dash() {
        assert_eq!(project_key("PROJ-1666"), "PROJ");
        assert_eq!(project_key("A-B-12"), "A-B");
    }

    fn project(name: &str, current: bool) -> herdr::RepoProject {
        herdr::RepoProject {
            name: name.into(),
            root: temp_dir(),
            repo_key: format!("/x/{name}/.git"),
            open_worktrees: 0,
            current,
        }
    }

    fn worktree(branch: &str) -> herdr::RepoWorktree {
        herdr::RepoWorktree {
            branch: branch.into(),
            path: format!("/x/{branch}"),
            open: false,
            is_main: false,
        }
    }

    #[test]
    fn worktree_with_configured_repo_lists_its_worktrees() {
        let mut app = list_app();
        app.cfg.worktree.repos.insert("PROJ".into(), temp_dir());
        press(&mut app, KeyCode::Char('w'));
        assert_eq!(app.view, View::WorktreeListPicker);
        assert_eq!(app.wt_repo, resolve_cwd(&temp_dir()).unwrap());
        assert!(!app.wt_project_asked);
        // Row 0 = "+ new worktree", prefilled from `[worktree].branch`.
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.view, View::WorktreeNameInput);
        assert_eq!(app.wt_name_input, "PROJ-7");
    }

    #[test]
    fn worktree_without_repo_asks_for_a_herdr_project() {
        let mut app = list_app();
        press(&mut app, KeyCode::Char('w'));
        assert_eq!(app.view, View::WorktreeProjectPicker);
        assert!(app.wt_project_asked);
        app.on_resp(Resp::Projects(Ok(vec![
            project("jira", true),
            project("web", false),
        ])));
        assert_eq!(app.wt_projects.len(), 2);
        assert_eq!(app.picker_sel, 0);

        press(&mut app, KeyCode::Char('2'));
        assert_eq!(app.view, View::WorktreeListPicker);
        assert_eq!(app.wt_repo_name, "web");
        assert_eq!(app.wt_repo, resolve_cwd(&temp_dir()).unwrap());
    }

    #[test]
    fn existing_worktree_of_the_issue_branch_is_preselected_and_reopened() {
        let mut app = list_app();
        press(&mut app, KeyCode::Char('w'));
        app.on_resp(Resp::Projects(Ok(vec![project("jira", true)])));
        press(&mut app, KeyCode::Enter);
        let repo = app.wt_repo.clone();
        app.on_resp(Resp::RepoWorktrees {
            repo,
            result: Ok(vec![worktree("main"), worktree("PROJ-7")]),
        });
        assert_eq!(app.picker_sel, 2);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.view, View::WorktreeAgentPicker);
        assert_eq!(app.wt_branch, "PROJ-7");

        // Esc: agent picker -> worktree list (same row) -> project picker -> List.
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.view, View::WorktreeListPicker);
        assert_eq!(app.picker_sel, 2);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.view, View::WorktreeProjectPicker);
        assert_eq!(app.picker_sel, 0);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.view, View::List);
    }

    #[test]
    fn stale_or_failed_herdr_answers_keep_the_flow_usable() {
        let mut app = list_app();
        app.cfg.worktree.repo = temp_dir();
        press(&mut app, KeyCode::Char('w'));
        // Another repo's answer is ignored.
        app.on_resp(Resp::RepoWorktrees {
            repo: "/elsewhere".into(),
            result: Ok(vec![worktree("x")]),
        });
        assert!(app.wt_worktrees.is_empty());
        assert!(app.wt_worktrees_loading);
        // A failed listing still offers "+ new worktree".
        let repo = app.wt_repo.clone();
        app.on_resp(Resp::RepoWorktrees {
            repo,
            result: Err("boom".into()),
        });
        assert!(matches!(&app.toast, Some((_, true, _))));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.view, View::WorktreeNameInput);

        // Projects arriving after `w` was left are dropped.
        let mut app = list_app();
        press(&mut app, KeyCode::Char('w'));
        press(&mut app, KeyCode::Esc);
        app.on_resp(Resp::Projects(Ok(vec![project("jira", true)])));
        assert!(app.wt_projects.is_empty());
        assert_eq!(app.view, View::List);
    }

    #[test]
    fn worktree_name_must_sanitize_to_a_branch() {
        let mut app = list_app();
        app.cfg.worktree.repo = temp_dir();
        press(&mut app, KeyCode::Char('w'));
        press(&mut app, KeyCode::Enter);
        app.wt_name_input = "@".into();
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.view, View::WorktreeNameInput);
        assert!(matches!(&app.toast, Some((_, true, _))));

        app.wt_name_input = "feature/x y".into();
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.view, View::WorktreeAgentPicker);
        assert_eq!(app.wt_branch, "feature/x-y");
    }

    #[test]
    fn worktree_esc_walks_back_one_step() {
        // Repo from config: agent picker -> name input (edit kept) -> list -> List.
        let mut app = list_app();
        app.cfg.worktree.repos.insert("PROJ".into(), temp_dir());
        press(&mut app, KeyCode::Char('w'));
        press(&mut app, KeyCode::Enter);
        app.wt_name_input = "feature/x y".into();
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.view, View::WorktreeAgentPicker);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.view, View::WorktreeNameInput);
        assert_eq!(app.wt_name_input, "feature/x y");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.view, View::WorktreeListPicker);
        assert_eq!(app.picker_sel, 0);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.view, View::List);

        // Typed path: name input -> list -> project picker ("type path…") -> path input.
        let mut app = list_app();
        press(&mut app, KeyCode::Char('w'));
        press(&mut app, KeyCode::Char('/'));
        assert_eq!(app.view, View::WorktreeRepoInput);
        app.cwd_input = temp_dir();
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.view, View::WorktreeListPicker);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.view, View::WorktreeNameInput);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.view, View::WorktreeListPicker);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.view, View::WorktreeProjectPicker);
        assert_eq!(app.picker_sel, app.wt_projects.len());
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.view, View::WorktreeRepoInput);
    }
}

/// Fill the delegate prompt template with issue fields.
pub fn build_prompt(cfg: &Config, issue: &Issue) -> String {
    let mut desc = issue.description.clone();
    if desc.trim().is_empty() {
        desc = "(no description)".into();
    }
    let max = cfg.delegate.max_description_chars;
    if max > 0 && desc.chars().count() > max {
        desc = desc.chars().take(max).collect::<String>() + "\n[… description truncated]";
    }
    cfg.delegate
        .prompt
        .replace("{key}", &issue.key)
        .replace("{summary}", &issue.summary)
        .replace("{description}", &desc)
        .replace("{url}", &issue.url)
        .replace("{status}", &issue.status)
        .replace("{assignee}", &issue.assignee)
        .replace("{reporter}", &issue.reporter)
        .replace("{priority}", &issue.priority)
        .replace("{type}", &issue.issue_type)
        .replace("{labels}", &issue.labels.join(", "))
        .trim()
        .to_string()
}

/// Longest `{slug}` / `{type}` substitution in worktree templates.
const SLUG_MAX: usize = 40;

/// Git-branch name for an issue from a `[worktree].branch` template.
/// Placeholders: {key} {type} {slug}. The result passes
/// `git check-ref-format --branch`; empty when nothing valid remains.
pub fn render_branch_name(template: &str, issue: &Issue) -> String {
    sanitize_branch_name(&fill_issue_placeholders(template, issue))
}

/// Fill a worktree path/label template: {key} {type} {slug} {branch}.
fn fill_worktree_template(template: &str, issue: &Issue, branch: &str) -> String {
    fill_issue_placeholders(template, issue).replace("{branch}", branch)
}

fn fill_issue_placeholders(template: &str, issue: &Issue) -> String {
    template
        .replace("{key}", &issue.key)
        .replace("{type}", &slugify(&issue.issue_type, SLUG_MAX))
        .replace("{slug}", &slugify(&issue.summary, SLUG_MAX))
}

/// Lowercase ASCII alphanumerics; every other run of chars becomes one '-'.
/// Longer than `max` → cut back to the last '-' boundary within `max`.
fn slugify(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max + 1));
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-');
    if out.len() <= max {
        return out.to_string();
    }
    // ASCII only, so byte indices are char boundaries.
    let head = &out[..max];
    let cut = if out.as_bytes()[max] == b'-' {
        head
    } else {
        head.rfind('-').map_or(head, |i| &head[..i])
    };
    cut.trim_end_matches('-').to_string()
}

/// Rewrite `raw` so it satisfies `git check-ref-format --branch`.
fn sanitize_branch_name(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\') {
                '-'
            } else {
                c
            }
        })
        .collect();
    loop {
        let before = s.clone();
        s = s.replace("@{", "-");
        while s.contains("..") {
            s = s.replace("..", ".");
        }
        s = collapse_runs(&s, '-');
        s = collapse_runs(&s, '/');
        s = s
            .split('/')
            .filter(|comp| !comp.starts_with('.'))
            .map(|comp| comp.strip_suffix(".lock").unwrap_or(comp))
            .collect::<Vec<_>>()
            .join("/");
        s = s
            .trim_start_matches(['-', '/', '.'])
            .trim_end_matches(['/', '.', '-'])
            .to_string();
        if s == before {
            break;
        }
    }
    if s == "@" || s == "HEAD" {
        String::new()
    } else {
        s
    }
}

fn collapse_runs(s: &str, ch: char) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if !(c == ch && out.ends_with(ch)) {
            out.push(c);
        }
    }
    out
}

/// Expand and validate a directory (worktree repo): non-empty existing directory.
fn resolve_cwd(cwd_raw: &str) -> Result<String, String> {
    let cwd = herdr::expand_path(cwd_raw);
    if cwd.is_empty() {
        return Err("path is empty".into());
    }
    if !Path::new(&cwd).is_dir() {
        return Err(format!("not a directory: {cwd}"));
    }
    Ok(cwd)
}

/// Jira project key of an issue key: everything before the last '-'
/// ("PROJ-1666" -> "PROJ").
fn project_key(issue_key: &str) -> &str {
    issue_key
        .rsplit_once('-')
        .map_or(issue_key, |(project, _)| project)
}

/// Last path component, for display ("/x/backend" -> "backend").
fn dir_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

/// Run herdr CLI work off the UI thread; it reports back through `Resp`.
/// Unit tests skip it: they must never reach a real herdr (or race the mock
/// CLI in `herdr::tests`) and feed `App::on_resp` directly instead.
fn spawn_herdr(work: impl FnOnce() + Send + 'static) {
    if cfg!(test) {
        return;
    }
    std::thread::spawn(work);
}

/// `herdr worktree` options for `issue` on `branch` from `[worktree]`.
fn worktree_opts(cfg: &Config, issue: &Issue, branch: &str) -> herdr::WorktreeOpts {
    let wcfg = &cfg.worktree;
    let path = if wcfg.path.trim().is_empty() {
        String::new()
    } else {
        herdr::expand_path(&fill_worktree_template(wcfg.path.trim(), issue, branch))
    };
    herdr::WorktreeOpts {
        branch: branch.to_string(),
        base: wcfg.base.trim().to_string(),
        path,
        label: fill_worktree_template(wcfg.label.trim(), issue, branch),
        trust_repository: wcfg.trust_repository,
    }
}

/// Build cwd options: configured default, unique cwds from running agents,
/// then common parents. Paths are stored expanded.
fn collect_cwd_choices(cfg: &Config, agents: &[HerdrAgent]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        let p = herdr::expand_path(raw);
        if p.is_empty() || !Path::new(&p).is_dir() {
            return;
        }
        if !out.iter().any(|x| x == &p) {
            out.push(p);
        }
    };

    if !cfg.delegate.default_cwd.trim().is_empty() {
        push(&cfg.delegate.default_cwd);
    }
    for a in agents {
        if !a.cwd.is_empty() {
            // Skip herdr-mirror helper panes — not useful work dirs.
            if a.cwd.contains("herdr-mirror") {
                continue;
            }
            push(&a.cwd);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        push(&cwd.to_string_lossy());
    }
    if let Ok(home) = std::env::var("HOME") {
        push(&home);
        push(&format!("{home}/Work"));
        push(&format!("{home}/Projects"));
        push(&format!("{home}/src"));
    }
    out
}

/// Unique herdr agent name: issue key + agent label + short time suffix so a
/// second delegate of the same issue does not collide.
fn unique_agent_name(issue_key: &str, agent_label: &str) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() % 10_000)
        .unwrap_or(0);
    // Keep names shell/CLI friendly.
    let key = issue_key.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "-");
    let label = agent_label.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "-");
    format!("{key}-{label}-{secs}")
}

fn short_home(p: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && p.starts_with(&home) {
        format!("~{}", &p[home.len()..])
    } else {
        p.to_string()
    }
}

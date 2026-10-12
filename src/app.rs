//! Application state and key handling. Network and herdr-CLI work runs on
//! background threads; results come back over an mpsc channel as `Resp`.

use crate::config::{Config, SpawnAgent};
use crate::herdr::{self, HerdrAgent, HerdrWorkspace, StartAgentOpts};
use crate::jira::{Comment, Issue, JiraClient, Page, Transition};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    HandoffConfirm,
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
        generation: u64,
        result: Result<Page<Issue>, String>,
    },
    Transitions {
        key: String,
        token: u64,
        result: Result<Vec<Transition>, String>,
    },
    Transitioned {
        key: String,
        name: String,
        token: u64,
        result: Result<(), String>,
    },
    Agents(Result<Vec<HerdrAgent>, String>),
    Workspaces(Result<Vec<HerdrWorkspace>, String>),
    HandoffPrepared {
        token: u64,
        result: Result<PromptPreview, String>,
    },
    Delegated {
        token: u64,
        key: String,
        label: String,
        submitted: bool,
        result: Result<HerdrAgent, herdr::DelegationFailure>,
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
        token: u64,
        result: Result<Page<Issue>, String>,
    },
    Detail {
        key: String,
        token: u64,
        result: Result<Issue, String>,
    },
    Comments {
        key: String,
        token: u64,
        result: Result<Page<Comment>, String>,
    },
}
pub fn is_epic(issue: &Issue) -> bool {
    issue.issue_type.eq_ignore_ascii_case("epic")
}

#[derive(Debug, Clone)]
pub struct PromptPreview {
    pub key: String,
    pub text: String,
    pub warnings: Vec<String>,
}

#[derive(Clone)]
enum HandoffDestination {
    Existing(HerdrAgent),
    New(StartAgentOpts),
}

#[derive(Clone)]
struct HandoffOperation {
    token: u64,
    key: String,
    destination: HandoffDestination,
    submit: bool,
    startup: u64,
    wait_ready: u64,
}

struct PendingTransition {
    token: u64,
    key: String,
}

struct RecoveryHandoff {
    agent: HerdrAgent,
    operation: HandoffOperation,
    preview: PromptPreview,
}

pub struct App {
    pub cfg: Config,
    pub client: Option<Arc<JiraClient>>,
    pub tx: Sender<Resp>,

    pub view: View,
    /// View that `Help` was opened from; drawn under the popup and restored
    /// when it closes.
    pub help_return: View,
    pub should_quit: bool,

    pub issues: Vec<Issue>,
    pub issues_total: Option<usize>,
    pub issues_has_more: bool,
    pub list_error: Option<String>,
    /// Epic expansion state: children keyed by epic key, plus which epics are
    /// currently open and which are still fetching.
    pub children: HashMap<String, Vec<Issue>>,
    pub children_total: HashMap<String, usize>,
    pub children_has_more: HashSet<String>,
    pub children_err: HashMap<String, String>,
    pub expanded: HashSet<String>,
    pub loading_children: HashSet<String>,
    pub selected: usize,
    pub current_title: String,
    pub loading: bool,
    /// The active query is independent of the currently highlighted filter.
    active_title: String,
    query_generation: u64,
    request_serial: u64,
    child_requests: HashMap<String, u64>,
    comment_requests: HashMap<String, u64>,
    detail_request: Option<u64>,
    transitions_request: Option<u64>,
    pending_transition: Option<PendingTransition>,
    pub detail_key: Option<String>,
    pub detail_issue: Option<Issue>,
    pub detail_loading: bool,
    pub detail_error: Option<String>,
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
    wizard_issue: Option<Issue>,
    handoff: Option<HandoffOperation>,
    pub handoff_preview: Option<PromptPreview>,
    pub handoff_scroll: u16,
    pub handoff_scroll_max: Cell<u16>,
    pub handoff_loading: bool,
    pub handoff_error: Option<String>,
    recovery: Option<RecoveryHandoff>,

    pub search_input: String,
    pub jql_input: String,
    pub last_jql: String,
    pub detail_scroll: u16,
    /// Largest useful scroll offset per detail pane, written by the renderer
    /// (it alone knows the wrapped height) so keys can't scroll past the end.
    pub detail_scroll_max: Cell<u16>,
    pub comment_scroll_max: Cell<u16>,
    /// Detail view pane rects from the last render, for mouse hit-testing;
    /// the comments rect stays empty when the pane is hidden.
    pub detail_issue_area: Cell<Rect>,
    pub detail_comments_area: Cell<Rect>,

    /// Comments per issue key (newest first), in-flight fetches and failures.
    pub comments: HashMap<String, Vec<Comment>>,
    pub comments_loading: HashSet<String>,
    pub comments_err: HashMap<String, String>,
    pub comments_total: HashMap<String, usize>,
    pub comments_has_more: HashSet<String>,
    pub comment_scroll: u16,
    /// Detail view: j/k scroll the comments pane instead of the description.
    pub detail_focus_comments: bool,

    pub toast: Option<(String, bool, Instant)>, // message, is_error, shown_at
}

/// Only an explicit first-page rejection of the `parent` field can use the
/// Server/DC "Epic Link" query. Later-page errors carry a different prefix.
fn unsupported_parent_field(error: &str) -> bool {
    let Some(message) = error.strip_prefix("HTTP 400:") else {
        return false;
    };
    let message = message.to_ascii_lowercase();
    if message.contains("permission") {
        return false;
    }
    ["'parent'", "\"parent\"", "`parent`"].iter().any(|field| {
        message.contains(&format!("field {field} does not exist"))
            || message.contains(&format!("unknown field {field}"))
            || message.contains(&format!("field {field} is not supported"))
    })
}

impl App {
    pub fn new(tx: Sender<Resp>) -> Self {
        let mut app = Self::unloaded(tx);
        let _ = app.reload_config();
        app
    }

    /// Empty state with the built-in default config; `new` then loads the real one.
    /// Crate-visible so render tests can build an App without config or network.
    pub(crate) fn unloaded(tx: Sender<Resp>) -> Self {
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
            help_return: View::List,
            should_quit: false,
            issues: vec![],
            issues_total: None,
            issues_has_more: false,
            list_error: None,
            children: HashMap::new(),
            children_total: HashMap::new(),
            children_has_more: HashSet::new(),
            children_err: HashMap::new(),
            expanded: HashSet::new(),
            loading_children: HashSet::new(),
            selected: 0,
            current_title: String::new(),
            loading: false,
            active_title: String::new(),
            query_generation: 0,
            request_serial: 0,
            child_requests: HashMap::new(),
            comment_requests: HashMap::new(),
            detail_request: None,
            transitions_request: None,
            pending_transition: None,
            detail_key: None,
            detail_issue: None,
            detail_loading: false,
            detail_error: None,
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
            wizard_issue: None,
            handoff: None,
            handoff_preview: None,
            handoff_scroll: 0,
            handoff_scroll_max: Cell::new(0),
            handoff_loading: false,
            handoff_error: None,
            recovery: None,
            search_input: String::new(),
            jql_input: String::new(),
            last_jql: String::new(),
            detail_scroll: 0,
            detail_scroll_max: Cell::new(0),
            comment_scroll_max: Cell::new(0),
            detail_issue_area: Cell::new(Rect::default()),
            detail_comments_area: Cell::new(Rect::default()),
            comments: HashMap::new(),
            comments_loading: HashSet::new(),
            comments_err: HashMap::new(),
            comments_total: HashMap::new(),
            comments_has_more: HashSet::new(),
            comment_scroll: 0,
            detail_focus_comments: false,
            toast: None,
        }
    }

    pub fn reload_config(&mut self) -> bool {
        if self.handoff_loading && self.handoff_preview.is_some() {
            self.toast(
                "handoff already dispatched — wait for delivery result before reloading",
                true,
            );
            return false;
        }
        if self.pending_transition.is_some() {
            self.toast(
                "transition already dispatched — wait for result before reloading",
                true,
            );
            return false;
        }
        self.cancel_handoff();
        if self.view == View::List {
            self.return_to_list();
        } else {
            self.wizard_issue = None;
        }
        self.invalidate_requests();
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
                    self.client = None;
                    self.fatal = Some(format!(
                        "{e}\n\nEdit {}: set [jira].base_url and api_token (or api_token_cmd); \
                         for basic auth also set email. No checkout-local config is required.",
                        crate::config::config_path().display()
                    ));
                }
            },
            Err(e) => {
                self.client = None;
                self.fatal = Some(e);
            }
        }
        true
    }

    fn next_token(&mut self) -> u64 {
        self.request_serial = self.request_serial.wrapping_add(1);
        self.request_serial
    }

    fn invalidate_requests(&mut self) {
        self.query_generation = self.query_generation.wrapping_add(1);
        self.next_token();
        self.child_requests.clear();
        self.comment_requests.clear();
        self.detail_request = None;
        self.transitions_request = None;
        self.transitions_loading = false;
        self.loading = false;
        self.loading_children.clear();
        self.comments_loading.clear();
        self.detail_loading = false;
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
        if self.detail_key.is_some()
            && (self.view == View::Detail
                || matches!(self.view, View::Help | View::HandoffConfirm)
                    && self.help_return == View::Detail)
        {
            let key = self.detail_key.as_deref()?;
            return self
                .detail_issue
                .as_ref()
                .filter(|issue| issue.key == key)
                .or_else(|| {
                    self.visible()
                        .into_iter()
                        .find(|(issue, _)| issue.key == key)
                        .map(|(issue, _)| issue)
                });
        }
        self.visible().get(self.selected).map(|(issue, _)| *issue)
    }

    pub(crate) fn wizard_issue_key(&self) -> Option<&str> {
        self.wizard_issue.as_ref().map(|issue| issue.key.as_str())
    }

    pub(crate) fn handoff_recovery_agent(&self) -> Option<&HerdrAgent> {
        self.recovery.as_ref().map(|recovery| &recovery.agent)
    }

    /// A real exit to the list, never the temporary list during handoff prep.
    fn return_to_list(&mut self) {
        self.view = View::List;
        self.help_return = View::List;
        self.wizard_issue = None;
        self.detail_key = None;
        self.detail_issue = None;
        self.detail_error = None;
        self.detail_request = None;
        self.detail_loading = false;
        self.comment_requests.clear();
        self.comments_loading.clear();
        self.comments_err.clear();
        self.transitions_request = None;
        self.transitions_loading = false;
        self.transitions.clear();
        self.transitions_for.clear();
        self.agents_loading = false;
        self.workspaces_loading = false;
        self.wt_projects_loading = false;
        self.wt_worktrees_loading = false;
    }

    fn restore_handoff_origin(&mut self) {
        if self.help_return == View::Detail {
            self.wizard_issue = None;
            self.view = View::Detail;
        } else {
            self.return_to_list();
        }
    }

    fn selected_key(&self) -> Option<String> {
        self.visible()
            .get(self.selected)
            .map(|(issue, _)| issue.key.clone())
    }

    fn preserve_selection(&mut self, key: Option<&str>) {
        let rows = self.visible();
        self.selected = key
            .and_then(|key| rows.iter().position(|(issue, _)| issue.key == key))
            .unwrap_or_else(|| self.selected.min(rows.len().saturating_sub(1)));
    }

    fn toast(&mut self, msg: impl Into<String>, is_error: bool) {
        self.toast = Some((msg.into(), is_error, Instant::now()));
    }

    // ---- background requests ----

    fn spawn_search(&mut self, jql: String, title: String) {
        self.last_jql = jql.clone();
        self.active_title = title.clone();
        self.current_title = title;
        self.query_generation = self.query_generation.wrapping_add(1);
        self.child_requests.clear();
        self.loading_children.clear();
        let generation = self.query_generation;
        let Some(client) = self.client.clone() else {
            return;
        };
        self.loading = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = client.search(&jql);
            let _ = tx.send(Resp::Issues { generation, result });
        });
    }

    fn refresh_query(&mut self) {
        if !self.last_jql.is_empty() {
            self.spawn_search(self.last_jql.clone(), self.active_title.clone());
        } else {
            self.load_filter(self.filter_idx);
        }
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
        let token = self.next_token();
        self.transitions_request = Some(token);
        self.transitions_for = key.clone();
        self.transitions.clear();
        self.transitions_loading = true;
        self.picker_sel = 0;
        self.view = View::TransitionPicker;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = client.transitions(&key);
            let _ = tx.send(Resp::Transitions { key, token, result });
        });
    }

    fn apply_transition(&mut self, t: Transition) -> bool {
        if self.pending_transition.is_some() {
            self.toast("transition already applying — wait for result", true);
            return false;
        }
        let Some(client) = self.client.clone() else {
            return false;
        };
        let key = self.transitions_for.clone();
        let token = self.next_token();
        self.pending_transition = Some(PendingTransition {
            token,
            key: key.clone(),
        });
        let tx = self.tx.clone();
        self.toast(format!("{key}: applying \"{}\"…", t.name), false);
        std::thread::spawn(move || {
            let result = client.apply_transition(&key, &t.id);
            let _ = tx.send(Resp::Transitioned {
                key,
                name: t.to_status,
                token,
                result,
            });
        });
        true
    }

    /// Explicit detail entry/refresh always fetches; cached values remain visible.
    fn fetch_comments(&mut self, key: String) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let token = self.next_token();
        self.comment_requests.insert(key.clone(), token);
        self.comments_loading.insert(key.clone());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = client.comments(&key);
            let _ = tx.send(Resp::Comments { key, token, result });
        });
    }

    fn fetch_detail(&mut self, key: String) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let token = self.next_token();
        self.detail_request = Some(token);
        self.detail_loading = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = client.issue(&key);
            let _ = tx.send(Resp::Detail { key, token, result });
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
        self.expanded.insert(key.clone());
        if self.children.contains_key(&key) {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let token = self.next_token();
        self.child_requests.insert(key.clone(), token);
        self.loading_children.insert(key.clone());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            // Cloud uses `parent`; Server/DC may require the classic "Epic Link" field.
            let result = client.search(&format!("parent = {key} ORDER BY created ASC"));
            let result = if result
                .as_ref()
                .err()
                .is_some_and(|error| unsupported_parent_field(error))
            {
                client.search(&format!("\"Epic Link\" = {key} ORDER BY created ASC"))
            } else {
                result
            };
            let _ = tx.send(Resp::Children {
                epic: key,
                token,
                result,
            });
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
        self.child_requests.remove(&epic_key);
        self.loading_children.remove(&epic_key);
        self.expanded.remove(&epic_key);
        // Land the selection on the epic row itself.
        if let Some(idx) = self.visible().iter().position(|(i, _)| i.key == epic_key) {
            self.selected = idx;
        }
    }

    fn request_agents(&mut self) {
        if self.handoff_loading || self.handoff.is_some() {
            return;
        }
        let Some(issue) = self.selected_issue().cloned() else {
            return;
        };
        self.wizard_issue = Some(issue);
        self.help_return = self.view;
        self.agents.clear();
        self.agents_loading = true;
        self.picker_sel = 0;
        self.pending_spawn = None;
        self.view = View::AgentPicker;
        let tx = self.tx.clone();
        spawn_herdr(move || {
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
        self.prepare_handoff(
            HandoffDestination::Existing(agent),
            self.cfg.delegate.submit,
        );
    }

    /// Spawn a fresh agent in the chosen workspace + cwd, wait until ready,
    /// send the Jira prompt.
    fn start_new_and_delegate(&mut self, cwd_raw: String) {
        let Some(issue) = self.wizard_issue.as_ref() else {
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

        let key = issue.key.clone();
        let placement = self.cfg.delegate.placement.clone();
        let focus = self.cfg.delegate.focus_new;
        let name = unique_agent_name(&key, &spawn.name);
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
        self.toast(
            format!(
                "preparing {key} for {ws_label} ({place_hint}, {})…",
                short_home(&cwd)
            ),
            false,
        );
        self.prepare_handoff(HandoffDestination::New(opts), self.cfg.delegate.submit);
    }

    fn cancel_handoff(&mut self) {
        self.handoff = None;
        self.handoff_preview = None;
        self.handoff_loading = false;
        self.next_token();
    }

    fn prepare_handoff(&mut self, destination: HandoffDestination, submit: bool) {
        if self.handoff.is_some() {
            return;
        }
        let Some(key) = self.wizard_issue.as_ref().map(|issue| issue.key.clone()) else {
            return;
        };
        let token = self.next_token();
        self.handoff_error = None;
        self.recovery = None;
        self.handoff_preview = None;
        self.handoff_scroll = 0;
        self.handoff_loading = true;
        let op = HandoffOperation {
            token,
            key: key.clone(),
            destination,
            submit,
            startup: self.cfg.delegate.startup_delay_ms,
            wait_ready: self.cfg.delegate.wait_ready_ms,
        };
        self.handoff = Some(op);
        let cfg = self.cfg.clone();
        let client = self.client.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = prepare_prompt(&cfg, client.as_deref(), &key);
            let _ = tx.send(Resp::HandoffPrepared { token, result });
        });
    }

    fn dispatch_handoff(&mut self) {
        let Some(op) = self.handoff.as_ref().cloned() else {
            return;
        };
        let Some(preview) = self.handoff_preview.as_ref() else {
            return;
        };
        let text = preview.text.clone();
        let key = op.key.clone();
        let label = match &op.destination {
            HandoffDestination::Existing(a) => a.label.clone(),
            HandoffDestination::New(opts) => opts.name.clone(),
        };
        self.view = self.help_return;
        self.handoff_loading = true;
        self.toast(format!("sending {key} to {label}…"), false);
        let tx = self.tx.clone();
        spawn_herdr(move || {
            let result = match &op.destination {
                HandoffDestination::Existing(agent) => {
                    herdr::send_to_agent(agent, &text, op.submit)
                        .map(|_| agent.clone())
                        .map_err(|message| herdr::DelegationFailure {
                            message,
                            agent: Some(agent.clone()),
                            retry_safe: false,
                        })
                }
                HandoffDestination::New(opts) => {
                    herdr::start_and_delegate(opts, &text, op.submit, op.startup, op.wait_ready)
                }
            };
            let _ = tx.send(Resp::Delegated {
                token: op.token,
                key,
                label,
                submitted: op.submit,
                result,
            });
        });
    }

    fn retry_handoff(&mut self) {
        if self.handoff_loading || self.handoff.is_some() {
            return;
        }
        let Some(RecoveryHandoff {
            agent,
            operation,
            preview,
        }) = self.recovery.take()
        else {
            return;
        };
        let text = preview.text.clone();
        let key = operation.key.clone();
        let label = agent.label.clone();
        let submit = operation.submit;
        let wait_ready = operation.wait_ready;
        let token = self.next_token();
        self.handoff_preview = Some(preview);
        self.handoff_loading = true;
        self.handoff_error = None;
        self.handoff = Some(HandoffOperation { token, ..operation });
        let tx = self.tx.clone();
        spawn_herdr(move || {
            let result = herdr::resume_delegation(&agent, &text, submit, wait_ready);
            let _ = tx.send(Resp::Delegated {
                token,
                key,
                label,
                submitted: submit,
                result,
            });
        });
    }

    fn keys_handoff_confirm(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.dispatch_handoff(),
            KeyCode::Esc => {
                self.cancel_handoff();
                self.restore_handoff_origin();
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.handoff_scroll = self
                    .handoff_scroll
                    .saturating_add(1)
                    .min(self.handoff_scroll_max.get())
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.handoff_scroll = self.handoff_scroll.saturating_sub(1)
            }
            KeyCode::PageDown => {
                self.handoff_scroll = self
                    .handoff_scroll
                    .saturating_add(10)
                    .min(self.handoff_scroll_max.get())
            }
            KeyCode::PageUp => self.handoff_scroll = self.handoff_scroll.saturating_sub(10),
            _ => {}
        }
    }

    // ---- `w`: git worktree for the selected issue ----

    /// Resolve the repo from `[worktree]` (per project key, then the default)
    /// or let the user pick a herdr project; then show the repo's worktrees.
    fn open_worktree(&mut self) {
        if self.handoff_loading || self.handoff.is_some() {
            return;
        }
        self.wizard_issue = self.selected_issue().cloned();
        let Some(issue) = self.wizard_issue.as_ref() else {
            return;
        };
        self.help_return = self.view;
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
            Err(e) => {
                self.wizard_issue = None;
                self.toast(format!("worktree repo: {e}"), true);
            }
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
        let Some(key) = self.wizard_issue.as_ref().map(|issue| issue.key.clone()) else {
            return;
        };
        let submit = self.cfg.worktree.submit.unwrap_or(self.cfg.delegate.submit);
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
        self.prepare_handoff(HandoffDestination::New(opts), submit);
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
            Resp::Issues { generation, result } => {
                if generation != self.query_generation {
                    return;
                }
                self.loading = false;
                match result {
                    Ok(page) => {
                        let selected_key = self.selected_key();
                        self.issues = page.items;
                        self.issues_total = page.total;
                        self.issues_has_more = page.has_more;
                        self.list_error = None;
                        self.children.clear();
                        self.children_total.clear();
                        self.children_has_more.clear();
                        self.children_err.clear();
                        self.expanded.clear();
                        self.child_requests.clear();
                        self.loading_children.clear();
                        self.preserve_selection(selected_key.as_deref());
                    }
                    Err(e) => {
                        self.list_error = Some(e.clone());
                        self.toast(format!("Jira: {e}"), true);
                    }
                }
            }
            Resp::Transitions { key, token, result } => {
                if key != self.transitions_for || self.transitions_request != Some(token) {
                    return;
                }
                self.transitions_request = None;
                self.transitions_loading = false;
                match result {
                    Ok(ts) => self.transitions = ts,
                    Err(e) => {
                        if self.view == View::TransitionPicker {
                            self.return_to_list();
                        }
                        self.toast(format!("{key}: transitions: {e}"), true);
                    }
                }
            }
            Resp::Transitioned {
                key,
                name,
                token,
                result,
            } => {
                if !self
                    .pending_transition
                    .as_ref()
                    .is_some_and(|pending| pending.token == token && pending.key == key)
                {
                    return;
                }
                self.pending_transition = None;
                match result {
                    Ok(()) => {
                        self.toast(format!("{key} → {name}"), false);
                        self.refresh_query();
                        if self.detail_key.as_deref() == Some(key.as_str())
                            && self.view == View::Detail
                        {
                            self.fetch_detail(key.clone());
                            self.fetch_comments(key);
                        }
                    }
                    Err(e) => self.toast(format!("{key}: {e}"), true),
                }
            }
            Resp::Agents(result) => {
                if self.view != View::AgentPicker || !self.agents_loading {
                    return;
                }
                self.agents_loading = false;
                match result {
                    Ok(agents) => {
                        self.agents = agents;
                        if self.agents.is_empty() && !self.can_start_new_agent() {
                            self.return_to_list();
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
                            self.return_to_list();
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
            Resp::HandoffPrepared { token, result } => {
                if self.handoff.as_ref().map_or(true, |op| op.token != token) {
                    return;
                }
                self.handoff_loading = false;
                match result {
                    Err(e) => {
                        self.handoff = None;
                        self.restore_handoff_origin();
                        self.handoff_error = Some(format!(
                            "Jira snapshot read failed: {e}; no agent or worktree started"
                        ));
                    }
                    Ok(preview) => {
                        let incomplete = !preview.warnings.is_empty();
                        self.handoff_preview = Some(preview);
                        if incomplete {
                            self.handoff_scroll = 0;
                            self.view = View::HandoffConfirm;
                        } else {
                            self.dispatch_handoff();
                        }
                    }
                }
            }
            Resp::Delegated {
                token,
                key,
                label,
                submitted,
                result,
            } => {
                if self.handoff.as_ref().map_or(true, |op| op.token != token) {
                    return;
                }
                let op = self.handoff.take().unwrap();
                self.handoff_loading = false;
                self.wizard_issue = None;
                match result {
                    Ok(_) if submitted => {
                        self.handoff_preview = None;
                        self.toast(format!("{key} delegated to {label}"), false);
                        herdr::notify(&format!("Jira {key} delegated to {label}"));
                    }
                    Ok(_) => {
                        self.handoff_preview = None;
                        self.toast(
                            format!("{key} pasted into {label} — review and press Enter"),
                            false,
                        );
                        herdr::notify(&format!("Jira {key} pasted into {label}"));
                    }
                    Err(failure) => {
                        let known_pane = failure.agent.is_some();
                        self.recovery = match (
                            failure.retry_safe,
                            failure.agent,
                            self.handoff_preview.take(),
                        ) {
                            (true, Some(agent), Some(preview)) => Some(RecoveryHandoff {
                                agent,
                                operation: op,
                                preview,
                            }),
                            _ => None,
                        };
                        let suffix = if self.recovery.is_some() {
                            "known not sent — press D to recheck the same pane"
                        } else if known_pane {
                            "delivery unknown — inspect pane; do not retry"
                        } else {
                            "not sent"
                        };
                        let msg = format!("delegate {key}: {}; {suffix}", failure.message);
                        self.handoff_error = Some(msg.clone());
                        self.toast(msg, true);
                    }
                }
            }
            Resp::WorktreeOpened { branch, result } => match result {
                Ok(path) => self.toast(
                    format!("worktree {branch} ready: {}", short_home(&path)),
                    false,
                ),
                Err(e) => self.toast(format!("worktree {branch}: {e}"), true),
            },
            Resp::Projects(result) => self.on_projects(result),
            Resp::RepoWorktrees { repo, result } => self.on_repo_worktrees(repo, result),
            Resp::Children {
                epic,
                token,
                result,
            } => {
                if self.child_requests.get(&epic) != Some(&token) || !self.expanded.contains(&epic)
                {
                    return;
                }
                self.child_requests.remove(&epic);
                self.loading_children.remove(&epic);
                match result {
                    Ok(page) => {
                        let selected_key = self.selected_key();
                        if let Some(total) = page.total {
                            self.children_total.insert(epic.clone(), total);
                        } else {
                            self.children_total.remove(&epic);
                        }
                        if page.has_more {
                            self.children_has_more.insert(epic.clone());
                        } else {
                            self.children_has_more.remove(&epic);
                        }
                        self.children_err.remove(&epic);
                        self.children.insert(epic, page.items);
                        self.preserve_selection(selected_key.as_deref());
                    }
                    Err(e) => {
                        self.children_err.insert(epic.clone(), e.clone());
                        self.toast(format!("{epic}: children: {e}"), true);
                    }
                }
            }
            Resp::Detail { key, token, result } => {
                if self.detail_key.as_deref() != Some(key.as_str())
                    || self.detail_request != Some(token)
                {
                    return;
                }
                self.detail_request = None;
                self.detail_loading = false;
                match result {
                    Ok(issue) if issue.key == key => {
                        self.detail_issue = Some(issue);
                        self.detail_error = None;
                    }
                    Ok(_) => {
                        self.detail_error = Some(format!("{key}: Jira returned a different issue"))
                    }
                    Err(e) => self.detail_error = Some(e),
                }
            }
            Resp::Comments { key, token, result } => {
                if self.comment_requests.get(&key) != Some(&token)
                    || self.detail_key.as_deref() != Some(key.as_str())
                {
                    return;
                }
                self.comment_requests.remove(&key);
                self.comments_loading.remove(&key);
                match result {
                    Ok(page) => {
                        self.comments_err.remove(&key);
                        if let Some(total) = page.total {
                            self.comments_total.insert(key.clone(), total);
                        } else {
                            self.comments_total.remove(&key);
                        }
                        if page.has_more {
                            self.comments_has_more.insert(key.clone());
                        } else {
                            self.comments_has_more.remove(&key);
                        }
                        self.comments.insert(key, page.items);
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
                KeyCode::Char('R') | KeyCode::Char('r') => {
                    let _ = self.reload_config();
                }
                _ => {}
            }
            return;
        }
        if key.code == KeyCode::Esc && self.handoff.is_some() && self.handoff_preview.is_none() {
            self.cancel_handoff();
            self.restore_handoff_origin();
            self.toast("Jira handoff preparation cancelled", false);
            return;
        }
        if key.code == KeyCode::Char('D') && matches!(self.view, View::List | View::Detail) {
            self.retry_handoff();
            return;
        }
        match self.view {
            View::HandoffConfirm => self.keys_handoff_confirm(key),
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
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                    self.view = self.help_return
                }
                _ => {}
            },
        }
    }

    /// Detail view: a left click focuses the pane under the cursor and the
    /// wheel scrolls that pane. Elsewhere the wheel acts as ↑/↓ (what the
    /// terminal sent before mouse capture was on).
    pub fn on_mouse(&mut self, m: MouseEvent) {
        if self.fatal.is_some() {
            return;
        }
        let delta = match m.kind {
            MouseEventKind::ScrollDown => 1,
            MouseEventKind::ScrollUp => -1,
            MouseEventKind::Down(MouseButton::Left) if self.view == View::Detail => {
                if let Some(comments) = self.detail_pane_at(m.column, m.row) {
                    self.detail_focus_comments = comments;
                }
                return;
            }
            _ => return,
        };
        if self.view == View::Detail {
            if let Some(comments) = self.detail_pane_at(m.column, m.row) {
                self.scroll_pane(comments, delta * 3);
            }
        } else {
            let code = if delta > 0 {
                KeyCode::Down
            } else {
                KeyCode::Up
            };
            self.on_key(KeyEvent::from(code));
        }
    }

    /// `Some(true)` over the comments pane, `Some(false)` over the issue pane.
    fn detail_pane_at(&self, column: u16, row: u16) -> Option<bool> {
        let p = Position::new(column, row);
        if self.detail_comments_area.get().contains(p) {
            Some(true)
        } else if self.detail_issue_area.get().contains(p) {
            Some(false)
        } else {
            None
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
                if let Some(issue) = self.selected_issue().cloned() {
                    let key = issue.key.clone();
                    self.comment_requests.clear();
                    self.comments_loading.clear();
                    self.detail_key = Some(key.clone());
                    self.detail_issue = Some(issue);
                    self.detail_error = None;
                    self.detail_scroll = 0;
                    self.detail_focus_comments = false;
                    self.view = View::Detail;
                    self.comment_scroll = 0;
                    self.fetch_detail(key.clone());
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
            KeyCode::Char('r') => self.refresh_query(),
            KeyCode::Char('R') => {
                if self.reload_config() && self.fatal.is_none() {
                    self.toast("config reloaded", false);
                }
            }
            KeyCode::Char('s') => self.request_transitions(),
            KeyCode::Char('d') => self.request_agents(),
            KeyCode::Char('w') => self.open_worktree(),
            KeyCode::Char('o') => self.open_in_browser(),
            KeyCode::Char('z') => self.zoom_toggle(),
            KeyCode::Char('?') => self.open_help(),
            _ => {}
        }
    }

    fn keys_detail(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.return_to_list(),
            KeyCode::Char('r') => {
                self.refresh_query();
                if let Some(key) = self.detail_key.clone().or_else(|| self.selected_key()) {
                    self.fetch_detail(key.clone());
                    self.fetch_comments(key);
                }
            }
            KeyCode::Tab => self.detail_focus_comments = !self.detail_focus_comments,
            KeyCode::Char('j') | KeyCode::Down => self.scroll_focused(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_focused(-1),
            KeyCode::PageDown => self.scroll_focused(15),
            KeyCode::PageUp => self.scroll_focused(-15),
            KeyCode::Char('g') => self.scroll_focused(i32::MIN),
            KeyCode::Char('G') => self.scroll_focused(i32::MAX),
            KeyCode::Char('s') => self.request_transitions(),
            KeyCode::Char('d') => self.request_agents(),
            KeyCode::Char('w') => self.open_worktree(),
            KeyCode::Char('o') => self.open_in_browser(),
            KeyCode::Char('z') => self.zoom_toggle(),
            KeyCode::Char('?') => self.open_help(),
            _ => {}
        }
    }

    fn open_help(&mut self) {
        self.help_return = self.view;
        self.view = View::Help;
    }

    /// View drawn under popups: `Help` shows over the view it came from.
    pub fn base_view(&self) -> View {
        if matches!(self.view, View::Help | View::HandoffConfirm) {
            self.help_return
        } else {
            self.view
        }
    }

    fn scroll_focused(&mut self, delta: i32) {
        self.scroll_pane(self.detail_focus_comments, delta);
    }

    /// Detail view: move one pane's scroll by `delta` lines, clamped to
    /// `0..=max` from the last render.
    fn scroll_pane(&mut self, comments: bool, delta: i32) {
        let (s, max) = if comments {
            (&mut self.comment_scroll, self.comment_scroll_max.get())
        } else {
            (&mut self.detail_scroll, self.detail_scroll_max.get())
        };
        *s = (i32::from(*s).saturating_add(delta)).clamp(0, i32::from(max)) as u16;
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
                if self.apply_transition(t) {
                    self.return_to_list();
                }
            }
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.return_to_list(),
            KeyCode::Char('j') | KeyCode::Down => {
                self.picker_sel = Self::move_sel(self.transitions.len(), self.picker_sel, 1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_sel = Self::move_sel(self.transitions.len(), self.picker_sel, -1)
            }
            KeyCode::Enter => {
                if let Some(t) = self.transitions.get(self.picker_sel).cloned() {
                    if self.apply_transition(t) {
                        self.return_to_list();
                    }
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
            self.delegate_to(agent);
            if self.handoff.is_some() {
                self.view = View::List;
            }
        }
    }

    fn keys_agent_picker(&mut self, key: KeyEvent) {
        let len = self.agent_picker_len();
        if let Some(idx) = Self::picker_number(&key, len) {
            self.pick_agent_row(idx);
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.return_to_list(),
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
        self.start_new_and_delegate(cwd);
        if self.handoff.is_some() {
            self.view = View::List;
        }
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
                self.start_new_and_delegate(cwd);
                if self.handoff.is_some() {
                    self.view = View::List;
                }
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
            KeyCode::Esc | KeyCode::Char('q') => self.return_to_list(),
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
                    self.return_to_list();
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
        let Some(issue) = self.wizard_issue.as_ref() else {
            return;
        };
        let opts = worktree_opts(&self.cfg, issue, &self.wt_branch);
        match spawn {
            None => {
                self.return_to_list();
                self.open_worktree_only(opts);
            }
            Some(spawn) => {
                self.start_worktree_agent(spawn, opts);
                if self.handoff.is_some() {
                    self.view = View::List;
                }
            }
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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::{Duration, Instant};

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
    fn link_only_template_needs_no_jira_client() {
        let cfg = test_cfg("{key}: {url} {snapshot_at}", 10);
        let p = prepare_prompt(&cfg, None, "PROJ-7").unwrap();
        assert!(p
            .text
            .contains("PROJ-7: https://x.atlassian.net/browse/PROJ-7"));
        assert!(p.warnings.is_empty());
        assert!(!p.text.contains("token"));
    }
    fn test_agent() -> HerdrAgent {
        HerdrAgent {
            target: "pane-1".into(),
            label: "agent".into(),
            status: "idle".into(),
            cwd: String::new(),
            pane_id: "pane-1".into(),
        }
    }

    fn pending_handoff(app: &mut App) -> u64 {
        let token = app.next_token();
        app.handoff = Some(HandoffOperation {
            token,
            key: "PROJ-7".into(),
            destination: HandoffDestination::Existing(test_agent()),
            submit: true,
            startup: 0,
            wait_ready: 100,
        });
        app.handoff_loading = true;
        token
    }

    #[test]
    fn partial_preparation_requires_confirmation_and_escape_invalidates_it() {
        let mut app = list_app();
        let token = pending_handoff(&mut app);
        app.wizard_issue = Some(test_issue());
        app.delegate_to(test_agent());
        assert_eq!(app.handoff.as_ref().unwrap().token, token);
        let mut another = test_issue();
        another.key = "PROJ-8".into();
        app.issues.push(another);
        app.selected = 1;
        app.on_resp(Resp::HandoffPrepared {
            token,
            result: Ok(PromptPreview {
                key: "PROJ-7".into(),
                text: "bounded snapshot".into(),
                warnings: vec!["Comments capped".into()],
            }),
        });
        assert_eq!(app.view, View::HandoffConfirm);
        assert_eq!(app.handoff_preview.as_ref().unwrap().key, "PROJ-7");
        assert!(!app.handoff_loading);
        assert_eq!(
            app.handoff_preview.as_ref().unwrap().text,
            "bounded snapshot"
        );
        press(&mut app, KeyCode::Esc);
        assert!(app.handoff.is_none());
        assert!(app.handoff_preview.is_none());
        app.on_resp(Resp::HandoffPrepared {
            token,
            result: Err("late error".into()),
        });
        assert!(app.handoff_error.is_none());
        let next = pending_handoff(&mut app);
        app.on_resp(Resp::HandoffPrepared {
            token: next,
            result: Ok(PromptPreview {
                key: "PROJ-7".into(),
                text: "confirmed bounded snapshot".into(),
                warnings: vec!["Description truncated".into()],
            }),
        });
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.view, View::List);
        assert!(app.handoff_loading);
        assert_eq!(app.handoff.as_ref().unwrap().token, next);
        assert_eq!(
            app.handoff_preview.as_ref().unwrap().text,
            "confirmed bounded snapshot"
        );
    }

    #[test]
    fn failed_fresh_read_cannot_dispatch_and_only_known_unsent_panes_can_retry() {
        let mut app = list_app();
        let token = pending_handoff(&mut app);
        app.on_resp(Resp::HandoffPrepared {
            token,
            result: Err("HTTP 401".into()),
        });
        assert!(app.handoff.is_none());
        assert!(app.handoff_error.as_ref().unwrap().contains("HTTP 401"));
        let token = pending_handoff(&mut app);
        app.handoff_preview = Some(PromptPreview {
            key: "PROJ-7".into(),
            text: "frozen".into(),
            warnings: vec![],
        });
        app.on_resp(Resp::Delegated {
            token,
            key: "PROJ-7".into(),
            label: "agent".into(),
            submitted: true,
            result: Err(herdr::DelegationFailure {
                message: "not ready".into(),
                agent: Some(test_agent()),
                retry_safe: true,
            }),
        });
        assert_eq!(app.handoff_recovery_agent().unwrap().target, "pane-1");
        assert_eq!(app.recovery.as_ref().unwrap().preview.text, "frozen");
        press(&mut app, KeyCode::Char('D'));
        let token = app.handoff.as_ref().unwrap().token;
        assert!(app.handoff_loading);
        assert_eq!(app.handoff.as_ref().unwrap().key, "PROJ-7");
        assert_eq!(app.handoff_preview.as_ref().unwrap().text, "frozen");
        app.on_resp(Resp::Delegated {
            token,
            key: "PROJ-7".into(),
            label: "agent".into(),
            submitted: true,
            result: Err(herdr::DelegationFailure {
                message: "send failed".into(),
                agent: Some(test_agent()),
                retry_safe: false,
            }),
        });
        assert!(app.recovery.is_none());
        assert!(app
            .handoff_error
            .as_ref()
            .unwrap()
            .contains("delivery unknown"));
    }

    #[test]
    fn recovery_survives_reload_and_retries_only_known_unsent_same_pane() {
        const CHILD: &str = "JIRA_APP_RECOVERY_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let dir = std::env::temp_dir().join(format!(
                "jira-recovery-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&dir).unwrap();
            std::fs::write(
                dir.join("config.toml"),
                "[jira]\nbase_url = \"http://127.0.0.1:1\"\nauth = \"bearer\"\napi_token = \"isolated\"\n",
            ).unwrap();
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "app::tests::recovery_survives_reload_and_retries_only_known_unsent_same_pane",
                ])
                .env(CHILD, "1")
                .env("HERDR_PLUGIN_CONFIG_DIR", &dir)
                .output()
                .unwrap();
            std::fs::remove_file(dir.join("config.toml")).unwrap();
            std::fs::remove_dir(dir).unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stdout)
            );
            return;
        }

        let mut app = list_app();
        let token = pending_handoff(&mut app);
        app.handoff_preview = Some(PromptPreview {
            key: "PROJ-7".into(),
            text: "original exact prompt".into(),
            warnings: vec![],
        });
        app.on_resp(Resp::Delegated {
            token,
            key: "PROJ-7".into(),
            label: "agent".into(),
            submitted: true,
            result: Err(herdr::DelegationFailure {
                message: "pane not ready".into(),
                agent: Some(test_agent()),
                retry_safe: true,
            }),
        });
        assert!(screen(&app).contains("D recheck and send"));
        assert!(screen(&app).contains("pane pane-1"));
        let obsolete = pending_handoff(&mut app); // a prepared read, not a dispatched write
        assert!(app.reload_config());
        assert!(app.handoff.is_none());
        assert_eq!(
            app.recovery.as_ref().unwrap().preview.text,
            "original exact prompt"
        );
        app.on_resp(Resp::HandoffPrepared {
            token: obsolete,
            result: Err("obsolete read".into()),
        });
        assert!(app
            .handoff_error
            .as_ref()
            .unwrap()
            .contains("pane not ready"));
        app.issues = vec![named_issue("PROJ-8")];
        press(&mut app, KeyCode::Char('D'));
        assert!(app.handoff_loading);
        let operation = app.handoff.as_ref().unwrap();
        assert_eq!(operation.key, "PROJ-7");
        assert!(operation.submit);
        assert_eq!(operation.wait_ready, 100);
        let HandoffDestination::Existing(agent) = &operation.destination else {
            panic!("retry must target the existing pane");
        };
        assert_eq!(agent.target, "pane-1");
        assert_eq!(
            app.handoff_preview.as_ref().unwrap().text,
            "original exact prompt"
        );
        let retry_token = operation.token;
        assert!(!app.reload_config()); // no cancellation of an already-dispatched retry
        assert!(app.handoff.is_some());
        app.on_resp(Resp::Delegated {
            token: retry_token,
            key: "PROJ-7".into(),
            label: "agent".into(),
            submitted: true,
            result: Err(herdr::DelegationFailure {
                message: "pane still not ready".into(),
                agent: Some(test_agent()),
                retry_safe: true,
            }),
        });
        assert_eq!(app.recovery.as_ref().unwrap().agent.target, "pane-1");
        assert_eq!(
            app.recovery.as_ref().unwrap().preview.text,
            "original exact prompt"
        );
        press(&mut app, KeyCode::Char('D'));
        let retry_token = app.handoff.as_ref().unwrap().token;
        app.on_resp(Resp::Delegated {
            token: retry_token,
            key: "PROJ-7".into(),
            label: "agent".into(),
            submitted: true,
            result: Err(herdr::DelegationFailure {
                message: "uncertain send".into(),
                agent: Some(test_agent()),
                retry_safe: false,
            }),
        });
        assert!(app.recovery.is_none());
        press(&mut app, KeyCode::Char('D'));
        assert!(app.handoff.is_none());
        assert!(!screen(&app).contains("D recheck and send"));
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
    fn named_issue(key: &str) -> Issue {
        let mut issue = test_issue();
        issue.key = key.into();
        issue
    }

    fn screen(app: &App) -> String {
        use ratatui::backend::TestBackend;
        let mut terminal = ratatui::Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(buffer.area.width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn wizard_headings_follow_captured_issue_across_list_replacement() {
        let mut app = list_app();
        app.issues.push(named_issue("PROJ-8"));
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.wizard_issue_key(), Some("PROJ-7"));
        app.issues = vec![named_issue("PROJ-8")];
        app.selected = 0;
        for (view, heading) in [
            (View::AgentPicker, "delegate PROJ-7 to agent"),
            (View::NewAgentTypePicker, "start agent for PROJ-7"),
            (View::WorktreeProjectPicker, "worktree for PROJ-7 — project"),
            (View::WorktreeRepoInput, "worktree for PROJ-7 — repo path"),
            (View::WorktreeListPicker, "worktree for PROJ-7 —"),
            (View::WorktreeNameInput, "new worktree for PROJ-7"),
            (
                View::WorktreeAgentPicker,
                "worktree for PROJ-7 — start an agent",
            ),
        ] {
            app.view = view;
            let rendered = screen(&app);
            assert!(rendered.contains(heading), "{view:?}: {rendered}");
            assert!(!rendered.contains("worktree for PROJ-8"), "{rendered}");
        }
        app.view = View::WorktreeAgentPicker;
        app.wt_repo = temp_dir();
        app.wt_branch = "PROJ-7".into();
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(app.handoff.as_ref().unwrap().key, "PROJ-7");
        let HandoffDestination::New(opts) = &app.handoff.as_ref().unwrap().destination else {
            panic!("worktree agent must start a new pane");
        };
        assert_eq!(opts.tab_label, "PROJ-7");
    }

    #[test]
    fn detail_status_exit_unpins_before_next_delegate_and_worktree() {
        let mut app = list_app();
        app.issues.push(named_issue("PROJ-8"));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.detail_key.as_deref(), Some("PROJ-7"));
        app.view = View::TransitionPicker;
        press(&mut app, KeyCode::Esc);
        assert!(app.detail_key.is_none());
        assert!(app.detail_issue.is_none());
        app.selected = 1;
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.wizard_issue_key(), Some("PROJ-8"));
        assert!(screen(&app).contains("delegate PROJ-8 to agent"));
        press(&mut app, KeyCode::Esc);
        assert!(app.wizard_issue_key().is_none());
        press(&mut app, KeyCode::Char('w'));
        assert_eq!(app.wizard_issue_key(), Some("PROJ-8"));
        assert!(screen(&app).contains("worktree for PROJ-8"));
        assert_eq!(app.wt_default_branch, "PROJ-8");
    }

    #[test]
    fn worktree_picker_and_operation_keep_its_original_issue_after_page_change() {
        let mut app = list_app();
        press(&mut app, KeyCode::Char('w'));
        assert_eq!(app.view, View::WorktreeProjectPicker);
        app.issues = vec![named_issue("PROJ-8")];
        assert!(screen(&app).contains("worktree for PROJ-7 — project"));
        app.on_resp(Resp::Projects(Ok(vec![project("checkout", true)])));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.view, View::WorktreeListPicker);
        assert!(screen(&app).contains("worktree for PROJ-7"));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.wt_name_input, "PROJ-7");
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('2'));
        let op = app.handoff.as_ref().unwrap();
        assert_eq!(op.key, "PROJ-7");
        let HandoffDestination::New(opts) = &op.destination else {
            panic!("worktree agent must use the frozen issue");
        };
        assert_eq!(opts.tab_label, "PROJ-7");
        assert_eq!(opts.worktree.as_ref().unwrap().branch, "PROJ-7");
    }

    #[test]
    fn detail_origin_survives_transient_list_and_confirmation() {
        let mut app = list_app();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('d'));
        app.view = View::List; // selecting an agent temporarily shows the list during prep
        let token = pending_handoff(&mut app);
        app.on_resp(Resp::HandoffPrepared {
            token,
            result: Ok(PromptPreview {
                key: "PROJ-7".into(),
                text: "review first".into(),
                warnings: vec!["partial snapshot".into()],
            }),
        });
        assert_eq!(app.view, View::HandoffConfirm);
        assert_eq!(app.base_view(), View::Detail);
        assert_eq!(app.detail_key.as_deref(), Some("PROJ-7"));
        assert_eq!(app.wizard_issue_key(), Some("PROJ-7"));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.view, View::Detail);
        assert_eq!(app.detail_key.as_deref(), Some("PROJ-7"));
        app.on_resp(Resp::Delegated {
            token,
            key: "PROJ-7".into(),
            label: "agent".into(),
            submitted: true,
            result: Err(herdr::DelegationFailure {
                message: "send status unknown".into(),
                agent: Some(test_agent()),
                retry_safe: false,
            }),
        });
        assert_eq!(app.view, View::Detail);
        assert!(app.wizard_issue_key().is_none());
        assert!(app.recovery.is_none());
    }

    #[test]
    fn detail_origin_worktree_confirmation_restores_detail_after_dispatch() {
        let mut app = list_app();
        app.cfg.worktree.repo = temp_dir();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('w'));
        assert_eq!(app.view, View::WorktreeListPicker);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('2'));
        assert_eq!(app.view, View::List);
        assert_eq!(app.help_return, View::Detail);
        assert_eq!(app.detail_key.as_deref(), Some("PROJ-7"));
        let token = app.handoff.as_ref().unwrap().token;
        app.on_resp(Resp::HandoffPrepared {
            token,
            result: Ok(PromptPreview {
                key: "PROJ-7".into(),
                text: "worktree prompt".into(),
                warnings: vec!["snapshot incomplete".into()],
            }),
        });
        assert_eq!(app.base_view(), View::Detail);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.view, View::Detail);
        assert_eq!(app.detail_key.as_deref(), Some("PROJ-7"));
    }

    fn page<T>(items: Vec<T>, total: Option<usize>, has_more: bool) -> Page<T> {
        Page {
            items,
            total,
            has_more,
        }
    }

    #[test]
    fn reverse_order_queries_keep_active_title_error_and_cached_rows() {
        let mut app = list_app();
        app.run_search("login".into());
        let old = app.query_generation;
        app.spawn_search(
            "project = PROJ ORDER BY updated DESC".into(),
            "custom JQL".into(),
        );
        let current = app.query_generation;
        app.on_resp(Resp::Issues {
            generation: old,
            result: Err("stale unauthorized".into()),
        });
        assert!(app.list_error.is_none());
        assert_eq!(app.current_title, "custom JQL");
        app.on_resp(Resp::Issues {
            generation: current,
            result: Err("401 unauthorized".into()),
        });
        assert_eq!(app.issues[0].key, "PROJ-7");
        assert_eq!(app.list_error.as_deref(), Some("401 unauthorized"));
        app.toast = None; // a toast expiring cannot erase the persistent failure
        assert!(app.list_error.is_some());
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(app.last_jql, "project = PROJ ORDER BY updated DESC");
        assert_eq!(app.current_title, "custom JQL");
        app.on_resp(Resp::Issues {
            generation: current + 1,
            result: Ok(page(vec![], Some(0), false)),
        });
        assert!(app.issues.is_empty());
        assert!(app.list_error.is_none());
    }

    #[test]
    fn query_refresh_preserves_key_then_clamps_when_removed() {
        let mut app = list_app();
        app.issues = vec![
            named_issue("PROJ-1"),
            named_issue("PROJ-2"),
            named_issue("PROJ-3"),
        ];
        app.selected = 1;
        app.spawn_search("text ~ \"login\"".into(), "search: login".into());
        let generation = app.query_generation;
        app.on_resp(Resp::Issues {
            generation,
            result: Ok(page(
                vec![named_issue("PROJ-3"), named_issue("PROJ-2")],
                Some(73),
                true,
            )),
        });
        assert_eq!(app.selected_issue().unwrap().key, "PROJ-2");
        assert_eq!(app.issues_total, Some(73));
        assert!(app.issues_has_more);
        app.pending_transition = Some(PendingTransition {
            token: 42,
            key: "PROJ-2".into(),
        });
        app.on_resp(Resp::Transitioned {
            key: "PROJ-2".into(),
            name: "In Progress".into(),
            token: 42,
            result: Ok(()),
        });
        assert_eq!(app.last_jql, "text ~ \"login\"");
        assert_eq!(app.current_title, "search: login");
        app.on_resp(Resp::Issues {
            generation: app.query_generation,
            result: Ok(page(vec![named_issue("PROJ-3")], Some(1), false)),
        });
        assert_eq!(app.selected_issue().unwrap().key, "PROJ-3");
        assert!(!app.issues_has_more);
    }

    #[test]
    fn late_children_are_ignored_after_collapse_query_and_reload_invalidation() {
        let mut app = list_app();
        app.issues[0].issue_type = "Epic".into();
        app.expanded.insert("PROJ-7".into());
        app.child_requests.insert("PROJ-7".into(), 1);
        app.loading_children.insert("PROJ-7".into());
        app.collapse_epic();
        app.on_resp(Resp::Children {
            epic: "PROJ-7".into(),
            token: 1,
            result: Ok(page(vec![named_issue("PROJ-8")], Some(1), false)),
        });
        assert!(app.children.is_empty());
        app.expanded.insert("PROJ-7".into());
        app.child_requests.insert("PROJ-7".into(), 2);
        app.spawn_search("new".into(), "new query".into());
        app.on_resp(Resp::Children {
            epic: "PROJ-7".into(),
            token: 2,
            result: Ok(page(vec![named_issue("PROJ-8")], Some(1), false)),
        });
        assert!(app.children.is_empty());
        let generation = app.query_generation;
        app.invalidate_requests(); // same invalidation reload_config uses, without reading environment
        app.on_resp(Resp::Issues {
            generation,
            result: Ok(page(vec![], Some(0), false)),
        });
        assert_eq!(app.issues.len(), 1);
    }

    #[test]
    fn detail_and_comments_pin_key_and_recover_after_error() {
        let mut app = list_app();
        app.issues.push(named_issue("PROJ-8"));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.detail_key.as_deref(), Some("PROJ-7"));
        app.issues.swap(0, 1);
        assert_eq!(app.selected_issue().unwrap().key, "PROJ-7");
        app.detail_request = Some(2);
        app.detail_loading = true;
        app.comment_requests.insert("PROJ-7".into(), 3);
        app.comments_loading.insert("PROJ-7".into());
        app.on_resp(Resp::Detail {
            key: "PROJ-8".into(),
            token: 2,
            result: Ok(named_issue("PROJ-8")),
        });
        assert!(app.detail_loading);
        app.on_resp(Resp::Detail {
            key: "PROJ-7".into(),
            token: 2,
            result: Err("forbidden".into()),
        });
        assert_eq!(app.selected_issue().unwrap().key, "PROJ-7");
        assert_eq!(app.detail_error.as_deref(), Some("forbidden"));
        app.on_resp(Resp::Comments {
            key: "PROJ-7".into(),
            token: 3,
            result: Err("timeout".into()),
        });
        assert_eq!(app.comments_err["PROJ-7"], "timeout");
        app.detail_request = Some(4);
        app.comment_requests.insert("PROJ-7".into(), 5);
        let mut fresh = named_issue("PROJ-7");
        fresh.description = "current description".into();
        app.on_resp(Resp::Detail {
            key: "PROJ-7".into(),
            token: 2,
            result: Ok(named_issue("PROJ-7")),
        });
        app.on_resp(Resp::Detail {
            key: "PROJ-7".into(),
            token: 4,
            result: Ok(fresh),
        });
        app.on_resp(Resp::Comments {
            key: "PROJ-7".into(),
            token: 5,
            result: Ok(page(vec![], Some(0), false)),
        });
        assert_eq!(
            app.selected_issue().unwrap().description,
            "current description"
        );
        assert!(app.detail_error.is_none());
        assert!(app.comments_err.is_empty());
    }

    #[test]
    fn read_invalidation_rejects_stale_gets_but_preserves_dispatched_post() {
        let mut app = list_app();
        app.spawn_search("project = PROJ".into(), "Project".into());
        press(&mut app, KeyCode::Enter);
        app.detail_request = Some(10);
        app.comment_requests.insert("PROJ-7".into(), 11);
        app.transitions_request = Some(12);
        app.pending_transition = Some(PendingTransition {
            token: 13,
            key: "PROJ-7".into(),
        });
        app.transitions_for = "PROJ-7".into();
        let prior_generation = app.query_generation;
        press(&mut app, KeyCode::Char('r'));
        assert_eq!(app.last_jql, "project = PROJ");
        app.on_resp(Resp::Issues {
            generation: prior_generation,
            result: Err("stale query failed".into()),
        });
        assert!(app.list_error.is_none());
        app.detail_request = Some(20);
        app.comment_requests.insert("PROJ-7".into(), 21);
        app.invalidate_requests();
        app.on_resp(Resp::Detail {
            key: "PROJ-7".into(),
            token: 20,
            result: Ok(named_issue("PROJ-7")),
        });
        app.on_resp(Resp::Comments {
            key: "PROJ-7".into(),
            token: 21,
            result: Ok(page(vec![], Some(0), false)),
        });
        app.on_resp(Resp::Transitions {
            key: "PROJ-7".into(),
            token: 12,
            result: Ok(vec![]),
        });
        app.on_resp(Resp::Transitioned {
            key: "PROJ-7".into(),
            name: "Done".into(),
            token: 13,
            result: Ok(()),
        });
        assert!(app.comments.is_empty());
        assert!(app
            .detail_issue
            .as_ref()
            .is_some_and(|i| i.description == "Steps to reproduce…"));
        assert_eq!(app.current_title, "Project");
        assert!(app.pending_transition.is_none());
    }

    #[test]
    fn transition_reply_owns_only_its_post_while_new_picker_and_query_remain_current() {
        let mut app = list_app();
        let mut cfg = test_cfg("{key}", 0);
        cfg.jira.base_url = "http://127.0.0.1:1".into();
        cfg.jira.auth = "bearer".into();
        cfg.jira.api_token = "local-only".into();
        app.client = Some(Arc::new(JiraClient::new(&cfg).unwrap()));
        app.issues.push(named_issue("PROJ-8"));
        app.spawn_search("project = OLD".into(), "old query".into());
        let write_token = app.next_token();
        app.pending_transition = Some(PendingTransition {
            token: write_token,
            key: "PROJ-7".into(),
        });
        app.selected = 1;
        app.request_transitions();
        let stale_get = app.transitions_request.unwrap();
        app.spawn_search("project = NEW".into(), "current query".into());
        app.invalidate_requests(); // a read invalidation cannot revoke a dispatched POST
        app.request_transitions();
        let current_get = app.transitions_request.unwrap();
        app.picker_sel = 1;
        app.on_resp(Resp::Transitioned {
            token: write_token,
            key: "PROJ-8".into(),
            name: "wrong issue".into(),
            result: Ok(()),
        });
        assert!(app.pending_transition.is_some());
        app.on_resp(Resp::Transitioned {
            token: write_token,
            key: "PROJ-7".into(),
            name: "Done".into(),
            result: Ok(()),
        });
        assert!(app.pending_transition.is_none());
        assert_eq!(app.last_jql, "project = NEW");
        assert_eq!(app.current_title, "current query");
        assert_eq!(app.view, View::TransitionPicker);
        assert_eq!(app.transitions_for, "PROJ-8");
        assert_eq!(app.transitions_request, Some(current_get));
        assert_eq!(app.picker_sel, 1);
        app.on_resp(Resp::Transitions {
            token: stale_get,
            key: "PROJ-8".into(),
            result: Ok(vec![]),
        });
        assert!(app.transitions_loading);
        app.on_resp(Resp::Transitions {
            token: current_get,
            key: "PROJ-8".into(),
            result: Ok(vec![Transition {
                id: "4".into(),
                name: "Start".into(),
                to_status: "Doing".into(),
            }]),
        });
        assert_eq!(app.transitions[0].name, "Start");
        assert_eq!(app.view, View::TransitionPicker);
    }

    #[test]
    fn transition_error_and_second_apply_keep_b_picker_without_overwriting_writer() {
        let mut app = list_app();
        let mut cfg = test_cfg("{key}", 0);
        cfg.jira.base_url = "http://127.0.0.1:1".into();
        cfg.jira.auth = "bearer".into();
        cfg.jira.api_token = "local-only".into();
        app.client = Some(Arc::new(JiraClient::new(&cfg).unwrap()));
        app.pending_transition = Some(PendingTransition {
            token: 91,
            key: "PROJ-7".into(),
        });
        app.transitions_for = "PROJ-8".into();
        app.transitions = vec![Transition {
            id: "4".into(),
            name: "Start".into(),
            to_status: "Doing".into(),
        }];
        app.view = View::TransitionPicker;
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.view, View::TransitionPicker);
        assert_eq!(app.pending_transition.as_ref().unwrap().key, "PROJ-7");
        assert!(matches!(&app.toast, Some((_, true, _))));
        assert!(!app.reload_config());
        assert_eq!(app.pending_transition.as_ref().unwrap().key, "PROJ-7");
        assert_eq!(app.view, View::TransitionPicker);
        app.view = View::List;
        press(&mut app, KeyCode::Char('R'));
        assert!(app.toast.as_ref().is_some_and(|(_, error, _)| *error));
        assert_eq!(app.pending_transition.as_ref().unwrap().key, "PROJ-7");
        app.view = View::TransitionPicker;
        app.on_resp(Resp::Transitioned {
            token: 91,
            key: "PROJ-7".into(),
            name: "Done".into(),
            result: Err("HTTP 409".into()),
        });
        assert!(app.pending_transition.is_none());
        assert_eq!(app.view, View::TransitionPicker);
        assert!(app
            .toast
            .as_ref()
            .is_some_and(|(message, error, _)| *error && message.contains("HTTP 409")));
        assert_eq!(app.transitions[0].name, "Start");
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.pending_transition.as_ref().unwrap().key, "PROJ-8");
        assert_eq!(app.view, View::List);
    }

    #[test]
    fn expanded_children_keep_selected_sibling_key_and_bound_metadata() {
        let mut app = list_app();
        app.issues[0].issue_type = "Epic".into();
        app.issues.push(named_issue("PROJ-9"));
        app.selected = 1;
        app.expanded.insert("PROJ-7".into());
        app.child_requests.insert("PROJ-7".into(), 12);
        app.on_resp(Resp::Children {
            epic: "PROJ-7".into(),
            token: 12,
            result: Ok(page(vec![named_issue("PROJ-8")], Some(4), true)),
        });
        assert_eq!(app.selected_issue().unwrap().key, "PROJ-9");
        assert_eq!(app.selected, 2);
        assert_eq!(app.children_total["PROJ-7"], 4);
        assert!(app.children_has_more.contains("PROJ-7"));
    }

    /// Reply to the parent query and observe any subsequent fallback query.
    fn epic_fixture(
        replies: Vec<Option<(u16, &'static str)>>,
    ) -> (
        App,
        thread::JoinHandle<Vec<String>>,
        std::sync::mpsc::Receiver<Resp>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let mut targets = Vec::new();
            let mut deadline = Instant::now() + Duration::from_secs(3);
            'requests: while targets.len() <= replies.len() {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(_) => break,
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                let mut bytes = [0u8; 4096];
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let count = match stream.read(&mut bytes) {
                        Ok(0) if request.is_empty() => continue 'requests,
                        Ok(count) => count,
                        Err(error)
                            if request.is_empty()
                                && matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                                ) =>
                        {
                            continue 'requests;
                        }
                        Err(error) => panic!("incomplete fixture request: {error}"),
                    };
                    assert!(count > 0 && request.len() + count < 16_384);
                    request.extend_from_slice(&bytes[..count]);
                }
                let target = String::from_utf8(request)
                    .unwrap()
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_owned();
                targets.push(target);
                let reply = replies
                    .get(targets.len() - 1)
                    .copied()
                    .flatten()
                    .or_else(|| {
                        (targets.len() > replies.len())
                            .then_some((200, r#"{"issues":[],"total":0,"startAt":0}"#))
                    });
                if let Some((code, body)) = reply {
                    let response = format!(
                        "HTTP/1.1 {code} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).unwrap();
                }
                deadline = Instant::now() + Duration::from_millis(350);
            }
            targets
        });
        let mut cfg = test_cfg("{key}", 0);
        cfg.jira.base_url = base;
        cfg.jira.auth = "bearer".into();
        cfg.jira.api_token = "test".into();
        let client = JiraClient::new(&cfg).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut app = App::unloaded(tx);
        app.client = Some(Arc::new(client));
        app.issues = vec![test_issue()];
        app.issues[0].issue_type = "Epic".into();
        (app, server, rx)
    }
    #[test]
    fn snapshot_reads_fresh_fields_and_bounds_comments_without_replacing_jira_placeholders() {
        let (app, server, _) = epic_fixture(vec![
            Some((
                200,
                r#"{"key":"PROJ-7","fields":{"summary":"Fresh {status}","status":{"name":"In Progress"},"updated":"2026-10-11T12:30:00.000+0000","description":"abcdef"}}"#,
            )),
            Some((
                200,
                r#"{"startAt":0,"maxResults":1,"total":3,"comments":[{"id":"app-snapshot-1","author":{"displayName":"Ada"},"created":"2026-10-10T09:30:00.000+0000","body":"secret {key} content"}]}"#,
            )),
        ]);
        let mut cfg = test_cfg(
            "{key} {summary} {status} {updated} {description} {comments} {snapshot_at}",
            3,
        );
        cfg.delegate.max_comments = 1;
        cfg.delegate.max_comment_chars = 12;
        let preview = prepare_prompt(&cfg, app.client.as_deref(), "PROJ-7").unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("/issue/PROJ-7?"));
        assert!(requests[1].contains("maxResults=1"));
        assert!(preview
            .text
            .contains("Fresh {status} In Progress 2026-10-11 12:30 abc"));
        assert!(!preview.text.contains("secret {key} content"));
        assert_eq!(preview.warnings.len(), 3);
        assert!(!preview.text.contains("Bearer test"));
        assert!(!preview.text.contains("Authorization"));
    }
    #[test]
    fn zero_budgets_omit_content_and_warn_instead_of_becoming_unlimited() {
        let (app, server, _) = epic_fixture(vec![
            Some((
                200,
                r#"{"key":"PROJ-7","fields":{"description":"secret description"}}"#,
            )),
            Some((
                200,
                r#"{"startAt":0,"maxResults":1,"total":1,"comments":[{"id":"app-zero-1","author":{"displayName":"Ada"},"created":"2026-10-10T09:30:00.000+0000","body":"secret comment"}]}"#,
            )),
        ]);
        let mut cfg = test_cfg("{description} {comments}", 0);
        cfg.delegate.max_comments = 0;
        cfg.delegate.max_comment_chars = 0;
        let preview = prepare_prompt(&cfg, app.client.as_deref(), "PROJ-7").unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].contains("maxResults=1"));
        assert_eq!(preview.warnings.len(), 3);
        assert!(!preview.text.contains("secret"));
    }

    fn expand_from_fixture(status: Option<(u16, &'static str)>) -> (App, Vec<String>) {
        let (mut app, server, rx) = epic_fixture(vec![status]);
        app.expand_epic();
        let response = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        app.on_resp(response);
        (app, server.join().unwrap())
    }

    #[test]
    fn epic_auth_and_transport_failures_remain_visible_without_alternate_search() {
        for (status, expected) in [
            (
                Some((401, r#"{"errorMessages":["invalid token"]}"#)),
                "HTTP 401: invalid token",
            ),
            (None, "request failed:"),
            (
                Some((
                    400,
                    r#"{"errorMessages":["Invalid JQL syntax near parent"]}"#,
                )),
                "HTTP 400:",
            ),
            (
                Some((
                    400,
                    r#"{"errorMessages":["Field 'parent' does not exist or you do not have permission"]}"#,
                )),
                "HTTP 400:",
            ),
        ] {
            let (app, requests) = expand_from_fixture(status);
            assert_eq!(
                requests.len(),
                1,
                "unexpected Epic Link retry: {requests:?}"
            );
            assert!(requests[0].contains("parent"));
            assert!(app.children.is_empty());
            assert!(app.expanded.contains("PROJ-7"));
            assert!(!app.loading_children.contains("PROJ-7"));
            assert!(app.children_err["PROJ-7"].contains(expected));
            assert!(app
                .toast
                .as_ref()
                .is_some_and(|(message, error, _)| *error && message.contains(expected)));
        }
    }

    #[test]
    fn epic_link_fallback_only_for_unsupported_parent_field() {
        let (mut app, server, rx) = epic_fixture(vec![
            Some((404, r#"{"errorMessages":["Cloud endpoint unavailable"]}"#)),
            Some((
                400,
                r#"{"errorMessages":["Field 'parent' does not exist"]}"#,
            )),
            Some((404, r#"{"errorMessages":["Cloud endpoint unavailable"]}"#)),
            Some((200, r#"{"issues":[],"total":0,"startAt":0}"#)),
        ]);
        app.expand_epic();
        app.on_resp(rx.recv_timeout(Duration::from_secs(3)).unwrap());
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 4);
        assert!(requests[..2]
            .iter()
            .all(|request| request.contains("parent")));
        assert!(requests[2..]
            .iter()
            .all(|request| request.contains("Epic") && request.contains("Link")));
        assert!(requests[1].starts_with("/rest/api/2/search?"));
        assert!(requests[3].starts_with("/rest/api/2/search?"));
        assert_eq!(app.children["PROJ-7"].len(), 0);
        assert!(!app.children_err.contains_key("PROJ-7"));
    }

    #[test]
    fn epic_later_page_error_does_not_trigger_field_fallback() {
        let (mut app, server, rx) = epic_fixture(vec![
            Some((
                200,
                r#"{"issues":[{"key":"PROJ-8","fields":{}}],"nextPageToken":"second","isLast":false}"#,
            )),
            Some((
                400,
                r#"{"errorMessages":["Field 'parent' does not exist"]}"#,
            )),
        ]);
        app.expand_epic();
        app.on_resp(rx.recv_timeout(Duration::from_secs(3)).unwrap());
        let requests = server.join().unwrap();
        assert_eq!(
            requests.len(),
            2,
            "unexpected Epic Link retry: {requests:?}"
        );
        assert!(requests[1].contains("nextPageToken"));
        assert!(app.children.is_empty());
        assert!(app.children_err["PROJ-7"].starts_with("Jira search later page: HTTP 400:"));
    }
}

/// Render only fields required by this template, from fresh Jira reads.
fn prepare_prompt(
    cfg: &Config,
    client: Option<&JiraClient>,
    key: &str,
) -> Result<PromptPreview, String> {
    let template = &cfg.delegate.prompt;
    let needs_comments = template.contains("{comments}");
    let needs_issue = needs_comments
        || [
            "{summary}",
            "{description}",
            "{status}",
            "{assignee}",
            "{reporter}",
            "{priority}",
            "{type}",
            "{labels}",
            "{updated}",
        ]
        .iter()
        .any(|field| template.contains(field));
    let issue = if needs_issue {
        Some(client.ok_or("Jira client unavailable")?.issue(key)?)
    } else {
        None
    };
    let mut warnings = Vec::new();
    let description = if template.contains("{description}") {
        let raw = issue.as_ref().unwrap().description.trim();
        let max = cfg.delegate.max_description_chars;
        if max == 0 {
            warnings.push("Description omitted: max_description_chars is zero".into());
            "(description omitted)".to_string()
        } else if raw.is_empty() {
            "(no description)".to_string()
        } else if raw.chars().count() > max {
            warnings.push(format!("Description truncated to {max} characters"));
            format!(
                "{}\n[… description truncated]",
                raw.chars().take(max).collect::<String>()
            )
        } else {
            raw.to_string()
        }
    } else {
        String::new()
    };
    let comments = if needs_comments {
        let limit = if cfg.delegate.max_comment_chars == 0 {
            0
        } else {
            cfg.delegate.max_comments
        };
        let page = client.unwrap().comments_with_limit(key, limit)?;
        if cfg.delegate.max_comments == 0 || cfg.delegate.max_comment_chars == 0 {
            if cfg.delegate.max_comments == 0 {
                warnings.push("Comments omitted: max_comments is zero".into());
            }
            if cfg.delegate.max_comment_chars == 0 {
                warnings.push("Comments omitted: max_comment_chars is zero".into());
            }
            "(comments omitted)".to_string()
        } else {
            if page.has_more || page.total.is_some_and(|n| n > page.items.len()) {
                warnings.push(format!("Comments capped at {}", cfg.delegate.max_comments));
            }
            let mut text = String::new();
            for comment in &page.items {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(&format!(
                    "{} ({})\n{}",
                    comment.author, comment.created, comment.body
                ));
            }
            let max = cfg.delegate.max_comment_chars;
            if text.chars().count() > max {
                warnings.push(format!("Comments truncated to {max} characters"));
                format!(
                    "{}\n[… comments truncated]",
                    text.chars().take(max).collect::<String>()
                )
            } else if text.is_empty() {
                "(no comments)".to_string()
            } else {
                text
            }
        }
    } else {
        String::new()
    };
    let labels = issue
        .as_ref()
        .map(|i| i.labels.join(", "))
        .unwrap_or_default();
    let url = issue
        .as_ref()
        .map(|i| i.url.clone())
        .unwrap_or_else(|| format!("{}/browse/{key}", cfg.jira.base_url.trim_end_matches('/')));
    let snapshot_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("snapshot clock: {e}"))?
        .as_secs()
        .to_string();
    let mut text = String::with_capacity(template.len() + description.len() + comments.len());
    let mut rest = template.as_str();
    while let Some(start) = rest.find('{') {
        text.push_str(&rest[..start]);
        rest = &rest[start..];
        let fields = issue.as_ref();
        let value = [
            ("{key}", key),
            ("{url}", url.as_str()),
            ("{snapshot_at}", snapshot_at.as_str()),
            ("{description}", description.as_str()),
            ("{comments}", comments.as_str()),
            ("{labels}", labels.as_str()),
            ("{summary}", fields.map_or("", |i| i.summary.as_str())),
            ("{status}", fields.map_or("", |i| i.status.as_str())),
            ("{updated}", fields.map_or("", |i| i.updated.as_str())),
            ("{assignee}", fields.map_or("", |i| i.assignee.as_str())),
            ("{reporter}", fields.map_or("", |i| i.reporter.as_str())),
            ("{priority}", fields.map_or("", |i| i.priority.as_str())),
            ("{type}", fields.map_or("", |i| i.issue_type.as_str())),
        ]
        .into_iter()
        .find_map(|(name, value)| rest.starts_with(name).then_some(value));
        if let Some(value) = value {
            text.push_str(value);
            rest = &rest[rest.find('}').unwrap() + 1..];
        } else {
            text.push('{');
            rest = &rest[1..];
        }
    }
    text.push_str(rest);
    Ok(PromptPreview {
        key: key.into(),
        text: text.trim().into(),
        warnings,
    })
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

use super::*;
use serde_json::json;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

// Exercise the real subprocess/JSON boundary without touching a live herdr
// session. All tests that change the CLI environment hold this lock.
static CLI_ENV: Mutex<()> = Mutex::new(());
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct Step {
    args: Vec<String>,
    response: Value,
    exit: u8,
    /// Print the response on stderr (herdr 0.9.1 API errors) instead of stdout.
    stderr: bool,
    delay_ms: u64,
}

impl Step {
    fn ok(args: &[&str], response: Value) -> Self {
        Self {
            args: args.iter().map(|s| s.to_string()).collect(),
            response,
            exit: 0,
            stderr: false,
            delay_ms: 0,
        }
    }

    fn error(args: &[&str], code: &str) -> Self {
        Self {
            exit: 1,
            stderr: true,
            ..Self::ok(
                args,
                json!({"error": {"code": code, "message": "test failure"}}),
            )
        }
    }
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

struct MockCli {
    _lock: MutexGuard<'static, ()>,
    dir: PathBuf,
    old_bin: Option<OsString>,
    old_pane: Option<OsString>,
    steps: usize,
}

impl MockCli {
    fn new(steps: Vec<Step>) -> Self {
        let lock = CLI_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "herdr-jira-test-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&dir).unwrap();
        let path = |name: &str| quote(dir.join(name).to_str().unwrap());
        let mut script = format!(
            "#!/bin/sh\nset -eu\ncount={}\nfailed={}\n\
             fail() {{ printf '%s\\n' \"$*\" > \"$failed\"; printf '%s\\n' \"$*\" >&2; exit 90; }}\n\
             n=0\nif [ -f \"$count\" ]; then read -r n < \"$count\"; fi\n\
             n=$((n + 1))\nprintf '%s\\n' \"$n\" > \"$count\"\ncase \"$n\" in\n",
            path("count"), path("failed")
        );
        for (i, step) in steps.iter().enumerate() {
            script.push_str(&format!(
                "{})\n[ \"$#\" -eq {} ] || fail 'step {}: wrong argument count'\n",
                i + 1,
                step.args.len(),
                i + 1
            ));
            for (j, arg) in step.args.iter().enumerate() {
                // Braces: POSIX sh reads `$10` as `${1}0`.
                // A readiness timeout must be positive and use only the
                // remaining budget, not restart it after agent detection.
                if let Some(max) = arg.strip_prefix("TIMEOUT<=") {
                    script.push_str(&format!(
                        "[ \"${{{}}}\" -gt 0 ] && [ \"${{{}}}\" -le {} ] || fail 'invalid timeout'\n",
                        j + 1,
                        j + 1,
                        max
                    ));
                } else {
                    script.push_str(&format!(
                        "[ \"${{{}}}\" = {} ] || fail 'step {}: wrong argument {}'\n",
                        j + 1,
                        quote(arg),
                        i + 1,
                        j + 1
                    ));
                }
            }
            if step.delay_ms > 0 {
                script.push_str(&format!("sleep {:.3}\n", step.delay_ms as f64 / 1000.0));
            }
            script.push_str(&format!(
                "printf '%s\\n' {}{}\nexit {}\n;;\n",
                quote(&step.response.to_string()),
                if step.stderr { " >&2" } else { "" },
                step.exit
            ));
        }
        script.push_str("*) fail \"unexpected command: $*\" ;;\nesac\n");
        let bin = dir.join("herdr");
        fs::write(&bin, script).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
        let old_bin = std::env::var_os("HERDR_BIN_PATH");
        let old_pane = std::env::var_os("HERDR_PANE_ID");
        std::env::set_var("HERDR_BIN_PATH", bin);
        std::env::set_var("HERDR_PANE_ID", "w1:jira");
        Self {
            _lock: lock,
            dir,
            old_bin,
            old_pane,
            steps: steps.len(),
        }
    }

    fn assert_complete(&self) {
        assert!(
            !self.dir.join("failed").exists(),
            "mock CLI: {:?}",
            fs::read_to_string(self.dir.join("failed"))
        );
        let count = fs::read_to_string(self.dir.join("count")).unwrap_or_else(|_| "0".into());
        assert_eq!(count.trim().parse::<usize>().unwrap(), self.steps);
    }
}

impl Drop for MockCli {
    fn drop(&mut self) {
        for (key, old) in [
            ("HERDR_BIN_PATH", &self.old_bin),
            ("HERDR_PANE_ID", &self.old_pane),
        ] {
            match old {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn agent() -> HerdrAgent {
    parse_agent(
        &json!({
            "pane_id": "w2:p3", "terminal_id": "term_invalid_target",
            "name": "possibly-ambiguous", "agent": "claude", "agent_status": "idle"
        }),
        "",
    )
    .unwrap()
}

fn opts(placement: &str) -> StartAgentOpts {
    StartAgentOpts {
        name: "jira-TEST-1".into(),
        cwd: "/tmp/project with spaces".into(),
        argv: vec!["claude".into(), "--model".into(), "model name".into()],
        placement: placement.into(),
        focus: false,
        workspace_id: "w2".into(),
        tab_label: "TEST-1".into(),
        worktree: None,
    }
}

const CHECKOUT: &str = "/tmp/wt/project/test-1";

fn worktree_opts() -> WorktreeOpts {
    WorktreeOpts {
        branch: "TEST-1".into(),
        base: "origin/main".into(),
        path: CHECKOUT.into(),
        label: "TEST-1".into(),
        trust_repository: false,
    }
}

fn worktree_opts_start() -> StartAgentOpts {
    StartAgentOpts {
        worktree: Some(worktree_opts()),
        ..opts("tab")
    }
}

/// `worktree open --cwd <repo> --branch TEST-1 --label TEST-1 <focus> [trust]`.
fn worktree_open_args(extra: &[&'static str]) -> Vec<&'static str> {
    let mut args = vec![
        "worktree",
        "open",
        "--cwd",
        "/tmp/project with spaces",
        "--branch",
        "TEST-1",
        "--label",
        "TEST-1",
    ];
    args.extend(extra);
    args
}

/// `worktree create ... --base origin/main --path <checkout> --label TEST-1 <focus> [trust]`.
fn worktree_create_args(extra: &[&'static str]) -> Vec<&'static str> {
    let mut args = vec![
        "worktree",
        "create",
        "--cwd",
        "/tmp/project with spaces",
        "--branch",
        "TEST-1",
        "--base",
        "origin/main",
        "--path",
        CHECKOUT,
        "--label",
        "TEST-1",
    ];
    args.extend(extra);
    args
}

/// Real `herdr worktree open|create` result shape (trimmed).
fn worktree_response(kind: &str, already_open: bool) -> Value {
    json!({"id": "cli:worktree", "result": {
        "type": kind,
        "already_open": already_open,
        "root_pane": {"pane_id": "w3:p1", "workspace_id": "w3", "tab_id": "w3:t1", "cwd": CHECKOUT},
        "tab": {"tab_id": "w3:t1"},
        "workspace": {"workspace_id": "w3", "label": "TEST-1",
            "worktree": {"checkout_path": CHECKOUT, "is_linked_worktree": true}},
        "worktree": {"branch": "TEST-1", "path": CHECKOUT, "open_workspace_id": "w3"}
    }})
}

fn run_in_pane_steps(pane: &'static str) -> Vec<Step> {
    vec![
        Step::ok(
            &["pane", "run", pane, "claude --model 'model name'"],
            json!({"result": {}}),
        ),
        Step::ok(
            &["agent", "rename", pane, "jira-TEST-1"],
            json!({"result": {}}),
        ),
    ]
}

fn tab_start_steps() -> Vec<Step> {
    vec![
        Step::ok(
            &[
                "tab",
                "create",
                "--workspace",
                "w2",
                "--cwd",
                "/tmp/project with spaces",
                "--label",
                "TEST-1",
                "--no-focus",
            ],
            json!({"result": {"tab": {"tab_id": "w2:t1"}, "root_pane": {"pane_id": "w2:p3", "terminal_id": "term_invalid_target"}}}),
        ),
        Step::ok(
            &["pane", "run", "w2:p3", "claude --model 'model name'"],
            json!({"result": {}}),
        ),
        // Detection may not have happened at rename time. Its failure must
        // not make us address an unassigned name when waiting or sending.
        Step::error(
            &["agent", "rename", "w2:p3", "jira-TEST-1"],
            "agent_not_found",
        ),
    ]
}

#[test]
fn parsed_agents_use_pane_ids_and_exclude_non_delegable_panes() {
    assert_eq!(agent().target, "w2:p3");
    let mut a = json!({"pane_id": "w2:p3", "agent": "claude", "agent_status": "idle"});
    assert!(parse_agent(&a, "w2:p3").is_none());
    a["agent_status"] = json!("unknown");
    assert!(parse_agent(&a, "").is_none());
    a["agent_status"] = json!("idle");
    a["pane_id"] = json!("");
    assert!(parse_agent(&a, "").is_none());
    assert!(parse_agent(&json!({"pane_id": "w2:p4", "agent_status": "idle"}), "").is_none());
}

#[test]
fn response_errors_preserve_api_codes_from_stdout_or_stderr() {
    let error = r#"{"error":{"code":"agent_not_found","message":"not detected"}}"#;
    // Older herdr: error JSON on stdout (exit status may even be 0).
    for success in [false, true] {
        let e = parse_response(&["agent", "get", "w2:p3"], success, error, "").unwrap_err();
        assert_eq!(e.code.as_deref(), Some("agent_not_found"));
        assert!(e.message.contains("not detected"));
    }
    // herdr 0.9.1: error JSON on stderr, empty stdout, exit 1.
    let e =
        parse_response(&["agent", "get", "w2:p3"], false, "", &format!("{error}\n")).unwrap_err();
    assert_eq!(e.code.as_deref(), Some("agent_not_found"));
    assert!(e.message.contains("not detected"));
}

#[test]
fn response_parser_handles_empty_success_and_non_json_errors() {
    assert_eq!(
        parse_response(&["pane", "send-keys"], true, "\n", "").unwrap(),
        Value::Null
    );
    let e = parse_response(
        &["agent", "prompt", "w2:p3", "private Jira text"],
        false,
        "",
        "usage: agent prompt",
    )
    .unwrap_err();
    assert!(e.message.contains("usage: agent prompt"));
    assert!(!e.message.contains("private Jira text"));
    assert!(parse_response(&["agent", "list"], true, "invalid", "").is_err());
}

#[test]
fn split_parent_stays_in_selected_workspace() {
    let panes = json!({"result": {"panes": [
        {"workspace_id": "w1", "pane_id": "w1:jira", "focused": true},
        {"workspace_id": "w2", "pane_id": "w2:p1"},
        {"workspace_id": "w2", "pane_id": "w2:p2", "focused": true}
    ]}});
    assert_eq!(split_parent(&panes, "w2", "w1:jira"), Some("w2:p2"));
    assert_eq!(split_parent(&panes, "w2", "w2:p1"), Some("w2:p1"));
    assert_eq!(split_parent(&panes, "w1", ""), Some("w1:jira"));
    assert_eq!(split_parent(&panes, "missing", "w1:jira"), None);
    let first = json!({"result": {"panes": [{"workspace_id": "w2", "pane_id": "w2:p1"}]}});
    assert_eq!(split_parent(&first, "w2", "w1:jira"), Some("w2:p1"));
}

#[test]
fn submitted_multiline_prompt_uses_one_server_side_call() {
    let text = "Issue TEST-1: 'quotes' $HOME\nОписание задачи\n".repeat(200);
    let mock = MockCli::new(vec![Step::ok(
        &["agent", "prompt", "w2:p3", &text],
        json!({"result": {}}),
    )]);
    send_to_agent(&agent(), &text, true, 0).unwrap();
    mock.assert_complete();
}

#[test]
fn unsubmitted_prompt_only_sends_literal_text() {
    let mock = MockCli::new(vec![Step::ok(
        &["pane", "send-text", "w2:p3", "one\ntwo"],
        json!({"result": {}}),
    )]);
    send_to_agent(&agent(), "one\ntwo", false, 0).unwrap();
    mock.assert_complete();
}

#[test]
fn prompt_failures_do_not_fall_back_to_raw_input() {
    for code in ["agent_not_found", "agent_blocked", "timeout"] {
        let mock = MockCli::new(vec![Step::error(
            &["agent", "prompt", "w2:p3", "prompt"],
            code,
        )]);
        assert!(send_to_agent(&agent(), "prompt", true, 0)
            .unwrap_err()
            .contains(code));
        mock.assert_complete();
    }
}

#[test]
fn cold_tab_waits_for_detection_and_idle_before_submitting() {
    let mut steps = tab_start_steps();
    steps.extend([
        Step::error(&["agent", "get", "w2:p3"], "agent_not_found"),
        Step::error(&["agent", "get", "w2:p3"], "agent_not_found"),
        Step::ok(
            &["agent", "get", "w2:p3"],
            json!({"result": {"agent": {"agent_status": "unknown"}}}),
        ),
        Step::ok(
            &[
                "agent",
                "wait",
                "w2:p3",
                "--until",
                "idle",
                "--timeout",
                "TIMEOUT<=4800",
            ],
            json!({"result": {}}),
        ),
        Step::ok(
            &["agent", "prompt", "w2:p3", "prompt"],
            json!({"result": {}}),
        ),
    ]);
    let mock = MockCli::new(steps);
    let started = start_and_delegate(&opts("tab"), "prompt", true, 0, 0, 5000).unwrap();
    assert_eq!(started.target, "w2:p3");
    mock.assert_complete();
}

#[test]
fn readiness_failure_aborts_delivery_in_both_submit_modes() {
    for submit in [true, false] {
        let mut steps = tab_start_steps();
        steps.extend([
            Step::ok(&["agent", "get", "w2:p3"], json!({"result": {"agent": {}}})),
            Step::error(
                &[
                    "agent",
                    "wait",
                    "w2:p3",
                    "--until",
                    "idle",
                    "--timeout",
                    "TIMEOUT<=5000",
                ],
                "timeout",
            ),
        ]);
        let mock = MockCli::new(steps);
        let error = start_and_delegate(&opts("tab"), "prompt", submit, 0, 0, 5000).unwrap_err();
        assert!(error.contains("not ready; prompt not sent"));
        assert!(error.contains("timeout"));
        mock.assert_complete();
    }
}

#[test]
fn detection_timeout_aborts_delivery() {
    let mut steps = tab_start_steps();
    let mut missing = Step::error(&["agent", "get", "w2:p3"], "agent_not_found");
    missing.delay_ms = 50;
    steps.push(missing);
    let mock = MockCli::new(steps);
    let error = start_and_delegate(&opts("tab"), "prompt", true, 0, 0, 20).unwrap_err();
    assert!(error.contains("not detected within 20ms"));
    mock.assert_complete();
}

#[test]
fn detection_errors_other_than_not_found_fail_immediately() {
    let mock = MockCli::new(vec![Step::error(
        &["agent", "get", "w2:p3"],
        "connection_failed",
    )]);
    assert!(wait_agent_status("w2:p3", "idle", 5000)
        .unwrap_err()
        .contains("connection_failed"));
    mock.assert_complete();
}

#[test]
fn zero_readiness_budget_explicitly_skips_waiting() {
    let mut steps = tab_start_steps();
    steps.push(Step::ok(
        &["pane", "send-text", "w2:p3", "prompt"],
        json!({"result": {}}),
    ));
    let mock = MockCli::new(steps);
    start_and_delegate(&opts("tab"), "prompt", false, 0, 0, 0).unwrap();
    mock.assert_complete();
}

#[test]
fn split_placements_create_then_run_in_selected_workspace() {
    for (direction, focus) in [("right", false), ("down", true)] {
        let mock = MockCli::new(vec![
            Step::ok(
                &["pane", "list", "--workspace", "w2"],
                json!({"result": {"panes": [
                    {"workspace_id": "w2", "pane_id": "w2:p1", "focused": true}
                ]}}),
            ),
            Step::ok(
                &[
                    "pane",
                    "split",
                    "w2:p1",
                    "--direction",
                    direction,
                    "--cwd",
                    "/tmp/project with spaces",
                    if focus { "--focus" } else { "--no-focus" },
                ],
                json!({"result": {"pane": {"pane_id": "w2:p3", "terminal_id": "term_invalid_target"}}}),
            ),
            Step::ok(
                &["pane", "run", "w2:p3", "claude --model 'model name'"],
                json!({"result": {}}),
            ),
            Step::ok(
                &["agent", "rename", "w2:p3", "jira-TEST-1"],
                json!({"result": {}}),
            ),
            Step::ok(&["agent", "get", "w2:p3"], json!({"result": {"agent": {}}})),
            Step::ok(
                &[
                    "agent",
                    "wait",
                    "w2:p3",
                    "--until",
                    "idle",
                    "--timeout",
                    "TIMEOUT<=5000",
                ],
                json!({"result": {}}),
            ),
            Step::ok(
                &["agent", "prompt", "w2:p3", "prompt"],
                json!({"result": {}}),
            ),
        ]);
        let mut options = opts(direction);
        options.focus = focus;
        let started = start_and_delegate(&options, "prompt", true, 0, 0, 5000).unwrap();
        assert_eq!(started.target, "w2:p3");
        mock.assert_complete();
    }
}

#[test]
fn malformed_split_response_does_not_start_or_send() {
    let mock = MockCli::new(vec![
        Step::ok(
            &["pane", "list", "--workspace", "w2"],
            json!({"result": {"panes": [{"workspace_id": "w2", "pane_id": "w2:p1"}]}}),
        ),
        Step::ok(
            &[
                "pane",
                "split",
                "w2:p1",
                "--direction",
                "right",
                "--cwd",
                "/tmp/project with spaces",
                "--no-focus",
            ],
            json!({"result": {}}),
        ),
    ]);
    assert!(
        start_and_delegate(&opts("right"), "prompt", true, 0, 0, 5000)
            .unwrap_err()
            .contains("no pane.pane_id")
    );
    mock.assert_complete();
}

#[test]
fn empty_workspace_does_not_split_jira_pane_instead() {
    let mock = MockCli::new(vec![Step::ok(
        &["pane", "list", "--workspace", "w2"],
        json!({"result": {"panes": []}}),
    )]);
    assert!(start_agent(&opts("down"))
        .unwrap_err()
        .contains("no pane to split in workspace w2"));
    mock.assert_complete();
}

#[test]
fn invalid_start_options_do_not_invoke_cli() {
    let mock = MockCli::new(vec![]);
    assert!(start_agent(&opts("left")).is_err());
    let mut options = opts("tab");
    options.argv.clear();
    assert!(start_agent(&options).is_err());
    options = opts("tab");
    options.name.clear();
    assert!(start_agent(&options).is_err());
    for placement in ["tab", "", "right", "down"] {
        options = opts(placement);
        options.workspace_id.clear();
        assert!(start_agent(&options).is_err(), "{placement:?}");
    }
    mock.assert_complete();
}

#[test]
fn invalid_worktree_options_do_not_invoke_cli() {
    let mock = MockCli::new(vec![]);
    let mut options = worktree_opts_start();
    options.worktree = Some(WorktreeOpts {
        branch: " ".into(),
        ..worktree_opts()
    });
    assert!(start_agent(&options).is_err());
    assert!(open_or_create_worktree("", &worktree_opts(), false).is_err());
    mock.assert_complete();
}

#[test]
fn new_worktree_branch_is_created_then_agent_runs_in_its_root_pane() {
    let mut steps = vec![
        Step::error(&worktree_open_args(&["--no-focus"]), "worktree_not_found"),
        Step::ok(
            &worktree_create_args(&["--no-focus"]),
            worktree_response("worktree_created", false),
        ),
    ];
    steps.extend(run_in_pane_steps("w3:p1"));
    let mock = MockCli::new(steps);
    let started = start_agent(&worktree_opts_start()).unwrap();
    assert_eq!(started.target, "w3:p1");
    assert_eq!(started.cwd, CHECKOUT);
    mock.assert_complete();
}

#[test]
fn worktree_start_does_not_need_a_workspace() {
    let mut steps = vec![
        Step::error(&worktree_open_args(&["--no-focus"]), "worktree_not_found"),
        Step::ok(
            &worktree_create_args(&["--no-focus"]),
            worktree_response("worktree_created", false),
        ),
    ];
    steps.extend(run_in_pane_steps("w3:p1"));
    let mock = MockCli::new(steps);
    let mut options = worktree_opts_start();
    options.workspace_id.clear();
    let started = start_agent(&options).unwrap();
    assert_eq!(started.target, "w3:p1");
    assert_eq!(started.cwd, CHECKOUT);
    mock.assert_complete();
}

#[test]
fn empty_worktree_base_path_and_label_are_omitted() {
    let mock = MockCli::new(vec![
        Step::error(
            &[
                "worktree",
                "open",
                "--cwd",
                "/tmp/project with spaces",
                "--branch",
                "TEST-1",
                "--no-focus",
            ],
            "worktree_not_found",
        ),
        Step::ok(
            &[
                "worktree",
                "create",
                "--cwd",
                "/tmp/project with spaces",
                "--branch",
                "TEST-1",
                "--no-focus",
            ],
            worktree_response("worktree_created", false),
        ),
    ]);
    let wt = WorktreeOpts {
        branch: "TEST-1".into(),
        ..WorktreeOpts::default()
    };
    let opened = open_or_create_worktree("/tmp/project with spaces", &wt, false).unwrap();
    assert_eq!(opened.pane_id, "w3:p1");
    mock.assert_complete();
}

#[test]
fn closed_worktree_is_reopened_without_create() {
    let mut steps = vec![Step::ok(
        &worktree_open_args(&["--no-focus"]),
        worktree_response("worktree_opened", false),
    )];
    steps.extend(run_in_pane_steps("w3:p1"));
    let mock = MockCli::new(steps);
    let started = start_agent(&worktree_opts_start()).unwrap();
    assert_eq!(started.target, "w3:p1");
    assert_eq!(started.cwd, CHECKOUT);
    mock.assert_complete();
}

#[test]
fn already_open_worktree_gets_a_new_tab_instead_of_its_busy_root_pane() {
    let mut steps = vec![
        Step::ok(
            &worktree_open_args(&["--no-focus"]),
            worktree_response("worktree_opened", true),
        ),
        Step::ok(
            &[
                "tab",
                "create",
                "--workspace",
                "w3",
                "--cwd",
                CHECKOUT,
                "--label",
                "TEST-1",
                "--no-focus",
            ],
            json!({"result": {"tab": {"tab_id": "w3:t2"}, "root_pane": {"pane_id": "w3:p2"}}}),
        ),
    ];
    steps.extend(run_in_pane_steps("w3:p2"));
    let mock = MockCli::new(steps);
    let started = start_agent(&worktree_opts_start()).unwrap();
    assert_eq!(started.target, "w3:p2");
    assert_eq!(started.cwd, CHECKOUT);
    mock.assert_complete();
}

#[test]
fn worktree_open_errors_other_than_not_found_do_not_create() {
    let mock = MockCli::new(vec![Step::error(
        &worktree_open_args(&["--no-focus"]),
        "not_git_worktree",
    )]);
    assert!(start_agent(&worktree_opts_start())
        .unwrap_err()
        .contains("not_git_worktree"));
    mock.assert_complete();
}

#[test]
fn worktree_response_without_root_pane_does_not_run_agent() {
    let mock = MockCli::new(vec![
        Step::error(&worktree_open_args(&["--no-focus"]), "worktree_not_found"),
        Step::ok(
            &worktree_create_args(&["--no-focus"]),
            json!({"result": {"type": "worktree_created",
                "root_pane": {"workspace_id": "w3", "cwd": CHECKOUT},
                "workspace": {"workspace_id": "w3"},
                "worktree": {"branch": "TEST-1", "path": CHECKOUT}}}),
        ),
    ]);
    assert!(start_agent(&worktree_opts_start())
        .unwrap_err()
        .contains("root_pane.pane_id"));
    mock.assert_complete();
}

#[test]
fn trusted_repository_flag_is_passed_to_open_and_create() {
    let mock = MockCli::new(vec![
        Step::error(
            &worktree_open_args(&["--focus", "--trust-repository"]),
            "worktree_not_found",
        ),
        Step::ok(
            &worktree_create_args(&["--focus", "--trust-repository"]),
            worktree_response("worktree_created", false),
        ),
    ]);
    let wt = WorktreeOpts {
        trust_repository: true,
        ..worktree_opts()
    };
    let opened = open_or_create_worktree("/tmp/project with spaces", &wt, true).unwrap();
    assert_eq!(opened.workspace_id, "w3");
    assert_eq!(opened.pane_id, "w3:p1");
    assert_eq!(opened.checkout_path, CHECKOUT);
    assert!(!opened.already_open);
    mock.assert_complete();
}

/// `workspace list` entry; `repo` = (repo_name, checkout_path, is_linked_worktree).
fn workspace_json(id: &str, number: u64, repo: Option<(&str, &str, bool)>) -> Value {
    let mut w = json!({"workspace_id": id, "label": id, "number": number, "focused": false});
    if let Some((name, checkout, linked)) = repo {
        w["worktree"] = json!({
            "checkout_path": checkout, "is_linked_worktree": linked,
            "repo_key": format!("/x/{name}/.git"), "repo_name": name,
            "repo_root": format!("/x/{name}"),
        });
    }
    w
}

fn workspace(id: &str, number: u64, repo: Option<(&str, &str, bool)>) -> HerdrWorkspace {
    parse_workspace(&workspace_json(id, number, repo)).unwrap()
}

#[test]
fn workspace_repo_is_parsed_only_for_git_workspaces() {
    let ws = workspace("w2", 2, Some(("backend", "/x/backend-feat", true)));
    let repo = ws.repo.unwrap();
    assert_eq!(repo.repo_key, "/x/backend/.git");
    assert_eq!(repo.repo_root, "/x/backend");
    assert_eq!(repo.checkout_path, "/x/backend-feat");
    assert!(repo.is_linked_worktree);
    assert!(workspace("w3", 3, None).repo.is_none());
    assert!(parse_workspace(&json!({"label": "no id"})).is_none());
}

#[test]
fn projects_fold_worktrees_into_their_repo_with_current_first() {
    let ws = vec![
        workspace("w1", 1, Some(("frontend", "/x/frontend", false))),
        workspace("w2", 2, None),
        workspace("w3", 3, Some(("backend", "/x/backend", false))),
        workspace("w4", 4, Some(("jira", "/x/jira-feat", true))),
        workspace("w5", 5, Some(("backend", "/x/backend-a", true))),
        workspace("w6", 6, Some(("backend", "/x/backend-b", true))),
    ];
    let summary = |current: &str| -> Vec<(String, usize, bool)> {
        projects_from_workspaces(&ws, current)
            .into_iter()
            .map(|p| (p.name, p.open_worktrees, p.current))
            .collect()
    };
    let row = |name: &str, open: usize, current: bool| (name.to_string(), open, current);
    // The current workspace is a linked worktree: its source repo still leads.
    assert_eq!(
        summary("w6"),
        vec![
            row("backend", 2, true),
            row("frontend", 0, false),
            row("jira", 1, false)
        ]
    );
    // A repo open only as a linked worktree is still a project; the rest keep workspace order.
    let projects = projects_from_workspaces(&ws, "w4");
    assert_eq!(projects[0].root, "/x/jira");
    assert_eq!(
        summary("w4"),
        vec![
            row("jira", 1, true),
            row("frontend", 0, false),
            row("backend", 2, false)
        ]
    );
    // Non-git or unknown current workspace: plain workspace order.
    assert_eq!(summary("w2"), summary(""));
    assert_eq!(summary("")[0], row("frontend", 0, false));
}

#[test]
fn worktree_list_is_parsed_and_unopenable_entries_are_skipped() {
    let mock = MockCli::new(vec![Step::ok(
        &["worktree", "list", "--cwd", "/x/jira"],
        json!({"result": {"type": "worktree_list",
        "source": {"repo_key": "/x/jira/.git", "repo_name": "jira", "repo_root": "/x/jira"},
        "worktrees": [
            {"branch": "main", "is_bare": false, "is_detached": false,
             "is_linked_worktree": false, "is_prunable": false,
             "path": "/x/jira", "open_workspace_id": "w22"},
            {"branch": "refs/heads/feat/x", "is_linked_worktree": true,
             "path": "/x/jira-feat", "open_workspace_id": null},
            {"branch": "old", "is_linked_worktree": true, "is_prunable": true, "path": "/gone"},
            {"is_detached": true, "is_linked_worktree": true, "path": "/x/detached"},
            {"branch": "", "is_bare": true, "path": "/x/bare"},
            {"branch": "fix", "is_linked_worktree": true, "path": "/x/jira-fix"}
        ]}}),
    )]);
    let wts = list_worktrees("/x/jira").unwrap();
    mock.assert_complete();
    let wt = |branch: &str, path: &str, open: bool, is_main: bool| RepoWorktree {
        branch: branch.into(),
        path: path.into(),
        open,
        is_main,
    };
    assert_eq!(
        wts,
        vec![
            wt("main", "/x/jira", true, true),
            wt("feat/x", "/x/jira-feat", false, false),
            wt("fix", "/x/jira-fix", false, false),
        ]
    );
}

/// `worktree list --workspace` result for the jira repo; `feat` is open in w30.
fn jira_worktree_list() -> Value {
    json!({"result": {"type": "worktree_list",
    "source": {"repo_key": "/x/jira/.git", "repo_name": "jira", "repo_root": "/x/jira",
               "source_checkout_path": "/x/jira", "source_workspace_id": "w22"},
    "worktrees": [
        {"branch": "main", "is_linked_worktree": false, "path": "/x/jira",
         "open_workspace_id": "w22"},
        {"branch": "feat", "is_linked_worktree": true, "path": "/x/jira-feat",
         "open_workspace_id": "w30"}
    ]}})
}

#[test]
fn probed_worktree_list_fills_the_workspace_repo() {
    let list = jira_worktree_list();
    let result = &list["result"];
    let linked = repo_from_worktree_list(result, "w30").unwrap();
    assert_eq!(linked.repo_key, "/x/jira/.git");
    assert_eq!(linked.repo_name, "jira");
    assert_eq!(linked.repo_root, "/x/jira");
    assert_eq!(linked.checkout_path, "/x/jira-feat");
    assert!(linked.is_linked_worktree);
    // No worktree entry open in this workspace: it is the source checkout.
    let source = repo_from_worktree_list(result, "w99").unwrap();
    assert_eq!(source.checkout_path, "/x/jira");
    assert!(!source.is_linked_worktree);
    assert!(repo_from_worktree_list(&json!({"worktrees": []}), "w22").is_none());
}

#[test]
fn projects_include_git_workspaces_without_worktree_info() {
    let mut backend = workspace_json("w1", 1, Some(("backend", "/x/backend", false)));
    backend["focused"] = json!(true);
    let mut jira = workspace_json("w22", 22, None);
    jira["worktree"] = Value::Null;
    let mock = MockCli::new(vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [backend, jira, workspace_json("w3", 3, None)]}}),
        ),
        Step::ok(
            &["worktree", "list", "--workspace", "w22"],
            jira_worktree_list(),
        ),
        Step::error(
            &["worktree", "list", "--workspace", "w3"],
            "not_a_git_repository",
        ),
    ]);
    let old_ws = std::env::var_os("HERDR_WORKSPACE_ID");
    std::env::set_var("HERDR_WORKSPACE_ID", "w22");
    let projects = list_projects();
    match old_ws {
        Some(value) => std::env::set_var("HERDR_WORKSPACE_ID", value),
        None => std::env::remove_var("HERDR_WORKSPACE_ID"),
    }
    mock.assert_complete();
    let names: Vec<(String, bool)> = projects
        .unwrap()
        .into_iter()
        .map(|p| (p.name, p.current))
        .collect();
    assert_eq!(
        names,
        vec![("jira".to_string(), true), ("backend".to_string(), false)]
    );
}

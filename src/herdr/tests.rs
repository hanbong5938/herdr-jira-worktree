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
        checkout: None,
    }
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
    send_to_agent(&agent(), &text, true).unwrap();
    mock.assert_complete();
}

#[test]
fn unsubmitted_prompt_only_sends_literal_text() {
    let mock = MockCli::new(vec![Step::ok(
        &["pane", "send-text", "w2:p3", "one\ntwo"],
        json!({"result": {}}),
    )]);
    send_to_agent(&agent(), "one\ntwo", false).unwrap();
    mock.assert_complete();
}

#[test]
fn prompt_failures_do_not_fall_back_to_raw_input() {
    for code in ["agent_not_found", "agent_blocked", "timeout"] {
        let mock = MockCli::new(vec![Step::error(
            &["agent", "prompt", "w2:p3", "prompt"],
            code,
        )]);
        assert!(send_to_agent(&agent(), "prompt", true)
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
    let started = start_and_delegate(&opts("tab"), "prompt", true, 0, 5000).unwrap();
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
        let error = start_and_delegate(&opts("tab"), "prompt", submit, 0, 5000).unwrap_err();
        assert!(error.message.contains("not ready; prompt not sent"));
        assert!(error.message.contains("timeout"));
        assert!(error.retry_safe);
        assert_eq!(error.agent.unwrap().pane_id, "w2:p3");
        mock.assert_complete();
    }
}

#[test]
fn failed_send_retains_pane_but_cannot_offer_a_safe_retry() {
    let mut steps = tab_start_steps();
    steps.extend([
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
        Step::error(&["agent", "prompt", "w2:p3", "prompt"], "timeout"),
    ]);
    let mock = MockCli::new(steps);
    let error = start_and_delegate(&opts("tab"), "prompt", true, 0, 5000).unwrap_err();
    assert_eq!(error.agent.unwrap().pane_id, "w2:p3");
    assert!(!error.retry_safe);
    assert!(error.message.contains("timeout"));
    mock.assert_complete();
}

#[test]
fn detection_timeout_aborts_delivery() {
    let mut steps = tab_start_steps();
    let mut missing = Step::error(&["agent", "get", "w2:p3"], "agent_not_found");
    missing.delay_ms = 50;
    steps.push(missing);
    let mock = MockCli::new(steps);
    let error = start_and_delegate(&opts("tab"), "prompt", true, 0, 20).unwrap_err();
    assert!(error.message.contains("not detected within 20ms"));
    assert!(error.retry_safe);
    assert_eq!(error.agent.unwrap().target, "w2:p3");
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
    start_and_delegate(&opts("tab"), "prompt", false, 0, 0).unwrap();
    mock.assert_complete();
}

#[test]
fn resume_with_zero_readiness_budget_uses_existing_pane_only() {
    let mock = MockCli::new(vec![Step::ok(
        &["pane", "send-text", "w2:p3", "prompt"],
        json!({"result": {}}),
    )]);
    let existing = agent();
    let resumed = resume_delegation(&existing, "prompt", false, 0).unwrap();
    assert_eq!(resumed.pane_id, existing.pane_id);
    assert_eq!(resumed.target, existing.target);
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
        let started = start_and_delegate(&options, "prompt", true, 0, 5000).unwrap();
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
    let error = start_and_delegate(&opts("right"), "prompt", true, 0, 5000).unwrap_err();
    assert!(error.message.contains("no pane.pane_id"));
    assert!(error.agent.is_none());
    assert!(!error.retry_safe);
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

struct GitFixture {
    root: PathBuf,
    linked: PathBuf,
}

impl GitFixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "herdr-jira-git-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let root = dir.join("source");
        let linked = dir.join("feature");
        fs::create_dir_all(&root).unwrap();
        let run = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&["init", "-b", "main", root.to_str().unwrap()]);
        run(&[
            "-C",
            root.to_str().unwrap(),
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "seed",
        ]);
        run(&[
            "-C",
            root.to_str().unwrap(),
            "worktree",
            "add",
            "-b",
            "feature",
            linked.to_str().unwrap(),
        ]);
        Self { root, linked }
    }

    fn request(&self, selection: Selection) -> Request {
        Request {
            repository: checkout::resolve_repository(self.root.to_str().unwrap(), Backend::Jj)
                .unwrap(),
            selection,
            label: "TEST-1".into(),
            trust_repository: false,
            recovery_acknowledged: false,
        }
    }

    fn existing(&self) -> Request {
        self.request(Selection::Existing(Entry {
            name: "feature".into(),
            path: Some(self.linked.to_str().unwrap().into()),
            open: false,
            is_source: false,
        }))
    }
}

impl Drop for GitFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.root.parent().unwrap());
    }
}

fn generic_response(cwd: &str) -> Value {
    json!({"result": {"type": "workspace_created",
        "workspace": {"workspace_id": "w3"},
        "tab": {"tab_id": "w3:t1"},
        "root_pane": {"pane_id": "w3:p1", "workspace_id": "w3", "tab_id": "w3:t1", "cwd": cwd}}})
}

#[test]
fn selected_existing_git_uses_exact_path_and_a_fresh_ui_root() {
    let repo = GitFixture::new();
    let path = repo.linked.to_str().unwrap();
    let steps = vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": []}}),
        ),
        Step::ok(
            &[
                "workspace",
                "create",
                "--cwd",
                path,
                "--label",
                "TEST-1",
                "--no-focus",
            ],
            generic_response(path),
        ),
    ];
    let mock = MockCli::new(steps);
    let opened = open_or_create_checkout(&repo.existing(), false).unwrap();
    assert_eq!(opened.checkout_path, path);
    assert_eq!(opened.pane_id, "w3:p1");
    assert!(!opened.already_open);
    mock.assert_complete();
}

#[test]
fn vanished_selection_is_never_recreated_even_with_valid_branch() {
    let repo = GitFixture::new();
    let mock = MockCli::new(vec![]);
    let mut request = repo.existing();
    if let Selection::Existing(entry) = &mut request.selection {
        entry.path = Some(
            repo.root
                .parent()
                .unwrap()
                .join("vanished")
                .to_string_lossy()
                .into_owned(),
        );
    }
    assert!(open_or_create_checkout(&request, false)
        .unwrap_err()
        .contains("vanished"));
    mock.assert_complete();
}

#[test]
fn existing_ui_workspace_uses_new_tab_without_running_busy_root() {
    let repo = GitFixture::new();
    let path = repo.linked.to_str().unwrap();
    let steps = vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [
                {"workspace_id": "w3", "number": 3, "worktree": null}
            ]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "w3"],
            json!({"result": {"panes": [{"workspace_id": "w3", "pane_id": "w3:p1",
                "cwd": path, "foreground_cwd": "/some/other/repo"}]}}),
        ),
        Step::ok(
            &[
                "tab",
                "create",
                "--workspace",
                "w3",
                "--cwd",
                path,
                "--label",
                "TEST-1",
                "--no-focus",
            ],
            json!({"result": {"tab": {"tab_id": "w3:t2"}, "root_pane": {"pane_id": "w3:p2"}}}),
        ),
        Step::ok(
            &["pane", "run", "w3:p2", "claude --model 'model name'"],
            json!({"result": {}}),
        ),
        Step::ok(
            &["agent", "rename", "w3:p2", "jira-TEST-1"],
            json!({"result": {}}),
        ),
    ];
    let mock = MockCli::new(steps);
    let mut options = opts("tab");
    options.checkout = Some(repo.existing().into());
    let started = start_agent(&options).unwrap();
    assert_eq!(started.pane_id, "w3:p2");
    assert_eq!(started.cwd, path);
    mock.assert_complete();
}

#[test]
fn conflicting_ui_workspaces_are_not_guessed_or_overwritten() {
    let repo = GitFixture::new();
    let path = repo.linked.to_str().unwrap();
    let subdir = repo.linked.join("src");
    fs::create_dir(&subdir).unwrap();
    let mock = MockCli::new(vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [
                {"workspace_id": "w3"}, {"workspace_id": "w4"}
            ]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "w3"],
            json!({"result": {"panes": [{"cwd": subdir}, {"cwd": path}]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "w4"],
            json!({"result": {"panes": [{"cwd": path}]}}),
        ),
    ]);
    assert!(open_or_create_checkout(&repo.existing(), false)
        .unwrap_err()
        .contains("multiple"));
    mock.assert_complete();
}

#[test]
fn git_new_creates_only_for_structured_not_found_and_uses_explicit_trust() {
    let repo = GitFixture::new();
    let source = repo.root.to_str().unwrap();
    let path = repo.linked.to_str().unwrap();
    let mut request = repo.request(Selection::New {
        name: "feature".into(),
        base: "main".into(),
        path: path.into(),
    });
    request.trust_repository = true;
    let mock = MockCli::new(vec![
        Step::error(
            &[
                "worktree",
                "open",
                "--cwd",
                source,
                "--branch",
                "feature",
                "--label",
                "TEST-1",
                "--focus",
                "--trust-repository",
            ],
            "worktree_not_found",
        ),
        Step::ok(
            &[
                "worktree",
                "create",
                "--cwd",
                source,
                "--branch",
                "feature",
                "--base",
                "main",
                "--path",
                path,
                "--label",
                "TEST-1",
                "--focus",
                "--trust-repository",
            ],
            json!({"result": {"root_pane": {"pane_id": "w3:p1", "cwd": path},
                "workspace": {"workspace_id": "w3"}, "worktree": {"path": path}}}),
        ),
    ]);
    let opened = open_or_create_checkout(&request, true).unwrap();
    assert_eq!(opened.checkout_path, path);
    mock.assert_complete();
}

#[test]
fn unrelated_git_open_error_does_not_create_or_retry_with_trust() {
    let repo = GitFixture::new();
    let request = repo.request(Selection::New {
        name: "feature".into(),
        base: String::new(),
        path: String::new(),
    });
    let mock = MockCli::new(vec![Step::error(
        &[
            "worktree",
            "open",
            "--cwd",
            repo.root.to_str().unwrap(),
            "--branch",
            "feature",
            "--label",
            "TEST-1",
            "--no-focus",
        ],
        "repository_untrusted",
    )]);
    assert!(open_or_create_checkout(&request, false)
        .unwrap_err()
        .contains("repository_untrusted"));
    mock.assert_complete();
}

#[test]
fn jj_native_checkout_survives_ui_failure_and_retry_reuses_same_path() {
    let Some(repo) = crate::checkout::tests::Fixture::new(false) else {
        return;
    };
    let checkout_path = repo.dir.join("native-space");
    let path = checkout_path.to_str().unwrap();
    let request = repo.request("native-space", &checkout_path, "");
    let mock = MockCli::new(vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": []}}),
        ),
        Step::error(
            &[
                "workspace",
                "create",
                "--cwd",
                path,
                "--label",
                "native-space",
                "--no-focus",
            ],
            "workspace_create_failed",
        ),
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": []}}),
        ),
        Step::ok(
            &[
                "workspace",
                "create",
                "--cwd",
                path,
                "--label",
                "native-space",
                "--no-focus",
            ],
            generic_response(path),
        ),
    ]);
    let error = open_or_create_checkout(&request, false).unwrap_err();
    assert!(error.contains(path));
    assert!(checkout_path.join(".jj").is_dir());
    fs::write(checkout_path.join("user-file"), "preserve me").unwrap();
    let opened = open_or_create_checkout(&request, false).unwrap();
    assert_eq!(opened.checkout_path, path);
    assert_eq!(
        fs::read_to_string(checkout_path.join("user-file")).unwrap(),
        "preserve me"
    );
    mock.assert_complete();
}

#[test]
fn malformed_generic_root_pane_does_not_launch_agent() {
    let repo = GitFixture::new();
    let path = repo.linked.to_str().unwrap();
    let mock = MockCli::new(vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": []}}),
        ),
        Step::ok(
            &[
                "workspace",
                "create",
                "--cwd",
                path,
                "--label",
                "TEST-1",
                "--no-focus",
            ],
            json!({"result": {"workspace": {"workspace_id": "w3"},
                "tab": {"tab_id": "w3:t1"}, "root_pane": {"cwd": path}}}),
        ),
    ]);
    let mut options = opts("tab");
    options.checkout = Some(repo.existing().into());
    assert!(start_agent(&options)
        .unwrap_err()
        .contains("missing root pane"));
    mock.assert_complete();
}

#[test]
fn project_discovery_folds_linked_jj_store_with_current_source() {
    let Some(repo) = crate::checkout::tests::Fixture::new(false) else {
        return;
    };
    let linked = repo.dir.join("linked");
    drop(checkout::ensure_jj(&repo.request("linked", &linked, "")).unwrap());
    let source = repo.root.to_str().unwrap();
    let linked_path = linked.to_str().unwrap();
    let mock = MockCli::new(vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [
                {"workspace_id": "w1", "number": 1},
                {"workspace_id": "w2", "number": 2}
            ]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "w1"],
            json!({"result": {"panes": [{"cwd": source}]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "w2"],
            json!({"result": {"panes": [{"cwd": linked_path}]}}),
        ),
    ]);
    let old = std::env::var_os("HERDR_WORKSPACE_ID");
    std::env::set_var("HERDR_WORKSPACE_ID", "w2");
    let projects = list_projects(Backend::Jj);
    match old {
        Some(value) => std::env::set_var("HERDR_WORKSPACE_ID", value),
        None => std::env::remove_var("HERDR_WORKSPACE_ID"),
    }
    let inventory = projects.unwrap();
    assert!(inventory.warnings.is_empty());
    assert_eq!(inventory.projects.len(), 1);
    assert_eq!(inventory.projects[0].backend, Backend::Jj);
    assert_eq!(inventory.projects[0].root, linked_path);
    assert!(inventory.projects[0].current);
    assert_eq!(inventory.projects[0].open_worktrees, 1);
    mock.assert_complete();
}

#[test]
fn checkout_subdirectory_reopens_existing_ui_workspace() {
    let repo = GitFixture::new();
    let path = repo.linked.to_str().unwrap();
    let subdir = repo.linked.join("src");
    fs::create_dir(&subdir).unwrap();
    let mock = MockCli::new(vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [{"workspace_id": "w1"}]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "w1"],
            json!({"result": {"panes": [
                {"cwd": subdir, "foreground_cwd": repo.root},
                {"cwd": path}
            ]}}),
        ),
    ]);
    let opened = open_or_create_checkout(&repo.existing(), false).unwrap();
    assert_eq!(opened.workspace_id, "w1");
    assert_eq!(opened.checkout_path, path);
    assert!(opened.already_open);
    mock.assert_complete();
}

#[test]
fn nested_repo_boundary_does_not_claim_parent_checkout() {
    let repo = GitFixture::new();
    let nested = repo.linked.join("nested");
    fs::create_dir(&nested).unwrap();
    let init = std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .arg(&nested)
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let subdir = nested.join("src");
    fs::create_dir(&subdir).unwrap();
    let path = repo.linked.to_str().unwrap();
    let mock = MockCli::new(vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [{"workspace_id": "nested"}]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "nested"],
            json!({"result": {"panes": [{"cwd": subdir, "foreground_cwd": path}]}}),
        ),
        Step::ok(
            &[
                "workspace",
                "create",
                "--cwd",
                path,
                "--label",
                "TEST-1",
                "--no-focus",
            ],
            generic_response(path),
        ),
    ]);
    let opened = open_or_create_checkout(&repo.existing(), false).unwrap();
    assert!(!opened.already_open);
    assert_eq!(opened.workspace_id, "w3");
    mock.assert_complete();
}

#[test]
fn sibling_checkout_and_foreground_cwd_do_not_claim_selection() {
    let repo = GitFixture::new();
    let path = repo.linked.to_str().unwrap();
    let source = repo.root.to_str().unwrap();
    let mock = MockCli::new(vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [{"workspace_id": "source"}]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "source"],
            json!({"result": {"panes": [{"cwd": source, "foreground_cwd": path}]}}),
        ),
        Step::ok(
            &[
                "workspace",
                "create",
                "--cwd",
                path,
                "--label",
                "TEST-1",
                "--no-focus",
            ],
            generic_response(path),
        ),
    ]);
    let opened = open_or_create_checkout(&repo.existing(), false).unwrap();
    assert!(!opened.already_open);
    mock.assert_complete();
}

#[test]
fn unknown_pane_inventory_blocks_duplicate_ui_creation() {
    let repo = GitFixture::new();
    let mock = MockCli::new(vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [{"workspace_id": "w1"}]}}),
        ),
        Step::error(&["pane", "list", "--workspace", "w1"], "pane_unavailable"),
    ]);
    let error = open_or_create_checkout(&repo.existing(), false).unwrap_err();
    assert!(error.contains("w1") && error.contains("pane"));
    assert!(error.contains(repo.linked.to_str().unwrap()));
    mock.assert_complete();
}

#[test]
fn missing_pane_cwd_is_not_evidence_that_checkout_is_vacant() {
    let repo = GitFixture::new();
    let mock = MockCli::new(vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [{"workspace_id": "w1"}]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "w1"],
            json!({"result": {"panes": [{"pane_id": "w1:p1"}]}}),
        ),
    ]);
    let error = open_or_create_checkout(&repo.existing(), false).unwrap_err();
    assert!(error.contains("stable cwd"));
    mock.assert_complete();
}

#[test]
fn subdirectory_open_marking_uses_stable_checkout_boundary() {
    let repo = GitFixture::new();
    let subdir = repo.linked.join("src");
    fs::create_dir(&subdir).unwrap();
    let root = repo.root.to_str().unwrap();
    let linked = repo.linked.to_str().unwrap();
    let mock = MockCli::new(vec![
        Step::ok(
            &["worktree", "list", "--cwd", root],
            json!({"result": {"worktrees": [
                {"path": root, "branch": "refs/heads/main"},
                {"path": linked, "branch": "refs/heads/feature", "is_linked_worktree": true}
            ]}}),
        ),
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [{"workspace_id": "w1"}]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "w1"],
            json!({"result": {"panes": [
                {"cwd": subdir, "foreground_cwd": root},
                {"cwd": subdir}
            ]}}),
        ),
    ]);
    let listing = resolve_and_list(root, Backend::Jj).unwrap();
    assert!(
        !listing
            .entries
            .iter()
            .find(|e| e.name == "main")
            .unwrap()
            .open
    );
    assert!(
        listing
            .entries
            .iter()
            .find(|e| e.name == "feature")
            .unwrap()
            .open
    );
    mock.assert_complete();
}

#[test]
fn unrelated_broken_jj_never_blocks_git_checkout_ownership() {
    let repo = GitFixture::new();
    let broken = repo.root.parent().unwrap().join("broken");
    fs::create_dir(&broken).unwrap();
    fs::write(broken.join(".jj"), "invalid marker").unwrap();
    let mock = MockCli::new(vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [{"workspace_id": "bad"}, {"workspace_id": "good"}]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "bad"],
            json!({"result": {"panes": [{"cwd": broken}]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "good"],
            json!({"result": {"panes": [{"cwd": repo.linked.join("src")}]}}),
        ),
    ]);
    fs::create_dir(repo.linked.join("src")).unwrap();
    let opened = open_or_create_checkout(&repo.existing(), false).unwrap();
    assert_eq!(opened.workspace_id, "good");
    mock.assert_complete();
}

#[test]
fn broken_candidate_reports_workspace_and_path_without_hiding_git() {
    let repo = GitFixture::new();
    let broken = repo.root.parent().unwrap().join("broken-jj");
    fs::create_dir(&broken).unwrap();
    fs::write(broken.join(".jj"), "invalid marker").unwrap();
    let root = repo.root.to_str().unwrap();
    let mock = MockCli::new(vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [
                {"workspace_id": "bad", "label": "unavailable jj", "number": 1,
                    "worktree": {"repo_root": broken, "checkout_path": broken}},
                {"workspace_id": "good", "label": "Git", "number": 2}
            ]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "bad"],
            json!({"result": {"panes": [{"cwd": broken}]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "good"],
            json!({"result": {"panes": [{"cwd": root}]}}),
        ),
    ]);
    let inventory = list_projects(Backend::Jj).unwrap();
    assert_eq!(inventory.projects.len(), 1);
    assert_eq!(inventory.projects[0].backend, Backend::Git);
    assert_eq!(inventory.projects[0].root, root);
    assert!(inventory
        .warnings
        .iter()
        .any(|w| w.contains("bad") && w.contains(broken.to_str().unwrap())));
    mock.assert_complete();
}

#[test]
fn unavailable_pane_and_metadata_candidates_preserve_healthy_project() {
    let repo = GitFixture::new();
    let root = repo.root.to_str().unwrap();
    let mock = MockCli::new(vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [
                {"workspace_id": "bad", "label": "Unavailable"},
                {"workspace_id": "good", "label": "Git"}
            ]}}),
        ),
        Step::error(&["pane", "list", "--workspace", "bad"], "pane_unavailable"),
        Step::error(
            &["worktree", "list", "--workspace", "bad"],
            "metadata_unavailable",
        ),
        Step::ok(
            &["pane", "list", "--workspace", "good"],
            json!({"result": {"panes": [{"cwd": root}]}}),
        ),
    ]);
    let inventory = list_projects(Backend::Jj).unwrap();
    assert_eq!(inventory.projects.len(), 1);
    assert_eq!(inventory.projects[0].root, root);
    assert_eq!(inventory.warnings.len(), 2);
    assert!(inventory.warnings.iter().all(|w| w.contains("bad")));
    mock.assert_complete();
}

#[test]
fn genuinely_empty_inventory_and_nonrepository_cwd_are_not_failures() {
    let repo = GitFixture::new();
    let empty = repo.root.parent().unwrap().to_str().unwrap();
    let mock = MockCli::new(vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [{"workspace_id": "empty"}]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "empty"],
            json!({"result": {"panes": [{"cwd": empty}]}}),
        ),
        Step::ok(
            &["worktree", "list", "--workspace", "empty"],
            json!({"result": {"worktrees": []}}),
        ),
    ]);
    let inventory = list_projects(Backend::Jj).unwrap();
    assert!(inventory.projects.is_empty());
    assert!(inventory.warnings.is_empty());
    mock.assert_complete();
}

#[test]
fn all_failed_candidates_return_actionable_error_not_empty_projects() {
    let repo = GitFixture::new();
    let broken = repo.root.parent().unwrap().join("broken-jj");
    fs::create_dir(&broken).unwrap();
    fs::write(broken.join(".jj"), "invalid marker").unwrap();
    let mock = MockCli::new(vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [{"workspace_id": "bad", "label": "Broken jj",
                "worktree": {"repo_root": broken, "checkout_path": broken}}]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "bad"],
            json!({"result": {"panes": [{"cwd": broken}]}}),
        ),
    ]);
    let error = list_projects(Backend::Jj).unwrap_err();
    assert!(error.contains("bad") && error.contains(broken.to_str().unwrap()));
    mock.assert_complete();
}

#[test]
fn global_workspace_list_failure_stays_fatal() {
    let mock = MockCli::new(vec![Step::error(
        &["workspace", "list"],
        "workspace_inventory_unavailable",
    )]);
    assert!(list_projects(Backend::Jj)
        .unwrap_err()
        .contains("workspace_inventory_unavailable"));
    mock.assert_complete();
}

#[test]
fn jj_native_failure_never_attempts_ui_or_agent_and_keeps_partial_checkout() {
    let Some(repo) = crate::checkout::tests::Fixture::new(false) else {
        return;
    };
    let checkout_path = repo.dir.join("native-error");
    let request = repo.request(
        "native-error",
        &checkout_path,
        "missing-revision-herdr-test",
    );
    let mock = MockCli::new(vec![]);
    let mut options = opts("tab");
    options.checkout = Some(request.into());
    let error = start_agent(&options).unwrap_err();
    assert!(error.contains("missing-revision-herdr-test"));
    if checkout_path.exists() {
        assert!(checkout_path.join(".jj").exists());
    }
    assert!(start_agent(&options).is_err());
    mock.assert_complete();
}

#[test]
fn metadata_checkout_path_marks_selected_root_when_pane_is_elsewhere() {
    let repo = GitFixture::new();
    let path = repo.linked.to_str().unwrap();
    let mock = MockCli::new(vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [
                {"workspace_id": "w1", "worktree": {"repo_root": repo.root,
                    "checkout_path": path, "is_linked_worktree": true}}
            ]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "w1"],
            json!({"result": {"panes": [{"cwd": repo.root}]}}),
        ),
    ]);
    let opened = open_or_create_checkout(&repo.existing(), false).unwrap();
    assert_eq!(opened.workspace_id, "w1");
    assert!(opened.already_open);
    mock.assert_complete();
}

#[test]
fn missing_metadata_checkout_candidate_does_not_hide_healthy_git() {
    let repo = GitFixture::new();
    let missing = repo.root.parent().unwrap().join("missing-jj-checkout");
    let root = repo.root.to_str().unwrap();
    let mock = MockCli::new(vec![
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [
                {"workspace_id": "missing", "worktree": {
                    "repo_root": missing, "checkout_path": missing}},
                {"workspace_id": "git"}
            ]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "missing"],
            json!({"result": {"panes": []}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "git"],
            json!({"result": {"panes": [{"cwd": root}]}}),
        ),
    ]);
    let inventory = list_projects(Backend::Jj).unwrap();
    assert_eq!(inventory.projects.len(), 1);
    assert!(inventory
        .warnings
        .iter()
        .any(|w| w.contains("missing") && w.contains(missing.to_str().unwrap())));
    mock.assert_complete();
}

#[test]
fn explicit_broken_repository_is_not_replaced_with_another_project() {
    let repo = GitFixture::new();
    let broken = repo.root.parent().unwrap().join("selected-broken-jj");
    fs::create_dir(&broken).unwrap();
    fs::write(broken.join(".jj"), "invalid marker").unwrap();
    let error = resolve_and_list(broken.to_str().unwrap(), Backend::Jj).unwrap_err();
    assert!(error.contains("jj") && error.contains(broken.to_str().unwrap()));
}

#[test]
fn zero_workspaces_is_an_empty_successful_inventory() {
    let mock = MockCli::new(vec![Step::ok(
        &["workspace", "list"],
        json!({"result": {"workspaces": []}}),
    )]);
    let inventory = list_projects(Backend::Jj).unwrap();
    assert!(inventory.projects.is_empty());
    assert!(inventory.warnings.is_empty());
    mock.assert_complete();
}

#[test]
fn listing_rejects_ambiguous_owners_at_the_exact_root() {
    let repo = GitFixture::new();
    let root = repo.root.to_str().unwrap();
    let linked = repo.linked.to_str().unwrap();
    let subdir = repo.linked.join("src");
    fs::create_dir(&subdir).unwrap();
    let mock = MockCli::new(vec![
        Step::ok(
            &["worktree", "list", "--cwd", root],
            json!({"result": {"worktrees": [
                {"path": root, "branch": "refs/heads/main"},
                {"path": linked, "branch": "refs/heads/feature", "is_linked_worktree": true}
            ]}}),
        ),
        Step::ok(
            &["workspace", "list"],
            json!({"result": {"workspaces": [
                {"workspace_id": "w1"}, {"workspace_id": "w2"}
            ]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "w1"],
            json!({"result": {"panes": [{"cwd": linked}]}}),
        ),
        Step::ok(
            &["pane", "list", "--workspace", "w2"],
            json!({"result": {"panes": [{"cwd": subdir}]}}),
        ),
    ]);
    let error = resolve_and_list(root, Backend::Jj).unwrap_err();
    assert!(error.contains("multiple") && error.contains(linked));
    mock.assert_complete();
}

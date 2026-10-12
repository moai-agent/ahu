//! Offline synthetic subprocesses and indexed metadata. No Git worktrees.
mod common;
use common::plugin::Fixture;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

const FIRST: &str = "00000000-0000-7000-8000-000000000001";
const SECOND: &str = "00000000-0000-7000-8000-000000000002";

fn fixture(mode: &str) -> Fixture {
    // Interpreter startup under parallel test load must not consume the process budget.
    Fixture::with_timeout(mode, 5000)
}

fn command(f: &Fixture) -> Command {
    let mut cmd = common::ahu();
    cmd.current_dir(&f.root)
        .args(["plugins", "invoke", "fixture", "--input-file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}
fn send(mut cmd: Command, input: &[u8]) -> Output {
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    // Early policy refusal may close the pipe before input is sent.
    let _ = stdin.write_all(input);
    drop(stdin);
    child.wait_with_output().unwrap()
}
fn failure(output: Output, code: &str, exit: i32, unknown: bool) {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "expected {code}; stdout bytes: {}, stderr bytes: {}",
        output.stdout.len(),
        output.stderr.len()
    );
    assert!(output.stdout.is_empty(), "{output:?}");
    let expected =
        json!({"schema_version":1,"status":"error","code":code,"outcome_may_be_unknown":unknown});
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).unwrap(),
        expected
    );
    assert_eq!(output.stderr.iter().filter(|b| **b == b'\n').count(), 1);
}
fn success(output: Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    assert!(output.stdout.len() <= 65536);
    assert!(output.stdout.iter().all(|b| b.is_ascii()));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["status"], "ok");
    value["result"].clone()
}

#[test]
fn generic_stdin_file_repo_selection_and_terminal_escaping() {
    let f = fixture("normal");
    f.install();
    let input =
        json!({"literal":"$(touch no); `echo no`", "control":"\u{1b}\u{7f}\u{85}\u{202e}🦀"});
    let result = success(send(command(&f), input.to_string().as_bytes()));
    assert_eq!(result["input"], input);
    assert!(ahu::task::is_canonical_task_uuid(
        result["context"]["invocation_id"].as_str().unwrap()
    ));
    f.stopped();
    for name in ["input.json", "--color=always", "--help"] {
        fs::write(f.root.join(name), input.to_string()).unwrap();
        let output = common::ahu()
            .current_dir("/tmp")
            .arg("--repo")
            .arg(&f.root)
            .args(["plugins", "invoke", "fixture", "--input-file", name])
            .output()
            .unwrap();
        assert_eq!(success(output)["input"], input);
    }
    assert!(!f.root.join("no").exists());
}

#[test]
fn policy_and_operator_gates_precede_input_and_installation() {
    let mut f = fixture("normal");
    failure(send(command(&f), b"null"), "unavailable", 4, false);
    f.install();
    for marker in [
        "AHU_TASK_ID",
        "AHU_TASK_DIR",
        "AHU_WORKER_SESSION",
        "AHU_PARENT_TASK",
        "AHU_BROKER_TOKEN",
    ] {
        let mut cmd = command(&f);
        cmd.env(marker, "");
        failure(send(cmd, b"null"), "denied", 4, false);
    }
    f.binding.enabled = false;
    f.save(true);
    let output = common::ahu()
        .current_dir(&f.root)
        .args([
            "plugins",
            "invoke",
            "fixture",
            "--input-file",
            "/missing/SECRET",
        ])
        .output()
        .unwrap();
    failure(output, "denied", 4, false);
    f.binding.enabled = true;
    f.save(false);
    failure(send(command(&f), b"null"), "denied", 4, false);
    assert!(!f.root.join("started").exists());
    failure(
        common::ahu()
            .args([
                "--repo",
                "/missing/SECRET",
                "plugins",
                "invoke",
                "fixture",
                "--input-file",
                "-",
            ])
            .output()
            .unwrap(),
        "unavailable",
        4,
        false,
    );
}

#[test]
fn input_refuses_nonregular_symlink_oversize_and_non_strict_json() {
    let f = fixture("normal");
    f.install();
    let invalid = [
        b"".to_vec(),
        b"{} {}".to_vec(),
        b"{\"x\":1,\"x\":2}".to_vec(),
        vec![b' '; 65537],
        format!("{}0{}", "[".repeat(200), "]".repeat(200)).into_bytes(),
        vec![255],
    ];
    for input in invalid {
        failure(send(command(&f), &input), "invalid_input", 2, false);
    }
    fs::write(f.root.join("file"), "null").unwrap();
    std::os::unix::fs::symlink(f.root.join("file"), f.root.join("link")).unwrap();
    let fifo = std::ffi::CString::new(f.root.join("fifo").to_str().unwrap()).unwrap();
    // SAFETY: synthetic, owned path and fixed mode.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    for name in ["link", "fifo", ".", "missing", "/dev/null"] {
        failure(
            common::ahu()
                .current_dir(&f.root)
                .args(["plugins", "invoke", "fixture", "--input-file", name])
                .output()
                .unwrap(),
            "invalid_input",
            2,
            false,
        );
    }
    assert!(!f.root.join("started").exists());
}

#[test]
fn protocol_process_and_complete_output_failures_are_sanitized() {
    for (mode, code) in [
        ("malformed", "protocol"),
        ("duplicate", "protocol"),
        ("remote_error", "failed"),
        ("nonzero", "failed"),
        ("overflow", "output_limit"),
        ("early_output", "output_limit"),
        ("never_read", "timed_out"),
        ("inherited_pipe", "timed_out"),
        ("unicode_expansion", "output_limit"),
        ("cli_framing_limit", "output_limit"),
    ] {
        let f = fixture(mode);
        f.install();
        failure(send(command(&f), b"null"), code, 5, true);
        f.stopped();
    }
    let f = fixture("stderr");
    f.install();
    success(send(command(&f), b"null"));
}

fn wait_started(f: &Fixture) {
    let until = Instant::now() + Duration::from_secs(5);
    while !f.root.join("started").exists() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn signals_cancel_stalled_intake_and_running_process_without_leaks() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        for started in [false, true] {
            let f = fixture("hang");
            f.install();
            let mut child = command(&f).spawn().unwrap();
            let mut input = child.stdin.take().unwrap();
            if started {
                input.write_all(b"null").unwrap();
                drop(input);
                wait_started(&f);
            } else {
                std::thread::sleep(Duration::from_millis(300));
            }
            // SAFETY: signal only the synthetic ahu subprocess owned by this test.
            assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
            failure(child.wait_with_output().unwrap(), "cancelled", 5, started);
            if started {
                f.stopped();
            } else {
                assert!(!f.root.join("started").exists());
            }
        }
    }
}

#[test]
fn broken_stdout_is_fixed_io_with_unknown_outcome() {
    let f = fixture("normal");
    f.install();
    let mut child = command(&f).spawn().unwrap();
    drop(child.stdout.take());
    let mut input = child.stdin.take().unwrap();
    input.write_all(b"null").unwrap();
    drop(input);
    failure(child.wait_with_output().unwrap(), "io", 5, true);
    f.stopped();
}

#[test]
fn parser_rejects_scope_widening_and_bad_flags_without_echoing_values() {
    let f = fixture("normal");
    for extra in [
        vec!["--allow-task", FIRST],
        vec![
            "--task-observations",
            "--allow-task",
            FIRST,
            "--allow-task",
            FIRST,
        ],
        vec!["--task-observations", "--allow-task", "SECRET"],
        vec!["--input-file", "SECRET"],
        vec!["--SECRET"],
        vec!["--task-observations", "--task-observations"],
        vec!["--task-observations", "--allow-task", "--help"],
        vec!["--task-observations", "--allow-task", "--color=always"],
    ] {
        let mut cmd = command(&f);
        cmd.args(extra);
        let output = send(cmd, b"null");
        assert_eq!(output.status.code(), Some(2));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("SECRET"));
        assert!(output.stdout.is_empty());
    }
    let output = common::ahu().args(["help", "plugins"]).output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--task-observations"));
}

fn observations(f: &Fixture, tasks: &[&str], request: Value) -> Output {
    let mut cmd = command(f);
    cmd.arg("--task-observations");
    for task in tasks {
        cmd.args(["--allow-task", task]);
    }
    send(cmd, request.to_string().as_bytes())
}

fn record(root: &std::path::Path, id: &str) -> std::path::PathBuf {
    let discovered = ahu::git::discover(root).unwrap();
    let record_source = root.to_path_buf();
    let adapter = ahu::harness::adapter_for("claude-code").unwrap();
    let command = adapter
        .launch_command(&ahu::harness::LaunchRequest {
            model: "claude-opus-5",
            prompt: "secret prompt",
            cwd: &record_source,
            permissions: Default::default(),
        })
        .unwrap();
    let enforcement = adapter
        .enforcement("claude-opus-5", Default::default())
        .unwrap();
    let record = ahu::task::TaskRecord {
        schema_version: ahu::task::TASK_SCHEMA_VERSION,
        task_id: id.to_string(),
        title: "private prompt title".to_string(),
        summary: String::new(),
        created_at: ahu::task::now_rfc3339(),
        repo_identity: discovered.identity(),
        repo_root: record_source.clone(),
        branch: "ahu/auto/gone0001".to_string(),
        worktree: record_source.clone(),
        base_commit: discovered.head.clone(),
        identity: ahu::task::LaunchIdentity {
            release_id: None,
            mode: ahu::task::LaunchMode::Automatic,
            agent: "auto".to_string(),
            agent_version: None,
            permissions: Default::default(),
            harness: "claude-code".to_string(),
            model: "claude-opus-5".to_string(),
            instructions_source: None,
            source_digest: None,
            instructions_digest: None,
            identity_digest: None,
            selection_basis: Some("test".to_string()),
        },
        policy_digest: "0".repeat(64),
        catalog_version: ahu::catalog::CATALOG_VERSION.to_string(),
        config_snapshot: Default::default(),
        config_snapshot_digest: "0".repeat(64),
        hooks: Default::default(),
        hooks_digest: String::new(),
        delivery: ahu::orchestration::deliver(None, "prompt").unwrap().1,
        prompt_digest: String::new(),
        harness_executable: std::path::PathBuf::from("claude"),
        materialize: Default::default(),
        launch_command: command,
        reliability_warning: None,
        enforcement,
        session_host: None,
        cmux_group_id: None,
        cmux_workspace_id: None,
        cmux_window_id: None,
        state: ahu::task::TaskState::Running,
    };
    let dir = root
        .join(".ahu/state/repos")
        .join(discovered.identity())
        .join("tasks")
        .join(id);
    ahu::task::save(&dir, &record, "secret prompt").unwrap();
    dir
}

fn indexed(f: &Fixture, id: &str, headless: bool) -> std::path::PathBuf {
    let mut dir = record(&f.root, id);
    let repo = ahu::git::discover(&f.root).unwrap();
    let kind = if headless {
        ahu::task_index::StoreKind::Headless
    } else {
        ahu::task_index::StoreKind::Worktree
    };
    if headless {
        let target = ahu::storage::HeadlessStore::for_repo(&repo)
            .unwrap()
            .directory
            .join(id);
        ahu::state::create_private_dir_all(&target).unwrap();
        let record = ahu::task::load(&dir).unwrap();
        ahu::task::save(&target, &record, "secret prompt").unwrap();
        dir = target;
    }
    ahu::task_index::register(&repo.identity(), id, &f.root, kind).unwrap();
    dir
}
fn read_request(id: &str) -> Value {
    json!({"version":1,"observations":[{"operation":"read","task":id}]})
}
fn file_snapshot(
    root: &std::path::Path,
) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    fn visit(
        root: &std::path::Path,
        paths: &mut std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
    ) {
        if !root.exists() {
            return;
        }
        for item in fs::read_dir(root).unwrap() {
            let path = item.unwrap().path();
            if path.is_dir() {
                paths.insert(path.clone(), Vec::new());
                visit(&path, paths);
            } else {
                paths.insert(path.clone(), fs::read(&path).unwrap());
            }
        }
    }
    let mut paths = Default::default();
    visit(root, &mut paths);
    paths
}

#[test]
fn exact_observations_ignore_unrelated_and_unused_corruption_without_state_writes() {
    for headless in [false, true] {
        let f = fixture("normal");
        f.install();
        let dir = indexed(&f, FIRST, headless);
        let corrupt = indexed(&f, SECOND, headless);
        fs::write(corrupt.join("task.json"), "SECRET invalid record").unwrap();
        let before = file_snapshot(&f.root.join(".ahu"));
        let req = json!({"version":1,"observations":[{"operation":"list","limit":1},{"operation":"read","task":FIRST},{"operation":"replay","task":FIRST,"after":0}]});
        let result = success(observations(&f, &[FIRST, SECOND], req));
        let input = &result["input"];
        assert_eq!(input["kind"], "ahu.task-observations");
        assert_eq!(input["observations"][0]["next_after"], FIRST);
        assert_eq!(
            input["observations"][0]["tasks"].as_array().unwrap().len(),
            1
        );
        assert_eq!(input["observations"][1]["snapshot"]["task"], FIRST);
        assert_eq!(
            input["observations"][2]["changes"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(!input.to_string().contains("secret"));
        assert_eq!(
            input["observations"][1]["snapshot"]
                .as_object()
                .unwrap()
                .len(),
            5
        );
        assert_eq!(file_snapshot(&f.root.join(".ahu")), before);
        fs::remove_file(f.root.join("started")).unwrap();
        failure(
            observations(
                &f,
                &[FIRST, SECOND],
                json!({"version":1,"observations":[{"operation":"list","limit":1,"after":FIRST}]}),
            ),
            "unavailable",
            4,
            false,
        );
        failure(
            observations(&f, &[FIRST], read_request(SECOND)),
            "denied",
            4,
            false,
        );
        failure(
            observations(
                &f,
                &[FIRST],
                json!({"version":1,"observations":[{"operation":"list","limit":1,"after":SECOND}]}),
            ),
            "denied",
            4,
            false,
        );
        // Full preflight denial wins over a corrupt earlier demanded task.
        fs::write(dir.join("task.json"), "invalid").unwrap();
        failure(
            observations(
                &f,
                &[FIRST],
                json!({"version":1,"observations":[{"operation":"read","task":FIRST},{"operation":"read","task":SECOND}]}),
            ),
            "denied",
            4,
            false,
        );
        assert!(!f.root.join("started").exists());
    }
}

#[test]
fn empty_scope_capabilities_and_missing_legacy_records_are_explicit() {
    let f = fixture("normal");
    f.install();
    // Even corrupt coordination storage cannot block capabilities-only requests.
    let repo = ahu::git::discover(&f.root).unwrap();
    let v1dir = ahu::storage::RepositoryStorage::new(&repo)
        .unwrap()
        .coordination_dir()
        .unwrap()
        .join("agent-tasks-v1");
    fs::create_dir_all(v1dir.parent().unwrap()).unwrap();
    fs::write(&v1dir, "invalid").unwrap();
    let caps = json!({"version":1,"observations":[]});
    assert!(
        success(observations(&f, &[FIRST], caps))["input"]["observations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let empty = json!({"version":1,"observations":[{"operation":"list","limit":1}]});
    assert!(
        success(observations(&f, &[], empty))["input"]["observations"][0]["tasks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    fs::remove_file(&v1dir).unwrap();
    fs::remove_file(f.root.join("started")).unwrap();
    failure(
        observations(&f, &[FIRST], read_request(FIRST)),
        "unavailable",
        4,
        false,
    );
    record(&f.root, FIRST); // Unindexed legacy task is deliberately unsupported.
    failure(
        observations(&f, &[FIRST], read_request(FIRST)),
        "unavailable",
        4,
        false,
    );
    for req in [
        json!({"version":2,"observations":[]}),
        json!({"version":1,"observations":[{"operation":"submit","task":FIRST}]}),
        json!({"version":1,"observations":[],"tasks":[FIRST]}),
        json!({"version":1,"observations":[{"operation":"list","limit":129}]}),
    ] {
        failure(observations(&f, &[FIRST], req), "invalid_input", 2, false);
    }
    assert!(!f.root.join("started").exists());
}

#[test]
fn hostile_pointer_record_symlink_and_v1_identity_conflict_fail_closed() {
    for variant in [
        "foreign",
        "id",
        "worktree",
        "symlink",
        "index_symlink",
        "schema",
        "v1_conflict",
        "index_schema",
        "index_repo",
        "index_checkout",
    ] {
        let f = fixture("normal");
        f.install();
        let dir = indexed(&f, FIRST, false);
        let repo = ahu::git::discover(&f.root).unwrap();
        let pointer = ahu::task_index::root_for(&repo)
            .unwrap()
            .join(format!("{FIRST}.json"));
        if matches!(variant, "index_schema" | "index_repo" | "index_checkout") {
            let mut entry: ahu::task_index::Entry =
                serde_json::from_slice(&fs::read(&pointer).unwrap()).unwrap();
            match variant {
                "index_schema" => entry.schema_version = 1,
                "index_repo" => entry.repo_identity = "foreign".into(),
                "index_checkout" => entry.checkout = "/missing/SECRET".into(),
                _ => unreachable!(),
            }
            ahu::state::write_json(&pointer, &entry).unwrap();
        } else if variant == "index_symlink" {
            let target = pointer.with_extension("saved");
            fs::rename(&pointer, &target).unwrap();
            std::os::unix::fs::symlink(target, &pointer).unwrap();
        } else if variant == "symlink" {
            let target = dir.join("saved.json");
            fs::rename(dir.join("task.json"), &target).unwrap();
            std::os::unix::fs::symlink(target, dir.join("task.json")).unwrap();
        } else if variant == "v1_conflict" {
            let core = ahu::agent_task::Controller::local(&repo).unwrap();
            let mut record = ahu::task::load(&dir).unwrap();
            core.track_existing(
                &repo,
                FIRST,
                "synthetic",
                ahu::agent_task::v1::Principal::Operator("test".into()),
                false,
            )
            .unwrap();
            record.prompt_digest = "different".into();
            ahu::task::save(&dir, &record, "secret").unwrap();
        } else {
            let mut r = ahu::task::load(&dir).unwrap();
            match variant {
                "foreign" => r.repo_identity = "foreign".into(),
                "id" => r.task_id = SECOND.into(),
                "worktree" => r.worktree = "/missing/SECRET".into(),
                "schema" => r.schema_version = 999,
                _ => unreachable!(),
            }
            ahu::task::save(&dir, &r, "secret").unwrap();
        }
        failure(
            observations(&f, &[FIRST], read_request(FIRST)),
            "unavailable",
            4,
            false,
        );
        assert!(!f.root.join("started").exists());
    }
}

#[test]
fn durable_replay_is_preserved_and_plugin_result_cannot_dispatch_actions() {
    use ahu::agent_task::{
        Controller,
        v1::{self, TaskService},
    };
    let f = fixture("mutation_result");
    f.install();
    indexed(&f, FIRST, false);
    let repo = ahu::git::discover(&f.root).unwrap();
    let core = Controller::local(&repo).unwrap();
    core.track_existing(
        &repo,
        FIRST,
        "synthetic",
        v1::Principal::Operator("test".into()),
        false,
    )
    .unwrap();
    let service = core
        .plugin_read_session("fixture", [FIRST.into()].into())
        .unwrap();
    let expected = service.replay(v1::VERSION, FIRST, 0).unwrap();
    let before = file_snapshot(&f.root.join(".ahu"));
    let output = success(observations(
        &f,
        &[FIRST],
        json!({"version":1,"observations":[{"operation":"replay","task":FIRST,"after":0}]}),
    ));
    assert_eq!(output, json!({"operation":"submit","task":FIRST}));
    assert_eq!(file_snapshot(&f.root.join(".ahu")), before);
    assert_eq!(service.replay(v1::VERSION, FIRST, 0).unwrap(), expected);
}

#[test]
fn stalled_stdin_has_a_fixed_intake_deadline_without_startup() {
    let f = fixture("normal");
    f.install();
    let start = Instant::now();
    let mut child = command(&f).spawn().unwrap();
    let input = child.stdin.take().unwrap(); // Keep the pipe open, without bytes.
    failure(child.wait_with_output().unwrap(), "timed_out", 5, false);
    assert!(start.elapsed() >= Duration::from_secs(30));
    assert!(start.elapsed() < Duration::from_secs(40));
    drop(input);
    assert!(!f.root.join("started").exists());
}

#[test]
fn blocked_stdout_has_a_bounded_delivery_deadline() {
    use std::os::fd::{AsRawFd, FromRawFd};
    let f = fixture("normal");
    f.install();
    let mut fds = [0; 2];
    // SAFETY: valid output buffer, successful pipe transfers two owned descriptors.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
    let reader = unsafe { fs::File::from_raw_fd(fds[0]) };
    let mut writer = unsafe { fs::File::from_raw_fd(fds[1]) };
    // Fill the pipe so even a small result cannot be delivered. Retain the reader.
    assert_eq!(
        unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) },
        0
    );
    while writer.write(&[0u8; 4096]).is_ok() {}
    let mut cmd = command(&f);
    cmd.stdout(Stdio::from(writer));
    let start = Instant::now();
    failure(send(cmd, b"null"), "timed_out", 5, true);
    assert!(start.elapsed() >= Duration::from_secs(30));
    assert!(start.elapsed() < Duration::from_secs(40));
    drop(reader);
    f.stopped();
}

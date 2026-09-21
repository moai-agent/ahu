mod common;
use common::TestRepo;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Output;

fn record(repo: &TestRepo, dir: PathBuf, id: &str) -> PathBuf {
    let discovered = ahu::git::discover(repo.path()).unwrap();
    let record_source = repo.path().to_path_buf();
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
        cmux_group_id: None,
        cmux_workspace_id: None,
        cmux_window_id: None,
        state: ahu::task::TaskState::Running,
    };
    ahu::task::save(&dir, &record, "secret prompt").unwrap();
    dir
}

struct Fixture {
    repo: TestRepo,
    runtime: tempfile::TempDir,
    dir: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let repo = TestRepo::new();
        let runtime = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = runtime.path().canonicalize().unwrap();
        ahu::state::write_json(
            &repo.path().join(".ahu/state/legacy-lookup.json"),
            &json!({"schema_version":1,"runtime_roots":[root]}),
        )
        .unwrap();
        let identity = ahu::git::discover(repo.path()).unwrap().identity();
        let dir = record(&repo, root.join(identity).join("abc1"), "abc1");
        std::fs::create_dir_all(dir.join("attempt-1")).unwrap();
        write_json(
            &dir.join("headless.json"),
            &json!({
                "schema_version":1, "options":{"background":true,"timeout_seconds":1800,"native_helpers":"disabled"},
                "harness_version":"synthetic", "executable_digest":"0", "parent_task":null,
                "depth":0,"attempt":1,"session":null,"native_controls":[],"gaps":[]
            }),
        );
        ahu::state::write_private_file(&dir.join("owner.lock"), b"").unwrap();
        Self { repo, runtime, dir }
    }
    fn run(&self, args: &[&str]) -> Output {
        common::ahu()
            .args(args)
            .current_dir(self.repo.path())
            .env("AHU_RUNTIME_DIR", self.runtime.path())
            .env("AHU_CMUX_BIN", self.runtime.path().join("no-cmux"))
            .env("AHU_EXECUTION_BACKEND", "headless")
            .output()
            .unwrap()
    }
    fn json(&self, command: &str) -> Value {
        let out = self.run(&[command, "abc1", "--output", "json", "--color=always"]);
        assert!(out.status.success(), "{:?}", out);
        assert!(!out.stdout.contains(&0x1b));
        serde_json::from_slice(&out.stdout).unwrap()
    }
    fn terminal(&self, outcome: &str) {
        write_json(
            &self.dir.join("attempt-1/result.json"),
            &json!({
                "schema_version":1,"task_id":"abc1","attempt":1,"outcome":outcome,
                "process":{"exit_code":if outcome == "succeeded" {0} else {1}},
                "harness":{"session":"native-session-1","terminal":true,"failed":outcome != "succeeded", "blockers":[]},
                "acceptance":"not assessed","completion_verified":false
            }),
        );
    }
}
fn write_json(path: &Path, value: &Value) {
    std::fs::write(path, value.to_string()).unwrap();
}
fn text(out: Output) -> String {
    assert!(out.status.success(), "{:?}", out);
    String::from_utf8(out.stdout).unwrap()
}
fn snapshot(path: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            entries.extend(snapshot(&path));
        } else {
            entries.push((path.clone(), std::fs::read(path).unwrap()));
        }
    }
    entries.sort();
    entries
}

#[test]
fn headless_controls_share_typed_task_reference_resolution() {
    let f = Fixture::new();
    f.terminal("succeeded");
    let before = snapshot(&f.dir);
    for command in ["task", "result", "wait"] {
        for reference in ["abc1", "ahu:task:abc1", "AHU:TASK:ABC1", "ahu:task:abc"] {
            let output = f.run(&[command, reference, "--output", "json"]);
            assert!(output.status.success(), "{command} {reference}: {output:?}");
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["task_id"], "abc1");
        }
        let output = f.run(&[command, "ahu:agent:abc1", "--output", "json"]);
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("expected a task reference"));
    }
    assert_eq!(snapshot(&f.dir), before);
}

#[test]
fn running_and_interrupted_are_observations_with_string_ids_and_read_only_inspection() {
    use std::os::fd::AsRawFd;
    let f = Fixture::new();
    let lock = std::fs::File::open(f.dir.join("owner.lock")).unwrap();
    // SAFETY: lock owns a live descriptor for the duration of the assertion.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let before = snapshot(&f.dir);
    let result = f.json("result");
    assert_eq!(result["task_id"], "abc1");
    assert_eq!(result["outcome"], "running");
    assert_eq!(result["review"]["liveness"], "live");
    assert_eq!(result["review"]["native_session"], Value::Null);
    let task = f.json("task");
    assert_eq!(task["execution_backend"], "headless");
    assert_eq!(task["attempt"], result["review"]);
    assert_eq!(task["session_state"], "running");
    for args in [&["tasks"][..], &["task", "abc1"], &["result", "abc1"]] {
        let out = text(f.run(args));
        assert!(out.contains("headless"), "{out}");
        assert!(out.contains("attempt   1 / running"), "{out}");
    }
    assert_eq!(snapshot(&f.dir), before);
    drop(lock);
    let result = f.json("result");
    assert_eq!(result["task_id"], "abc1");
    assert_eq!(result["outcome"], "interrupted");
    assert_eq!(result["review"]["liveness"], "stale");
    assert_eq!(f.json("task")["session_state"], "running");
    let out = text(f.run(&["result", "abc1"]));
    assert!(out.contains("no live supervisor"));
    assert!(out.contains("ahu diff ahu:task:abc1"));
    assert!(out.contains("not verified"));
    assert_eq!(snapshot(&f.dir), before);
}

#[test]
fn terminal_outcomes_show_known_session_locations_and_never_accept_work() {
    let f = Fixture::new();
    for outcome in [
        "succeeded",
        "failed",
        "cancelled",
        "timed_out",
        "capture_failed",
        "supervisor_error",
    ] {
        f.terminal(outcome);
        let value = f.json("result");
        assert_eq!(value["task_id"], "abc1");
        assert_eq!(value["review"]["outcome"], outcome);
        assert_eq!(value["review"]["native_session"], "native-session-1");
        assert_eq!(
            value["review"]["native_session_source"],
            "result.json harness.session"
        );
        assert_eq!(value["review"]["completion_verified"], false);
        for command in ["task", "result"] {
            let out = text(f.run(&[command, "abc1"]));
            for expected in [
                outcome,
                "native-session-1",
                "result.json",
                "result.md",
                "ahu result ahu:task:abc1",
                "not assessed",
            ] {
                assert!(out.contains(expected), "{expected}: {out}");
            }
        }
    }
}

#[test]
fn unavailable_specs_and_results_do_not_hide_other_tasks_or_claim_success() {
    let f = Fixture::new();
    record(&f.repo, f.dir.parent().unwrap().join("abc2"), "abc2");
    let spec = std::fs::read(f.dir.join("headless.json")).unwrap();
    for body in [None, Some("broken"), Some("{\"schema_version\":99}")] {
        if let Some(body) = body {
            std::fs::write(f.dir.join("headless.json"), body).unwrap();
        } else {
            std::fs::remove_file(f.dir.join("headless.json")).unwrap();
        }
        let task = f.json("task");
        assert_eq!(task["execution_backend"], "headless");
        assert_eq!(task["attempt"]["outcome"], "unavailable");
        let out = text(f.run(&["tasks"]));
        assert!(
            out.contains("abc1") && out.contains("abc2") && out.contains("unavailable"),
            "{out}"
        );
        assert!(!f.run(&["result", "abc1"]).status.success());
    }
    std::fs::write(f.dir.join("headless.json"), spec).unwrap();
    for body in ["broken", "{}", "{\"outcome\":\"succeeded\"}"] {
        std::fs::write(f.dir.join("attempt-1/result.json"), body).unwrap();
        assert_eq!(f.json("task")["attempt"]["outcome"], "unavailable");
        assert!(!f.run(&["result", "abc1"]).status.success());
    }
    f.terminal("succeeded");
    let path = f.dir.join("attempt-1/result.json");
    let original: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for (key, value) in [
        ("task_id", json!("abc2")),
        ("attempt", json!(2)),
        ("schema_version", json!(99)),
        ("process", json!({"exit_code":1})),
        ("harness", json!({"terminal":false,"failed":false})),
    ] {
        let mut bad = original.clone();
        bad[key] = value;
        write_json(&path, &bad);
        assert!(!f.run(&["result", "abc1"]).status.success());
        assert_eq!(f.json("task")["attempt"]["outcome"], "unavailable");
    }
}

#[test]
fn hostile_and_large_metadata_is_bounded_in_human_output_and_raw_in_json() {
    let f = Fixture::new();
    f.terminal("failed");
    let path = f.dir.join("attempt-1/result.json");
    let mut result: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let hostile = format!("native\u{1b}[2J\u{202e}\n{}", "x".repeat(4000));
    result["harness"]["session"] = hostile.clone().into();
    result["harness"]["blockers"] = json!(vec![hostile.clone(); 20]);
    result["private_record"] = "do-not-show-this-record".into();
    write_json(&path, &result);
    let task = f.json("task");
    assert_eq!(task["attempt"]["native_session"], hostile);
    assert_eq!(f.json("result")["harness"]["session"], hostile);
    assert!(!task.to_string().contains("do-not-show-this-record"));
    for command in ["task", "result"] {
        let out = text(f.run(&[command, "abc1"]));
        assert!(!out.contains(['\u{1b}', '\u{202e}']));
        assert!(out.contains("[truncated]"));
        assert!(!out.contains("secret prompt") && !out.contains("do-not-show-this-record"));
        assert!(out.len() < 14000, "{}", out.len());
    }
    std::fs::write(&path, "x".repeat(1024 * 1024 + 1)).unwrap();
    assert_eq!(f.json("task")["attempt"]["outcome"], "unavailable");
    assert!(!f.run(&["result", "abc1"]).status.success());
}

#[test]
fn unreadable_signals_and_redirected_metadata_are_explicit_and_never_modified() {
    let f = Fixture::new();
    std::fs::remove_file(f.dir.join("owner.lock")).unwrap();
    let task = f.json("task");
    assert_eq!(task["liveness"], "unknown");
    assert_eq!(task["attempt"]["outcome"], "unavailable");
    assert!(!f.dir.join("owner.lock").exists());
    f.terminal("succeeded");
    assert_eq!(f.json("task")["attempt"]["outcome"], "succeeded");
    assert_eq!(f.json("task")["liveness"], "unknown");
    let result = f.dir.join("attempt-1/result.json");
    let target = f.runtime.path().join("do-not-read");
    std::fs::rename(&result, &target).unwrap();
    std::os::unix::fs::symlink(&target, &result).unwrap();
    assert_eq!(f.json("task")["attempt"]["outcome"], "unavailable");
    assert!(!f.run(&["result", "abc1"]).status.success());
    assert!(result.symlink_metadata().unwrap().file_type().is_symlink());
}

#[test]
fn resume_session_has_provenance_and_started_metadata_is_not_a_session_source() {
    let f = Fixture::new();
    write_json(
        &f.dir.join("attempt-1/started.json"),
        &json!({
            "schema_version":1,"attempt":1,"pid":123,"session":"unverified-session"
        }),
    );
    assert!(f.json("task")["attempt"]["native_session"].is_null());
    let path = f.dir.join("headless.json");
    let mut spec: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    spec["session"] = "resume-target".into();
    write_json(&path, &spec);
    let task = f.json("task");
    assert_eq!(task["attempt"]["native_session"], "resume-target");
    assert_eq!(
        task["attempt"]["native_session_source"],
        "headless.json resume target"
    );
    f.terminal("succeeded");
    assert_eq!(
        f.json("task")["attempt"]["native_session"],
        "native-session-1"
    );
    spec["schema_version"] = 99.into();
    write_json(&path, &spec);
    let task = f.json("task");
    assert!(task["attempt"]["native_session"].is_null());
    assert_eq!(task["attempt"]["outcome"], "unavailable");
}

#[test]
fn legacy_supervisor_error_ids_are_normalized_only_for_the_matching_task() {
    let f = Fixture::new();
    let path = f.dir.join("attempt-1/result.json");
    let mut value = json!({
        "schema_version":1,"task_id":f.dir.file_name(),"attempt":1,
        "outcome":"supervisor_error","blockers":["synthetic failure"],"acceptance":"not assessed"
    });
    write_json(&path, &value);
    assert_eq!(f.json("result")["task_id"], "abc1");
    value["task_id"] = json!(Path::new("abc2").file_name());
    write_json(&path, &value);
    assert!(!f.run(&["result", "abc1"]).status.success());
}

#[test]
fn nonregular_result_metadata_is_refused_without_waiting_or_changing_it() {
    let f = Fixture::new();
    let path = f.dir.join("attempt-1/result.json");
    std::fs::create_dir(&path).unwrap();
    assert_eq!(f.json("task")["attempt"]["outcome"], "unavailable");
    assert!(!f.run(&["result", "abc1"]).status.success());
    std::fs::remove_dir(&path).unwrap();
    let name = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    // SAFETY: name is a NUL-terminated synthetic path with no existing entry.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert_eq!(f.json("task")["attempt"]["outcome"], "unavailable");
    assert!(!f.run(&["result", "abc1"]).status.success());
    use std::os::unix::fs::FileTypeExt;
    assert!(path.symlink_metadata().unwrap().file_type().is_fifo());
}

#[test]
fn inspection_size_limit_does_not_change_wait_lifecycle_results() {
    let f = Fixture::new();
    f.terminal("succeeded");
    let path = f.dir.join("attempt-1/result.json");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["agent_report"] = json!({"text":"x".repeat(1024 * 1024 + 1)});
    write_json(&path, &value);
    assert_eq!(f.json("task")["attempt"]["outcome"], "unavailable");
    let out = f.run(&["wait", "abc1", "--output", "json"]);
    assert!(out.status.success(), "{:?}", out);
    let waited: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(waited["outcome"], "succeeded");
    assert_eq!(
        waited["agent_report"]["text"],
        value["agent_report"]["text"]
    );
}

#[test]
fn wait_refuses_foreign_or_scalar_results_without_success_or_panic() {
    let f = Fixture::new();
    let path = f.dir.join("attempt-1/result.json");
    f.terminal("succeeded");
    let mut foreign: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    foreign["task_id"] = "abc2".into();
    for value in [foreign, json!("scalar"), json!([]), Value::Null] {
        write_json(&path, &value);
        let out = f.run(&["wait", "abc1"]);
        assert_eq!(out.status.code(), Some(5), "{:?}", out);
        assert!(out.stdout.is_empty(), "{:?}", out);
        assert!(String::from_utf8_lossy(&out.stderr).contains("result metadata"));
    }
}

#[test]
fn missing_corrupt_and_foreign_external_task_records_do_not_hide_other_tasks() {
    let f = Fixture::new();
    let other = f.dir.parent().unwrap().join("abc2");
    record(&f.repo, other.clone(), "abc2");
    let path = other.join("task.json");
    let mut foreign: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    foreign["task_id"] = "abc1".into();
    for body in [None, Some("broken".into()), Some(foreign.to_string())] {
        match body {
            Some(body) => std::fs::write(&path, body).unwrap(),
            None => std::fs::remove_file(&path).unwrap(),
        }
        let out = text(f.run(&["tasks"]));
        assert!(
            out.contains("abc1") && out.contains("abc2") && out.contains("unreadable"),
            "{out}"
        );
        assert_eq!(f.json("task")["task_id"], "abc1");
        let bad = f.run(&["task", "abc2"]);
        assert!(!bad.status.success());
        assert!(String::from_utf8_lossy(&bad.stderr).contains("unreadable"));
    }
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(f.dir.join("task.json"), &path).unwrap();
    let out = text(f.run(&["tasks"]));
    assert!(out.contains("abc2") && out.contains("unreadable"));
    assert_eq!(f.json("task")["task_id"], "abc1");
    assert!(path.symlink_metadata().unwrap().file_type().is_symlink());
}

#[test]
fn review_inventory_refuses_redirected_or_non_directory_task_paths() {
    let f = Fixture::new();
    let other = f.dir.parent().unwrap().join("abc2");
    record(&f.repo, other.clone(), "abc2");
    let path = other.join("task.json");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["title"] = "outside-record-sentinel".into();
    write_json(&path, &value);
    let outside = f.runtime.path().join("detached");
    std::fs::rename(&other, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &other).unwrap();
    let out = text(f.run(&["tasks"]));
    assert!(out.contains("abc1") && out.contains("abc2") && out.contains("unreadable"));
    assert!(!out.contains("outside-record-sentinel"));
    assert_eq!(f.json("task")["task_id"], "abc1");
    assert!(other.symlink_metadata().unwrap().file_type().is_symlink());
    std::fs::remove_file(&other).unwrap();
    std::fs::write(&other, b"not a directory").unwrap();
    let out = text(f.run(&["tasks"]));
    assert!(out.contains("abc1") && out.contains("abc2") && out.contains("unreadable"));
    assert_eq!(std::fs::read(&other).unwrap(), b"not a directory");
}

#[test]
fn legacy_resume_refuses_without_rewriting_or_migrating_records() {
    let f = Fixture::new();
    f.terminal("succeeded");
    let prompt = f.runtime.path().join("followup.txt");
    std::fs::write(&prompt, "synthetic continuation").unwrap();
    let before = snapshot(f.runtime.path());
    let out = f.run(&[
        "resume",
        "abc1",
        "--prompt-file",
        prompt.to_str().unwrap(),
        "--output",
        "json",
    ]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("original runner"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(snapshot(f.runtime.path()), before);
    let repo = ahu::git::discover(f.repo.path()).unwrap();
    assert!(!ahu::headless::store(&repo).unwrap().exists());
}

#[test]
fn review_regression_session_checkpoint_requires_bound_supported_evidence() {
    let f = Fixture::new();
    let valid = json!({"schema_version":2,"task_id":"abc1","attempt":1,"harness":"claude-code","session":"native-session","native_data_location":null});
    for (field, invalid) in [
        ("schema_version", json!(999)),
        ("schema_version", json!(1)),
        ("task_id", json!("another-task")),
        ("attempt", json!(2)),
        ("harness", json!("codex")),
        ("session", json!("")),
        ("session", json!("   ")),
        ("session", json!(42)),
        ("unexpected", json!("field")),
        ("session", json!("x".repeat(4097))),
        ("session", json!("bad\nsession")),
    ] {
        let mut checkpoint = valid.clone();
        checkpoint[field] = invalid;
        write_json(&f.dir.join("attempt-1/native-session.json"), &checkpoint);
        let out = f.json("result");
        assert!(out["review"]["native_session"].is_null(), "{field}: {out}");
        assert!(
            out["review"]["blockers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|b| b.as_str().unwrap_or("").contains("checkpoint unavailable")),
            "{out}"
        );
    }
    write_json(&f.dir.join("attempt-1/native-session.json"), &valid);
    let out = f.json("result");
    assert_eq!(out["review"]["native_session"], "native-session");
}

#[test]
fn review_regression_hardlinked_metadata_is_unavailable() {
    let f = Fixture::new();
    f.terminal("succeeded");
    let result = f.dir.join("attempt-1/result.json");
    std::fs::hard_link(&result, f.dir.join("copied-result.json")).unwrap();
    let out = f.run(&["result", "abc1", "--output", "json"]);
    assert!(!out.status.success(), "{out:?}");
    let out = f.json("task");
    assert_eq!(out["attempt"]["outcome"], "unavailable", "{out}");
}

#[test]
fn hardlinked_live_lock_is_unknown_and_wait_exits_promptly() {
    use std::os::fd::AsRawFd;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let f = Fixture::new();
    let held = f.runtime.path().join("live-owner.lock");
    ahu::state::write_private_file(&held, b"").unwrap();
    let file = std::fs::File::open(&held).unwrap();
    // SAFETY: file owns a valid descriptor for the duration of the test.
    assert_eq!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    std::fs::remove_file(f.dir.join("owner.lock")).unwrap();
    std::fs::hard_link(&held, f.dir.join("owner.lock")).unwrap();
    let value = f.json("task");
    assert_eq!(value["attempt"]["liveness"], "unknown");
    assert!(value["attempt"]["supervisor_owned"].is_null());
    let mut child = common::ahu()
        .current_dir(f.repo.path())
        .args(["wait", "abc1", "--output", "json"])
        .env("AHU_EXECUTION_BACKEND", "headless")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("wait borrowed another task's lock indefinitely");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("liveness unknown"));
}

#[test]
fn task_record_and_spec_hardlinks_are_refused_without_rewriting_legacy_data() {
    for name in ["task.json", "headless.json"] {
        let f = Fixture::new();
        let before = std::fs::read(f.dir.join(name)).unwrap();
        std::fs::hard_link(f.dir.join(name), f.dir.join("alias.json")).unwrap();
        assert!(
            !f.run(&["result", "abc1", "--output", "json"])
                .status
                .success()
        );
        assert_eq!(std::fs::read(f.dir.join(name)).unwrap(), before);
    }
}

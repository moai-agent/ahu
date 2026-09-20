mod common;
use common::{TestRepo, git};
use std::process::Output;

#[test]
#[cfg(unix)]
fn cross_window_liveness_requires_complete_enumeration_before_reconciliation() {
    use std::os::unix::fs::PermissionsExt;
    for case in ["live", "closed", "unreadable", "malformed"] {
        let repo = TestRepo::new();
        let dir = record(&repo, "crosswindow");
        let path = dir.join("task.json");
        let mut saved: ahu::task::TaskRecord =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        saved.cmux_workspace_id = Some("task-in-b".into());
        saved.cmux_window_id = Some("window-b".into());
        ahu::state::write_json(&path, &saved).unwrap();
        let cmux = repo.state_path().join("cmux");
        std::fs::write(&cmux, r#"#!/usr/bin/env python3
import json,os,sys
if sys.argv[1] == 'ping': print('PONG'); sys.exit(0)
method,params = sys.argv[2],json.loads(sys.argv[3])
case = os.environ['AHU_TEST_WINDOW_CASE']
if method == 'window.list': print(json.dumps({'windows':[{'id':'window-a'},{'id':'window-b'}]}))
elif method == 'workspace.list':
 if params['window_id'] == 'window-b' and case == 'unreadable': sys.exit(1)
 if params['window_id'] == 'window-b' and case == 'malformed': print('{}')
 else: print(json.dumps({'workspaces':[{'id':'task-in-b','current_directory':'/tmp'}] if params['window_id']=='window-b' and case=='live' else []}))
else: raise AssertionError(method)
"#).unwrap();
        std::fs::set_permissions(&cmux, std::fs::Permissions::from_mode(0o700)).unwrap();
        let run = |args: &[&str]| {
            common::ahu()
                .args(args)
                .current_dir(repo.path())
                .env("CMUX_WORKSPACE_ID", "caller-in-a")
                .env("AHU_CMUX_BIN", &cmux)
                .env("AHU_TEST_WINDOW_CASE", case)
                .output()
                .unwrap()
        };
        let listed = run(&["tasks"]);
        assert!(
            listed.status.success(),
            "{}",
            String::from_utf8_lossy(&listed.stderr)
        );
        let persisted: ahu::task::TaskRecord =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            persisted.state,
            if case == "closed" {
                ahu::task::TaskState::Exited
            } else {
                ahu::task::TaskState::Running
            }
        );
        let detail = run(&["task", "crosswindow", "--output", "json"]);
        assert!(
            detail.status.success(),
            "{}",
            String::from_utf8_lossy(&detail.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&detail.stdout).unwrap();
        assert_eq!(
            value["liveness"],
            match case {
                "live" => "live",
                "closed" => "stale",
                _ => "unknown",
            }
        );
    }
}

fn record(repo: &TestRepo, id: &str) -> std::path::PathBuf {
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
    let dir = repo
        .path()
        .join(".ahu/state/repos")
        .join(discovered.identity())
        .join("tasks")
        .join(id);
    ahu::task::save(&dir, &record, "secret prompt").unwrap();
    dir
}

fn run(repo: &TestRepo, args: &[&str]) -> Output {
    common::ahu()
        .args(args)
        .current_dir(repo.path())
        .env("AHU_CMUX_BIN", repo.state_path().join("missing-cmux"))
        .output()
        .unwrap()
}

#[test]
#[cfg(unix)]
fn focus_by_handle_selects_the_bound_workspace_and_json_keeps_canonical_identity() {
    use std::os::unix::fs::PermissionsExt;
    let repo = TestRepo::new();
    let id = ahu::task::new_task_id().unwrap();
    let dir = record(&repo, &id);
    let path = dir.join("task.json");
    let mut saved: ahu::task::TaskRecord =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    saved.cmux_workspace_id = Some("only-this-workspace".into());
    ahu::state::write_json(&path, &saved).unwrap();
    let discovered = ahu::git::discover(repo.path()).unwrap();
    ahu::task_handles::reserve(&discovered, &id, Some("review"), "").unwrap();
    let cmux = repo.state_path().join("cmux");
    std::fs::write(
        &cmux,
        r#"#!/usr/bin/env python3
import json,sys
if sys.argv[1] == 'ping': print('PONG'); sys.exit(0)
assert sys.argv[1:3] == ['rpc','workspace.select']
assert json.loads(sys.argv[3]) == {'workspace_id':'only-this-workspace'}
print('{}')
"#,
    )
    .unwrap();
    std::fs::set_permissions(&cmux, std::fs::Permissions::from_mode(0o700)).unwrap();
    let focused = common::ahu()
        .args(["focus", "@review"])
        .current_dir(repo.path())
        .env("AHU_CMUX_BIN", &cmux)
        .output()
        .unwrap();
    assert!(
        focused.status.success(),
        "{}",
        String::from_utf8_lossy(&focused.stderr)
    );
    let inspected = run(&repo, &["task", "@review", "--output", "json"]);
    assert!(inspected.status.success());
    let value: serde_json::Value = serde_json::from_slice(&inspected.stdout).unwrap();
    assert_eq!(value["task_id"], id);
    assert_eq!(value["task_ref"], format!("ahu:task:{id}"));
    assert_eq!(value["task_handle"], "@review");
    assert_eq!(value["repo_identity"], discovered.identity());
    assert_eq!(value["harness_executable"], "claude");
    let human = run(&repo, &["task", "@review"]);
    assert!(String::from_utf8_lossy(&human.stdout).contains(&format!("@review (ahu:task:{id})")));
}

fn run_with_runtime(repo: &TestRepo, runtime: &std::path::Path, args: &[&str]) -> Output {
    ahu::state::write_json(
        &repo.path().join(".ahu/state/legacy-lookup.json"),
        &serde_json::json!({"schema_version":1,"runtime_roots":[runtime.canonicalize().unwrap()]}),
    )
    .unwrap();
    common::ahu()
        .args(args)
        .current_dir(repo.path())
        .env("AHU_CMUX_BIN", repo.state_path().join("missing-cmux"))
        .output()
        .unwrap()
}

#[test]
fn task_json_is_small_unstyled_and_available_without_cmux_or_worktree() {
    let repo = TestRepo::new();
    let dir = record(&repo, "abc1");
    let path = dir.join("task.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["branch"] = "hostile\u{1b}[2Jbranch".into();
    value["worktree"] = repo.path().join("missing").to_str().unwrap().into();
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let before = std::fs::read(&path).unwrap();
    let output = run(
        &repo,
        &["task", "abc", "--output", "json", "--color=always"],
    );
    assert!(output.status.success(), "{:?}", output);
    assert!(output.stderr.is_empty());
    assert!(!output.stdout.contains(&0x1b));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["session_state"], "running");
    assert_eq!(json["state_source"], "record");
    assert_eq!(json["completion_verified"], false);
    assert_eq!(json["worktree_exists"], false);
    assert_eq!(json["branch"], value["branch"]);
    assert_eq!(std::fs::read(path).unwrap(), before);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(!text.contains("secret prompt"));
    assert!(!text.contains("private prompt title"));
    let human = run(&repo, &["task", "abc1"]);
    assert!(!human.stdout.contains(&0x1b));
    assert!(
        String::from_utf8(human.stdout)
            .unwrap()
            .contains("[session running]")
    );
    let diff = run(&repo, &["diff", "abc1"]);
    assert_eq!(diff.status.code(), Some(5));
}

#[test]
fn lookup_rejects_ambiguity_including_unreadable_records_and_accepts_exact_ids() {
    let repo = TestRepo::new();
    record(&repo, "abc1");
    let bad = record(&repo, "abc2");
    std::fs::write(bad.join("task.json"), "broken").unwrap();
    for command in ["task", "diff"] {
        let out = run(&repo, &[command, "abc"]);
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("ambiguous"));
        assert_eq!(run(&repo, &[command, "abc2"]).status.code(), Some(5));
        assert_eq!(run(&repo, &[command, "missing"]).status.code(), Some(2));
        assert_eq!(run(&repo, &[command]).status.code(), Some(2));
        assert_eq!(
            run(&repo, &[command, "abc1", "--output", "yaml"])
                .status
                .code(),
            Some(2)
        );
    }
    record(&repo, "abc12");
    assert!(run(&repo, &["task", "abc1"]).status.success());
}

#[test]
fn diff_covers_base_to_working_tree_without_executing_helpers_or_staging() {
    let repo = TestRepo::new();
    record(&repo, "abc1");
    repo.write("committed.txt", "committed change\n");
    repo.commit("task change");
    repo.write("staged.txt", "staged change\n");
    git(repo.path(), &["add", "staged.txt"]);
    repo.write("README.md", "unstaged change\n");
    repo.write("untracked.txt", "untracked change\n");
    // An external helper would fail the test if invoked.
    git(
        repo.path(),
        &["config", "diff.external", "/nonexistent-diff-helper"],
    );
    git(
        repo.path(),
        &["config", "diff.fixture.textconv", "/nonexistent-textconv"],
    );
    repo.write(".gitattributes", "*.txt diff=fixture\n");
    let before = git(repo.path(), &["status", "--porcelain"]);
    let output = run(&repo, &["diff", "abc1"]);
    assert!(output.status.success(), "{:?}", output);
    let patch = String::from_utf8(output.stdout).unwrap();
    for expected in ["+committed change", "+staged change", "+unstaged change"] {
        assert!(patch.contains(expected), "{patch}");
    }
    assert!(!patch.contains("untracked change"));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Untracked (not included in diff): untracked.txt")
    );
    assert_eq!(git(repo.path(), &["status", "--porcelain"]), before);
}

#[test]
fn diff_refuses_foreign_checkouts_and_option_like_bases() {
    let repo = TestRepo::new();
    let foreign = TestRepo::new();
    let path = record(&repo, "abc1").join("task.json");
    let original: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for (key, value) in [
        ("worktree", foreign.path().to_str().unwrap()),
        ("base_commit", "--output=unexpected"),
    ] {
        let mut record = original.clone();
        record[key] = value.into();
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert_eq!(run(&repo, &["diff", "abc1"]).status.code(), Some(5));
    }
}

// The files a finished headless attempt leaves in the runtime store: the
// frozen spec naming its attempt directory, and that attempt's result
// envelope carrying the recorded outside-worktree writes.
fn plant_outside_writes(store: &std::path::Path, id: &str, outside: &[&str]) {
    let dir = store.join(id);
    std::fs::write(
        dir.join("headless.json"),
        serde_json::json!({
            "schema_version": 1,
            "options": {
                "background": false,
                "timeout_seconds": 1800,
                "native_helpers": "disabled"
            },
            "harness_version": "test",
            "executable_digest": "0",
            "parent_task": null,
            "depth": 0,
            "attempt": 1,
            "session": null,
            "native_controls": [],
            "gaps": []
        })
        .to_string(),
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("attempt-1")).unwrap();
    std::fs::write(
        dir.join("attempt-1").join("result.json"),
        serde_json::json!({ "writes_outside_worktree": outside }).to_string(),
    )
    .unwrap();
}

#[test]
fn diff_discloses_recorded_outside_writes_only_when_the_worktree_diff_is_empty() {
    let repo = TestRepo::new();
    let identity = ahu::git::discover(repo.path()).unwrap().identity();
    let runtime = tempfile::TempDir::new().unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(runtime.path()).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(runtime.path(), perms).unwrap();
    }
    let store = runtime.path().join(&identity);
    std::fs::create_dir_all(&store).unwrap();
    for id in ["abc1", "abc2"] {
        let internal = record(&repo, id);
        // The checkout store wins ties in `task::list`, so the record has to
        // move, not be copied: a leftover internal entry would shadow the
        // runtime one and the diff would never read the planted files.
        std::fs::rename(&internal, store.join(id)).unwrap();
    }
    let escaped = runtime.path().join("escaped.txt");
    plant_outside_writes(&store, "abc1", &[escaped.to_str().unwrap()]);
    plant_outside_writes(&store, "abc2", &[]);

    let out = run_with_runtime(&repo, runtime.path(), &["diff", "abc1"]);
    assert!(out.status.success(), "{:?}", out);
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(
            "No changes in the task worktree, but write tool calls in the recorded event stream targeted paths outside it:"
        ),
        "{stderr}"
    );
    assert!(stderr.contains("escaped.txt"), "{stderr}");
    assert!(
        stderr.contains("Run `ahu result ahu:task:abc1` for the full recorded list."),
        "{stderr}"
    );

    let out = run_with_runtime(&repo, runtime.path(), &["diff", "abc2"]);
    assert!(out.status.success(), "{:?}", out);
    assert!(out.stdout.is_empty());
    assert!(out.stderr.is_empty());

    repo.write("README.md", "unstaged change\n");
    let out = run_with_runtime(&repo, runtime.path(), &["diff", "abc1"]);
    assert!(out.status.success(), "{:?}", out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("+unstaged change"), "{stdout}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("No changes in the task worktree"),
        "{stderr}"
    );
}

#[test]
fn old_records_without_summary_remain_readable_and_new_summaries_round_trip() {
    let repo = TestRepo::new();
    let dir = record(&repo, "old1");
    let path = dir.join("task.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value.as_object_mut().unwrap().remove("summary");
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let mut loaded = ahu::task::load(&dir).unwrap();
    assert!(loaded.summary.is_empty());
    loaded.summary = "Useful summary".into();
    ahu::task::save(&dir, &loaded, "secret prompt").unwrap();
    assert_eq!(ahu::task::load(&dir).unwrap().summary, "Useful summary");
}

// Exercise the real CLI against a disposable cmux executable. Changes to a
// record model the supervisor acknowledgement; these are protocol tests, not
// proof of OS liveness or protection against a same-user record writer.
fn interactive_cancel_case(case: &str) {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};
    let repo = TestRepo::new();
    let dir = record(&repo, "cancel-fixture");
    let mut saved = ahu::task::load(&dir).unwrap();
    saved.cmux_workspace_id = Some("owned-fixture".into());
    if case == "terminal" {
        saved.state = ahu::task::TaskState::Cancelled;
    }
    ahu::task::save(&dir, &saved, "synthetic input").unwrap();
    let cmux = repo.state_path().join("cmux");
    let calls = repo.state_path().join("calls");
    std::fs::write(&cmux, "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$AHU_TEST_CMUX_CALLS\"\nprintf '{\"ok\":true,\"result\":{}}\\n'\n").unwrap();
    std::fs::set_permissions(&cmux, std::fs::Permissions::from_mode(0o700)).unwrap();
    let transition = match case {
        "confirmed" => Some(ahu::task::TaskState::Cancelled),
        "finished" => Some(ahu::task::TaskState::Exited),
        _ => None,
    };
    let writer = if transition.is_some() || case == "unreadable" {
        let dir = dir.clone();
        Some(std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !dir.join("cancel.json").exists() {
                assert!(Instant::now() < deadline, "cancel request never arrived");
                std::thread::sleep(Duration::from_millis(10));
            }
            if let Some(state) = transition {
                ahu::task::set_state(&dir, state).unwrap();
            } else {
                std::fs::write(dir.join("task.json"), b"invalid").unwrap();
            }
        }))
    } else {
        None
    };
    let output = common::ahu()
        .args(["cancel", "cancel-fixture", "--output", "json"])
        .current_dir(repo.path())
        .env("AHU_CMUX_BIN", &cmux)
        .env("AHU_TEST_CMUX_CALLS", &calls)
        .output()
        .unwrap();
    if let Some(writer) = writer {
        writer.join().unwrap();
    }
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let cancellation = match case {
        "confirmed" => "confirmed",
        "finished" => "finished-on-its-own",
        "terminal" => "already-terminal",
        _ => "requested-unconfirmed",
    };
    assert_eq!(value["cancellation"], cancellation);
    if case == "confirmed" {
        assert_eq!(value["workspace"], "closed");
        let calls = std::fs::read_to_string(calls).unwrap();
        let close: Vec<_> = calls
            .lines()
            .filter(|line| line.contains("workspace.close"))
            .collect();
        assert_eq!(close.len(), 1, "{calls}");
        assert!(close[0].contains("owned-fixture"), "{calls}");
    } else {
        assert_eq!(value["workspace"], "left open");
        assert!(
            !calls.exists(),
            "unconfirmed or completed task contacted cmux"
        );
    }
    assert_eq!(dir.join("cancel.json").exists(), case != "terminal");
    assert!(dir.join("task.json").exists());
    assert!(saved.worktree.exists());
    if case == "timeout" {
        assert_eq!(
            ahu::task::load(&dir).unwrap().state,
            ahu::task::TaskState::Running
        );
    }
}

#[test]
fn interactive_cancel_without_supervisor_confirmation_preserves_workspace() {
    interactive_cancel_case("timeout");
}

#[test]
fn interactive_cancel_record_read_failure_preserves_workspace() {
    interactive_cancel_case("unreadable");
}

#[test]
fn interactive_cancel_acknowledgement_closes_only_its_workspace() {
    interactive_cancel_case("confirmed");
}

#[test]
fn interactive_cancel_natural_exit_preserves_workspace() {
    interactive_cancel_case("finished");
}

#[test]
fn interactive_cancel_terminal_record_does_not_claim_new_termination() {
    interactive_cancel_case("terminal");
}

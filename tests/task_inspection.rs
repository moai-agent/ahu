mod common;
use common::TestRepo;
use std::process::Output;

fn mcp_read(repo: &TestRepo, name: &str, arguments: serde_json::Value) -> serde_json::Value {
    use std::{io::Write, process::Stdio};
    let mut child = common::ahu()
        .args(["mcp", "serve"])
        .current_dir(repo.path())
        .env("AHU_CMUX_BIN", repo.state_path().join("missing-cmux"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    for request in [
        serde_json::json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{}}),
        serde_json::json!({"jsonrpc":"2.0", "id":2, "method":"tools/call", "params":{"name":name,"arguments":arguments}}),
    ] {
        writeln!(input, "{request}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|value| value["id"] == 2)
        .unwrap()
}

#[test]
fn cli_mcp_reads_share_resolution_permissions_and_legacy_schemas_without_migration() {
    use serde_json::{Value, json};
    let repo = TestRepo::new();
    let dir = record(&repo, "abc1");
    record(&repo, "abc12");
    let before = std::fs::read(dir.join("task.json")).unwrap();
    for selector in ["abc1", "ahu:task:abc1", "abc12"] {
        let cli = run(&repo, &["task", selector, "--output", "json"]);
        assert!(cli.status.success(), "{cli:?}");
        let cli: Value = serde_json::from_slice(&cli.stdout).unwrap();
        let mcp = mcp_read(&repo, "ahu_task_get", json!({"task":selector}));
        assert_eq!(cli, mcp["result"]["structuredContent"]);
    }
    let cli = run(&repo, &["tasks", "--output", "json"]);
    assert!(cli.status.success());
    let cli: Value = serde_json::from_slice(&cli.stdout).unwrap();
    let mcp = mcp_read(&repo, "ahu_tasks_list", json!({}));
    for key in ["schema_version", "tasks", "unreadable", "notes"] {
        assert_eq!(cli[key], mcp["result"]["structuredContent"][key]);
    }
    for selector in ["missing", "abc"] {
        let cli = run(&repo, &["task", selector, "--output", "json"]);
        assert!(!cli.status.success());
        let mcp = mcp_read(&repo, "ahu_task_get", json!({"task":selector}));
        assert!(
            mcp.get("error").is_some() || mcp["result"]["isError"] == true,
            "{mcp}"
        );
    }
    // Neither transport accepts a record belonging to a different repository.
    let foreign_dir = record(&repo, "ffff");
    let mut foreign: Value =
        serde_json::from_slice(&std::fs::read(foreign_dir.join("task.json")).unwrap()).unwrap();
    foreign["repo_identity"] = "another-repository".into();
    std::fs::write(
        foreign_dir.join("task.json"),
        serde_json::to_vec(&foreign).unwrap(),
    )
    .unwrap();
    assert!(
        !run(&repo, &["task", "ffff", "--output", "json"])
            .status
            .success()
    );
    let denied = mcp_read(&repo, "ahu_task_get", json!({"task":"ffff"}));
    assert!(denied.get("error").is_some() || denied["result"]["isError"] == true);
    let forged = mcp_read(
        &repo,
        "ahu_task_get",
        json!({"task":"abc1", "principal":"operator"}),
    );
    assert!(forged.get("error").is_some());
    assert_eq!(std::fs::read(dir.join("task.json")).unwrap(), before);
    assert!(
        !dir.parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("agent-tasks-v1")
            .exists()
    );
}

#[test]
fn plugin_read_session_is_exact_immutable_and_denies_all_execution_controls() {
    use ahu::agent_task::{inspection::Inspection, v1::*};
    let repo = TestRepo::new();
    let dir = record(&repo, "abc1");
    record(&repo, "abc2");
    let mut starting: ahu::task::TaskRecord =
        serde_json::from_slice(&std::fs::read(dir.join("task.json")).unwrap()).unwrap();
    starting.state = ahu::task::TaskState::Starting;
    ahu::state::write_json(&dir.join("task.json"), &starting).unwrap();
    let discovered = ahu::git::discover(repo.path()).unwrap();
    let view = Inspection::open(&discovered).unwrap();
    assert!(view.plugin_read_session("", Default::default()).is_err());
    let ungranted = view
        .plugin_read_session("other", Default::default())
        .unwrap();
    assert_eq!(ungranted.get(VERSION, "abc1"), Err(Error::Denied));
    let plugin = view
        .plugin_read_session("fixture", ["abc1".into(), "missing".into()].into())
        .unwrap();
    assert_eq!(
        plugin.capabilities(VERSION).unwrap().operations,
        [Operation::Read, Operation::Replay].into()
    );
    assert_eq!(plugin.get(VERSION, "abc2"), Err(Error::Denied));
    assert_eq!(plugin.get(VERSION, "abc"), Err(Error::Denied));
    assert_eq!(plugin.get(VERSION, "missing"), Err(Error::NotFound));
    assert_eq!(plugin.effects(VERSION, "abc2"), Err(Error::Denied));
    assert_eq!(plugin.replay(VERSION, "abc2", 0), Err(Error::Denied));
    assert_eq!(plugin.replay(VERSION, "missing", 0), Err(Error::NotFound));
    assert_eq!(
        plugin.get(VERSION + 1, "abc1"),
        Err(Error::UnsupportedVersion)
    );
    let snapshot = plugin.get(VERSION, "abc1").unwrap();
    assert_eq!(snapshot.lifecycle, Lifecycle::OutcomeUnknown);
    assert_eq!(snapshot.identity.provider, "ahu.legacy-core");
    assert!(!snapshot.supports_input);
    assert_eq!(
        plugin.effects(VERSION, "abc1").unwrap().start,
        DeliveryState::Unknown
    );
    let replay = plugin.replay(VERSION, "abc1", 0).unwrap();
    assert_eq!(replay.snapshot, snapshot);
    assert_eq!(replay.changes.len(), 1);
    assert!(
        plugin
            .replay(VERSION, "abc1", snapshot.revision)
            .unwrap()
            .changes
            .is_empty()
    );
    assert_eq!(
        plugin.replay(VERSION, "abc1", snapshot.revision + 1),
        Err(Error::StaleRevision {
            current: snapshot.revision
        })
    );
    for action in [
        Action::Submit,
        Action::Cancel,
        Action::Input("literal".into()),
    ] {
        assert_eq!(
            plugin.mutate(Mutation {
                version: VERSION,
                task: "abc1".into(),
                operation_id: "attempt".into(),
                expected_revision: snapshot.revision,
                action
            }),
            Err(Error::Denied)
        );
    }
    let mut saved: ahu::task::TaskRecord =
        serde_json::from_slice(&std::fs::read(dir.join("task.json")).unwrap()).unwrap();
    saved.state = ahu::task::TaskState::Exited;
    ahu::state::write_json(&dir.join("task.json"), &saved).unwrap();
    assert_eq!(plugin.get(VERSION, "abc1").unwrap(), snapshot);
    let fresh = Inspection::open(&discovered)
        .unwrap()
        .plugin_read_session("fixture", ["abc1".into()].into())
        .unwrap();
    assert_eq!(
        fresh.get(VERSION, "abc1").unwrap().lifecycle,
        Lifecycle::Exited
    );
    assert_eq!(fresh.get(VERSION, "abc2"), Err(Error::Denied));
}

#[test]
fn inspection_preserves_durable_owner_identity_effects_and_replay_boundaries() {
    use ahu::agent_task::{Controller, inspection::Inspection, v1::*};
    let repo = TestRepo::new();
    let id = ahu::task::new_task_id().unwrap();
    let dir = record(&repo, &id);
    let discovered = ahu::git::discover(repo.path()).unwrap();
    let core = Controller::local(&discovered).unwrap();
    let owner = Principal::Plugin("original-owner".into());
    let mut snapshot = core
        .track_existing(&discovered, &id, "fixture-provider", owner.clone(), true)
        .unwrap();
    for index in 0..REPLAY_WINDOW + 2 {
        snapshot = core
            .reconcile(
                &id,
                "fixture-provider",
                snapshot.revision,
                if index % 2 == 0 {
                    Lifecycle::WaitingForApproval
                } else {
                    Lifecycle::Running
                },
            )
            .unwrap();
    }
    let path = dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("agent-tasks-v1")
        .join(format!("{id}.json"));
    let before = std::fs::read(&path).unwrap();
    let record_before = std::fs::read(dir.join("task.json")).unwrap();
    let plugin = Inspection::open(&discovered)
        .unwrap()
        .plugin_read_session("original-owner", [id.clone()].into())
        .unwrap();
    assert_eq!(plugin.get(VERSION, &id).unwrap(), snapshot);
    assert_eq!(snapshot.owner, owner);
    assert_eq!(snapshot.identity.provider, "fixture-provider");
    assert!(matches!(
        plugin.replay(VERSION, &id, 0),
        Err(Error::ReplayExpired { .. })
    ));
    let replay = plugin
        .replay(VERSION, &id, snapshot.revision - REPLAY_WINDOW as u64)
        .unwrap();
    assert_eq!(replay.changes.len(), REPLAY_WINDOW);
    assert!(
        replay
            .changes
            .iter()
            .all(|change| change.revision > snapshot.revision - REPLAY_WINDOW as u64)
    );
    assert!(
        plugin
            .replay(VERSION, &id, snapshot.revision)
            .unwrap()
            .changes
            .is_empty()
    );
    assert_eq!(
        plugin.replay(VERSION, &id, snapshot.revision + 1),
        Err(Error::StaleRevision {
            current: snapshot.revision
        })
    );
    assert_eq!(
        plugin.effects(VERSION, &id).unwrap().start,
        DeliveryState::Confirmed
    );
    for action in [
        Action::Submit,
        Action::Cancel,
        Action::Input("literal".into()),
    ] {
        assert_eq!(
            plugin.mutate(Mutation {
                version: VERSION,
                task: id.clone(),
                operation_id: "deny".into(),
                expected_revision: snapshot.revision,
                action
            }),
            Err(Error::Denied)
        );
    }
    assert!(
        run(&repo, &["task", &id, "--output", "json"])
            .status
            .success()
    );
    assert!(mcp_read(&repo, "ahu_task_get", serde_json::json!({"task":id}))["result"].is_object());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read(dir.join("task.json")).unwrap(), record_before);
    // A legacy record cannot rewrite a service-owned frozen identity.
    let mut changed: ahu::task::TaskRecord = serde_json::from_slice(&record_before).unwrap();
    changed.identity.model = "different-model".into();
    ahu::state::write_json(&dir.join("task.json"), &changed).unwrap();
    assert!(Inspection::open(&discovered).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // Existing corrupt V1 state must not silently fall back to legacy metadata.
    std::fs::write(dir.join("task.json"), &record_before).unwrap();
    ahu::state::write_private_file(&path, b"corrupt").unwrap();
    assert!(
        !run(&repo, &["task", &id, "--output", "json"])
            .status
            .success()
    );
    let denied = mcp_read(&repo, "ahu_task_get", serde_json::json!({"task":id}));
    assert!(denied.get("error").is_some() || denied["result"]["isError"] == true);
    assert_eq!(std::fs::read(&path).unwrap(), b"corrupt");
}

#[test]
#[cfg(unix)]
fn read_only_listing_preserves_records_and_observes_cross_window_liveness() {
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
        assert_eq!(persisted.state, ahu::task::TaskState::Running);
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
    saved.cmux_group_id = Some("only-this-group".into());
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
method,params = sys.argv[2],json.loads(sys.argv[3])
if method == 'window.list': print(json.dumps({'windows':[{'id':'window'}]}))
elif method == 'workspace.list': print(json.dumps({'workspaces':[{'id':'only-this-workspace','current_directory':'/tmp'}]}))
elif method == 'workspace.group.list': print(json.dumps({'groups':[{'id':'only-this-group','name':'fixture','anchor_workspace_id':'anchor','member_workspace_ids':['anchor','only-this-workspace']}]}))
elif method == 'workspace.select':
 assert params == {'workspace_id':'only-this-workspace'}
 print('{}')
else: raise AssertionError(method)
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
}

/// `ahu diff` was removed. Plain Git on the task's own checkout does the same
/// job, so the word must fail like any other one ahu does not know rather than
/// resolve a task and do something surprising with it.
#[test]
fn diff_is_not_a_command_and_task_offers_the_git_command_instead() {
    let repo = TestRepo::new();
    let dir = record(&repo, "abc1");
    for args in [
        vec!["diff"],
        vec!["diff", "abc1"],
        vec!["diff", "abc1", "--output", "json"],
    ] {
        let out = run(&repo, &args);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("unknown command"), "{args:?}: {stderr}");
        assert!(out.stdout.is_empty(), "{args:?}");
        assert_eq!(
            out.status.code(),
            run(&repo, &["definitely-not-a-command"]).status.code(),
            "{args:?}"
        );
    }

    let saved: ahu::task::TaskRecord =
        serde_json::from_slice(&std::fs::read(dir.join("task.json")).unwrap()).unwrap();
    let text = String::from_utf8(run(&repo, &["task", "abc1"]).stdout).unwrap();
    assert!(
        text.contains(&format!(
            "review    git -C '{}' diff {}",
            saved.worktree.display(),
            saved.base_commit.as_deref().unwrap()
        )),
        "{text}"
    );
}

#[test]
fn lookup_rejects_ambiguity_including_unreadable_records_and_accepts_exact_ids() {
    let repo = TestRepo::new();
    record(&repo, "abc1");
    let bad = record(&repo, "abc2");
    std::fs::write(bad.join("task.json"), "broken").unwrap();
    let out = run(&repo, &["task", "abc"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("ambiguous"));
    assert_eq!(run(&repo, &["task", "abc2"]).status.code(), Some(5));
    assert_eq!(run(&repo, &["task", "missing"]).status.code(), Some(2));
    assert_eq!(run(&repo, &["task"]).status.code(), Some(2));
    assert_eq!(
        run(&repo, &["task", "abc1", "--output", "yaml"])
            .status
            .code(),
        Some(2)
    );
    record(&repo, "abc12");
    assert!(run(&repo, &["task", "abc1"]).status.success());
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

/// Removing `ahu diff` must not remove the one place ahu says that a session
/// reported writing outside the worktree it was given. Looking only at the
/// branch would show nothing and mean nothing; `ahu task` now carries the
/// disclosure, and points at `ahu result` for the rest of it.
#[test]
fn task_discloses_recorded_writes_outside_the_worktree() {
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
        // runtime one and the planted files would never be read.
        std::fs::rename(&internal, store.join(id)).unwrap();
    }
    let escaped: Vec<String> = (0..5)
        .map(|n| {
            runtime
                .path()
                .join(format!("escaped-{n}.txt"))
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    let borrowed: Vec<&str> = escaped.iter().map(String::as_str).collect();
    plant_outside_writes(&store, "abc1", &borrowed);
    plant_outside_writes(&store, "abc2", &[]);

    let out = run_with_runtime(&repo, runtime.path(), &["task", "abc1"]);
    assert!(out.status.success(), "{:?}", out);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(
            "write tool calls in the recorded event stream targeted paths outside the task"
        ),
        "{text}"
    );
    // Bounded, with the remainder named rather than silently dropped.
    assert!(text.contains("escaped-0.txt"), "{text}");
    assert!(text.contains("escaped-2.txt"), "{text}");
    assert!(!text.contains("escaped-3.txt"), "{text}");
    assert!(text.contains("... and 2 more"), "{text}");
    assert!(
        text.contains("Run `ahu result ahu:task:abc1` for the full recorded list."),
        "{text}"
    );

    // Nothing recorded, nothing claimed.
    let out = run_with_runtime(&repo, runtime.path(), &["task", "abc2"]);
    assert!(out.status.success(), "{:?}", out);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(!text.contains("outside the task"), "{text}");

    // The JSON contract is unchanged; the disclosure is a human-output line.
    let out = run_with_runtime(&repo, runtime.path(), &["task", "abc1", "--output", "json"]);
    assert!(out.status.success(), "{:?}", out);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert!(value.get("writes_outside_worktree").is_none(), "{value}");
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
    saved.cmux_group_id = Some("owned-fixture-group".into());
    saved.cmux_workspace_id = Some("owned-fixture".into());
    if case == "terminal" {
        saved.state = ahu::task::TaskState::Cancelled;
    }
    ahu::task::save(&dir, &saved, "synthetic input").unwrap();
    let cmux = repo.state_path().join("cmux");
    let calls = repo.state_path().join("calls");
    std::fs::write(&cmux, r##"#!/usr/bin/env python3
import json,os,sys
if sys.argv[1] == 'ping': print('PONG'); sys.exit(0)
method,params = sys.argv[2],json.loads(sys.argv[3])
if method == 'window.list': print(json.dumps({'windows':[{'id':'window'}]}))
elif method == 'workspace.list': print(json.dumps({'workspaces':[{'id':'owned-fixture','current_directory':'/tmp'}]}))
elif method == 'workspace.group.list': print(json.dumps({'groups':[{'id':'owned-fixture-group','name':'fixture','anchor_workspace_id':'anchor','member_workspace_ids':['anchor','owned-fixture']}]}))
elif method == 'workspace.close':
 open(os.environ['AHU_TEST_CMUX_CALLS'],'a').write(method+' '+json.dumps(params)+'\n')
 print('{}')
else: raise AssertionError(method)
"##).unwrap();
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

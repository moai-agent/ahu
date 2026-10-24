mod common;
use common::TestRepo;
use std::process::Output;

fn mcp_read(repo: &TestRepo, name: &str, arguments: serde_json::Value) -> serde_json::Value {
    use std::{io::Write, process::Stdio};
    let mut child = common::ahu()
        .args(["mcp", "serve"])
        .current_dir(repo.path())
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
    assert!(ungranted.list(VERSION, None, 1).unwrap().tasks.is_empty());
    let plugin = view
        .plugin_read_session("fixture", ["abc1".into(), "missing".into()].into())
        .unwrap();
    assert_eq!(
        plugin.capabilities(VERSION).unwrap().operations,
        [Operation::List, Operation::Read, Operation::Replay].into()
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
    assert_eq!(plugin.list(VERSION, None, 2), Err(Error::NotFound));
    let scoped = view
        .plugin_read_session("fixture", ["abc1".into()].into())
        .unwrap();
    let page = scoped.list(VERSION, None, 1).unwrap();
    assert_eq!(page.tasks, vec![snapshot.clone()]);
    assert_eq!(page.next_after, None);
    assert_eq!(scoped.list(VERSION, Some("abc2"), 1), Err(Error::Denied));
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
    assert_eq!(scoped.list(VERSION, None, 1).unwrap(), page);
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
fn task_state_uses_process_ownership_not_terminal_workspace_presence() {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;
    for case in ["live", "closed", "unreadable"] {
        let repo = TestRepo::new();
        let dir = record(&repo, "ownercheck");
        let owner_path = dir.join("owner.lock");
        let held = if case == "unreadable" {
            let target = repo.state_path().join("owner-target");
            std::fs::write(&target, b"not an owner lock").unwrap();
            std::os::unix::fs::symlink(target, &owner_path).unwrap();
            None
        } else {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .mode(0o600)
                .open(&owner_path)
                .unwrap();
            if case == "live" {
                // SAFETY: file retains a valid descriptor until it is dropped.
                assert_eq!(
                    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                    0
                );
                Some(file)
            } else {
                drop(file);
                None
            }
        };
        let detail = run(&repo, &["task", "ownercheck", "--output", "json"]);
        assert!(detail.status.success(), "{:?}", detail);
        let value: serde_json::Value = serde_json::from_slice(&detail.stdout).unwrap();
        assert_eq!(
            value["session_state"],
            match case {
                "live" => "running",
                "closed" => "exited",
                _ => "unknown",
            }
        );
        let persisted: ahu::task::TaskRecord =
            serde_json::from_slice(&std::fs::read(dir.join("task.json")).unwrap()).unwrap();
        assert_eq!(
            persisted.state,
            if case == "closed" {
                ahu::task::TaskState::Exited
            } else {
                ahu::task::TaskState::Running
            }
        );
        drop(held);
    }
}

#[test]
#[cfg(unix)]
fn current_terminal_task_is_running_only_while_its_owner_lock_is_held() {
    use std::os::fd::AsRawFd;
    let repo = TestRepo::new();
    let dir = record(&repo, "foreground");
    let owner_path = dir.join("owner.lock");
    use std::os::unix::fs::OpenOptionsExt;
    let owner = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(&owner_path)
        .unwrap();
    // SAFETY: owner retains a valid descriptor until the lock is dropped.
    assert_eq!(
        unsafe { libc::flock(owner.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let active = run(&repo, &["task", "foreground", "--output", "json"]);
    assert!(active.status.success(), "{:?}", active);
    let active: serde_json::Value = serde_json::from_slice(&active.stdout).unwrap();
    assert_eq!(active["session_state"], "running");

    drop(owner);
    let stale = run(&repo, &["task", "foreground", "--output", "json"]);
    assert!(stale.status.success(), "{:?}", stale);
    let stale: serde_json::Value = serde_json::from_slice(&stale.stdout).unwrap();
    assert_eq!(stale["session_state"], "exited");
    let persisted: ahu::task::TaskRecord =
        serde_json::from_slice(&std::fs::read(dir.join("task.json")).unwrap()).unwrap();
    assert_eq!(persisted.state, ahu::task::TaskState::Exited);
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
        session_ref: None,
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
        .output()
        .unwrap()
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
    assert_eq!(json["schema_version"], 2);
    assert_eq!(json["session_state"], "unknown");
    assert!(json.get("state_source").is_none());
    assert!(json.get("liveness").is_none());
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
            .contains("[session unknown]")
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
            "review    git -C '{}' diff --no-ext-diff --no-textconv '{}' --",
            saved.worktree.display(),
            saved.base_commit.as_deref().unwrap()
        )),
        "{text}"
    );

    let path = dir.join("task.json");
    let mut altered: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    altered["base_commit"] = "HEAD; printf AHU_REVIEW_SENTINEL".into();
    std::fs::write(&path, serde_json::to_vec(&altered).unwrap()).unwrap();
    let text = String::from_utf8(run(&repo, &["task", "abc1"]).stdout).unwrap();
    assert!(text.contains("review unavailable: recorded base commit is invalid"));
    assert!(!text.contains("git -C "));
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

    // The versioned JSON summary reports the reconciled session state.
    let out = run_with_runtime(&repo, runtime.path(), &["task", "abc1", "--output", "json"]);
    assert!(out.status.success(), "{:?}", out);
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["schema_version"], 2);
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

//! What `ahu tasks` says about records it cannot read.
//!
//! `task::list` used to drop unreadable records, despite claiming to report them.
//! Unreadable records were initially the exception.
//!
//! The schema-2 bump made it the rule. Every record written by an earlier ahu is
//! refused, so a repository with five tasks, five worktrees, five branches and
//! three live cmux sessions had `ahu tasks` print "No ahu tasks have been
//! launched from this repository." A positive claim of absence, false, on the
//! one surface that could have told the user where their leftover worktrees
//! were.

mod common;

use std::path::{Path, PathBuf};

use common::TestRepo;

/// Write a task record directory by hand, at whichever schema version is asked
/// for, without going through `task::save`.
///
/// Schema 1 is written as raw JSON on purpose: this build cannot construct a
/// `TaskRecord` at that version, and a fixture that could would not be testing
/// the thing that actually happens on a real machine.
fn write_schema_1(tasks_dir: &Path, task_id: &str) -> PathBuf {
    let dir = tasks_dir.join(task_id);
    std::fs::create_dir_all(&dir).unwrap();
    let record = serde_json::json!({
        "schema_version": 1,
        "task_id": task_id,
        "title": "an earlier task",
        "created_at": "2026-09-01T00:00:00Z",
        "repo_identity": "r",
        "repo_root": "/nonexistent",
        "branch": format!("ahu/chris/{task_id}"),
        "worktree": "/nonexistent",
        "base_commit": null,
        "identity": {
            "mode": "named",
            "agent": "chris",
            "agent_version": "1.0.0",
            "permissions": "prompt",
            "harness": "claude-code",
            "model": "claude-opus-5",
            "instructions_source": ".claude/agents/chris.md",
            // Schema 1's meaning: the whole file's digest, under this name.
            "instructions_digest": "1".repeat(64),
            "identity_digest": null,
            "selection_basis": null,
        },
        "policy_digest": "0".repeat(64),
        "catalog_version": ahu::catalog::CATALOG_VERSION,
        "config_snapshot": {"entries": [], "skipped_directories": []},
        "config_snapshot_digest": "0".repeat(64),
        "materialize": {"written": [], "removed": [], "concurrently_modified": []},
        "launch_command": {"program": "claude", "args": []},
        "prompt_digest": "0".repeat(64),
        "enforcement": {
            "harness": "claude-code",
            "harness_version": null,
            "model_fixed_for_session": false,
            "gaps": [],
            "applied_controls": [],
        },
        "reliability_warning": null,
        "cmux_group_id": null,
        "cmux_workspace_id": null,
        "cmux_window_id": null,
        "state": "running",
    });
    std::fs::write(
        dir.join("task.json"),
        serde_json::to_string_pretty(&record).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("prompt.txt"), "earlier work").unwrap();
    dir
}

/// A valid schema-2 record, written the way a launch writes one.
fn write_current(tasks_dir: &Path, repo: &TestRepo, task_id: &str) -> PathBuf {
    let discovered = ahu::git::discover(repo.path()).unwrap();
    let adapter = ahu::harness::adapter_for("claude-code").unwrap();
    let (delivered, delivery) =
        ahu::orchestration::deliver(Some("You are chris."), "current work").unwrap();
    let command = adapter
        .launch_command(&ahu::harness::LaunchRequest {
            model: "claude-opus-5",
            prompt: &delivered,
            cwd: &discovered.root,
            permissions: Default::default(),
        })
        .unwrap();
    let record = ahu::task::TaskRecord {
        schema_version: ahu::task::TASK_SCHEMA_VERSION,
        task_id: task_id.to_string(),
        title: "a current task".to_string(),
        summary: String::new(),
        created_at: "2026-09-12T00:00:00Z".to_string(),
        repo_identity: discovered.identity(),
        repo_root: discovered.root.clone(),
        branch: format!("ahu/chris/{task_id}"),
        worktree: discovered.root.join(format!(".worktrees/{task_id}")),
        base_commit: discovered.head.clone(),
        identity: ahu::task::LaunchIdentity {
            mode: ahu::task::LaunchMode::Named,
            agent: "chris".to_string(),
            agent_version: Some("1.0.0".to_string()),
            permissions: Default::default(),
            harness: "claude-code".to_string(),
            model: "claude-opus-5".to_string(),
            instructions_source: Some(".claude/agents/chris.md".to_string()),
            source_digest: Some("2".repeat(64)),
            instructions_digest: Some("3".repeat(64)),
            identity_digest: Some("4".repeat(64)),
            selection_basis: None,
        },
        policy_digest: "0".repeat(64),
        catalog_version: ahu::catalog::CATALOG_VERSION.to_string(),
        config_snapshot: Default::default(),
        config_snapshot_digest: "0".repeat(64),
        hooks: Default::default(),
        hooks_digest: String::new(),
        materialize: Default::default(),
        launch_command: command.redacted(),
        delivery,
        prompt_digest: ahu::util::digest_bytes(b"current work"),
        harness_executable: PathBuf::from("/usr/local/bin/claude"),
        reliability_warning: None,
        enforcement: adapter
            .enforcement("claude-opus-5", Default::default())
            .unwrap(),
        cmux_group_id: None,
        cmux_workspace_id: None,
        cmux_window_id: None,
        state: ahu::task::TaskState::Exited,
    };
    let dir = tasks_dir.join(task_id);
    ahu::task::save(&dir, &record, "current work").unwrap();
    dir
}

/// `AHU_STATE_DIR` is process-wide, and these tests run in parallel by default.
///
/// Every read and write of it in this binary goes through `with_state`, so the
/// guard makes the mutation exclusive. Without it the suite passes under
/// `--test-threads=1` and fails at random otherwise, which is a worse outcome
/// than either result on its own.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Run `f` with `AHU_STATE_DIR` pointed at this repository's scratch state.
fn with_state<T>(repo: &TestRepo, f: impl FnOnce() -> T) -> T {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: the guard makes this binary the only mutator, and every reader
    // here runs inside it.
    unsafe { std::env::set_var("AHU_STATE_DIR", repo.state_path()) };
    let result = f();
    drop(guard);
    result
}

/// Run a command against that state, capturing what it printed.
fn scripted(
    repo: &TestRepo,
    f: impl FnOnce(&mut ahu::launcher::Console<'_>) -> ahu::util::Result<i32>,
) -> (i32, String) {
    let mut input = std::io::Cursor::new(Vec::new());
    let mut output: Vec<u8> = Vec::new();
    let code = with_state(repo, || {
        let mut console = ahu::launcher::Console {
            input: &mut input,
            output: &mut output,
            interactive: false,
        };
        f(&mut console)
    });
    let text = String::from_utf8_lossy(&output).to_string();
    match code {
        Ok(code) => (code, text),
        Err(e) => (1, format!("{text}\nERROR: {e}")),
    }
}

/// The tasks directory for a repository, created.
fn tasks_dir(repo: &TestRepo) -> PathBuf {
    let discovered = ahu::git::discover(repo.path()).unwrap();
    with_state(repo, || {
        let dir = ahu::state::tasks_dir(&discovered.identity()).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    })
}

/// One readable record and one refused one: both are accounted for.
#[test]
fn tasks_lists_the_readable_record_and_reports_the_unreadable_one() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    write_schema_1(&dir, "006aa50000000000a1");
    write_current(&dir, &repo, "006aa50000000000b2");

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let (code, text) = scripted(&repo, |console| ahu::commands::tasks(console, &discovered));
    assert_eq!(code, 0, "{text}");

    // The readable one is listed as usual.
    assert!(text.contains("006aa50000000000b2"), "{text}");
    assert!(text.contains("a current task"), "{text}");

    // And the refused one is reported, with its id and the reason.
    assert!(text.contains("006aa50000000000a1"), "{text}");
    assert!(text.contains("[unreadable]"), "{text}");
    assert!(
        text.contains("1 task(s) in this repository could not be read or are incomplete"),
        "{text}"
    );
    assert!(
        text.contains("schema version (1)") && text.contains("reads 2"),
        "the reason must say why, not just that: {text}"
    );
    assert!(
        text.contains("they are not gone"),
        "the user's problem is the leftover worktree: {text}"
    );

    // Nothing was rewritten. The refused file is audit trail.
    let raw = std::fs::read_to_string(dir.join("006aa50000000000a1/task.json")).unwrap();
    assert!(raw.contains("\"schema_version\": 1"), "{raw}");
    assert!(
        raw.contains(&"1".repeat(64)),
        "schema 1's instructions_digest must be left exactly as it was"
    );

    // And `task_dirs` accounts for both, so a helper cannot reintroduce the bug.
    let dirs = with_state(&repo, || ahu::commands::task_dirs(&discovered).unwrap());
    assert_eq!(dirs.len(), 2, "{dirs:?}");
}

/// Every record unreadable: the absence claim must not be printed.
///
/// This is the assertion the whole finding reduces to.
#[test]
fn tasks_never_claims_no_tasks_exist_when_records_were_refused() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    for id in [
        "006aa50000000000c1",
        "006aa50000000000c2",
        "006aa50000000000c3",
        "006aa50000000000c4",
        "006aa50000000000c5",
    ] {
        write_schema_1(&dir, id);
    }

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let (code, text) = scripted(&repo, |console| ahu::commands::tasks(console, &discovered));
    assert_eq!(code, 0, "{text}");

    assert!(
        !text.contains(ahu::commands::NO_TASKS),
        "ahu claimed no tasks exist while holding five refused records:\n{text}"
    );
    assert!(
        text.contains("5 task(s) in this repository could not be read or are incomplete"),
        "{text}"
    );
    for id in [
        "006aa50000000000c1",
        "006aa50000000000c2",
        "006aa50000000000c3",
        "006aa50000000000c4",
        "006aa50000000000c5",
    ] {
        assert!(text.contains(id), "{id} is missing from:\n{text}");
    }
}

/// A repository that genuinely has no tasks still says so.
///
/// The fix must not turn an honest statement into a hedge.
#[test]
fn tasks_still_says_so_when_there_really_are_none() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    tasks_dir(&repo);

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let (code, text) = scripted(&repo, |console| ahu::commands::tasks(console, &discovered));
    assert_eq!(code, 0, "{text}");
    assert!(text.contains(ahu::commands::NO_TASKS), "{text}");
}

/// The leftover worktree and branch are what the user actually needs, and the
/// task id in the directory name is enough to recover both.
#[test]
fn an_unreadable_record_still_names_its_worktree_and_branch() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    let task_id = "006aa50000000000d1";
    write_schema_1(&dir, task_id);

    // Create the worktree and branch the task would have left behind.
    let discovered = ahu::git::discover(repo.path()).unwrap();
    let worktree = ahu::state::worktree_dir(&discovered.root, task_id).unwrap();
    ahu::git::add_worktree(
        &discovered,
        &worktree,
        &format!("ahu/chris/{task_id}"),
        discovered.head.as_deref().unwrap(),
    )
    .unwrap();

    let (_, text) = scripted(&repo, |console| ahu::commands::tasks(console, &discovered));

    assert!(
        text.contains(&ahu::util::display_path(&worktree)),
        "the worktree must be named: {text}"
    );
    assert!(
        text.contains(&format!("ahu/chris/{task_id}")),
        "the branch must be recovered from git: {text}"
    );
    assert!(
        !text.contains("no longer on disk"),
        "the worktree is right there: {text}"
    );
}

/// When the worktree is gone the output says that rather than implying it is
/// still there, and points at the authoritative listing.
#[test]
fn an_unreadable_record_whose_worktree_is_gone_says_so() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    write_schema_1(&dir, "006aa50000000000e1");

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let (_, text) = scripted(&repo, |console| ahu::commands::tasks(console, &discovered));

    assert!(text.contains("no longer on disk"), "{text}");
    assert!(
        text.contains("branch    none found for this task id"),
        "{text}"
    );
    assert!(
        text.contains("git worktree list") && text.contains(".worktrees/"),
        "the user must be pointed at what ahu could not recover: {text}"
    );
}

/// `ahu focus` must not report a task as nonexistent when its record is merely
/// unreadable.
#[test]
fn focus_distinguishes_an_unreadable_task_from_a_missing_one() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    write_schema_1(&dir, "006aa50000000000f1");

    let discovered = ahu::git::discover(repo.path()).unwrap();

    // The task exists; ahu just cannot read its record.
    let (_, text) = scripted(&repo, |console| {
        ahu::commands::focus(console, &discovered, "006aa50000000000f1")
    });
    assert!(
        text.contains("exists but ahu cannot read its record"),
        "{text}"
    );
    assert!(text.contains("schema version (1)"), "{text}");
    assert!(text.contains("Run `ahu tasks`"), "{text}");
    assert!(
        !text.contains("no task matching"),
        "an unreadable task is not a missing one: {text}"
    );

    // A genuinely absent id still says so — and mentions that other records
    // could not be read, so "not found" is not mistaken for "nothing here".
    let (_, text) = scripted(&repo, |console| {
        ahu::commands::focus(console, &discovered, "006aa5000000000099")
    });
    assert!(text.contains("no readable task matching"), "{text}");
    assert!(text.contains("could not be read either"), "{text}");
}

/// With no unreadable records at all, `focus` keeps its original message.
#[test]
fn focus_on_a_clean_repository_still_reports_a_missing_task_plainly() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    tasks_dir(&repo);

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let (_, text) = scripted(&repo, |console| {
        ahu::commands::focus(console, &discovered, "006aa5000000000099")
    });
    assert!(text.contains("no task matching"), "{text}");
    assert!(!text.contains("could not be read"), "{text}");
}

/// `task::list` is the layer the bug lived in: assert its contract directly.
#[test]
fn list_carries_what_it_could_not_read() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    write_schema_1(&dir, "006aa50000000000g1");
    write_current(&dir, &repo, "006aa50000000000g2");

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let listing = with_state(&repo, || ahu::task::list(&discovered).unwrap());

    assert_eq!(listing.records.len(), 1);
    assert_eq!(listing.unreadable.len(), 1);
    assert!(!listing.is_empty());

    let found = &listing.unreadable[0];
    assert_eq!(found.task_id, "006aa50000000000g1");
    assert!(found.dir.ends_with("006aa50000000000g1"));
    assert!(
        found.reason.contains("schema version (1)"),
        "{}",
        found.reason
    );
    assert_eq!(listing.dirs().len(), 2);

    // Nothing was read out of the refused file: the id comes from the directory
    // name, which ahu chose, not from a schema it does not understand.
    assert!(
        !found.reason.contains("an earlier task"),
        "the refused record's fields must not be surfaced: {}",
        found.reason
    );
}

/// A directory that is not a schema problem at all — truncated JSON — is
/// reported the same way, so the fix is about unreadability, not about schema 1.
#[test]
fn a_corrupt_record_is_reported_rather_than_dropped() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    let corrupt = dir.join("006aa50000000000h1");
    std::fs::create_dir_all(&corrupt).unwrap();
    std::fs::write(corrupt.join("task.json"), "{ this is not json").unwrap();

    let discovered = ahu::git::discover(repo.path()).unwrap();
    let (code, text) = scripted(&repo, |console| ahu::commands::tasks(console, &discovered));
    assert_eq!(code, 0, "{text}");
    assert!(!text.contains(ahu::commands::NO_TASKS), "{text}");
    assert!(text.contains("006aa50000000000h1"), "{text}");
    assert!(text.contains("not a valid ahu task record"), "{text}");
}

/// A launch whose drift comparison could not see earlier records says so, rather
/// than letting "no drift" mean both "nothing changed" and "ahu could not look".
#[test]
fn a_launch_says_when_drift_could_not_read_earlier_records() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.write("assignment.txt", "do the thing\n");
    repo.commit("fixture");
    let dir = tasks_dir(&repo);
    write_schema_1(&dir, "006aa50000000000i1");

    let bin = common::fake_harness(repo.state_path(), &repo.state_path().join("argv"));
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
        .current_dir(repo.path())
        .args(["launch", "@chris", "--prompt-file"])
        .arg(repo.path().join("assignment.txt"))
        .arg("--dry-run")
        .env("AHU_STATE_DIR", repo.state_path())
        .env("AHU_CMUX_BIN", repo.state_path().join("missing-cmux"))
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("1 earlier task record(s) for this repository could not be read"),
        "{text}"
    );
    assert!(text.contains("drift was"), "{text}");
}

//! A task's state belongs to the task's worktree.
//!
//! ahu chooses where a task's record, prompt and session state go at launch and
//! wires the child to it, so nothing has to be prefixed with `AHU_STATE_DIR`.
//! Putting that state inside the worktree the task works in is what makes
//! removing the worktree remove the task's state with it, and what lets the
//! primary checkout and every sibling worktree find the live tasks by looking
//! in one place: `.worktrees/`.

mod common;

use std::path::{Path, PathBuf};

use ahu::{git, state, task};
use common::TestRepo;

/// A repository with a configuration and one registered agent.
fn fixture() -> TestRepo {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    repo
}

/// Run the real binary with no state override at all, from `dir`.
///
/// `env_remove` rather than a pointed-at temporary directory: these tests are
/// about ordinary operation, where nothing sets that variable.
fn ahu_in(dir: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
        .args(args)
        .current_dir(dir)
        .env_remove("AHU_STATE_DIR")
        .output()
        .expect("ahu runs")
}

fn text_of(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Create a task worktree and the record that belongs to it, the way `execute`
/// does: worktree first, then its own state directory, then the record.
fn prepare_task(repo: &TestRepo, from: &Path, task_id: &str) -> PathBuf {
    let discovered = git::discover(from).unwrap();
    state::ensure_worktrees_root(&discovered.root).unwrap();
    let worktree = state::worktree_dir(&discovered.root, task_id).unwrap();
    let branch = format!("ahu/chris/{task_id}");
    git::add_worktree(&discovered, &worktree, &branch, "HEAD").unwrap();
    state::ensure_checkout_state(&worktree).unwrap();

    let identity = discovered.identity();
    let dir = state::worktree_task_dir(&worktree, &identity, task_id);
    task::save(&dir, &record_for(repo, task_id, &worktree), "current work").unwrap();
    dir
}

fn record_for(repo: &TestRepo, task_id: &str, worktree: &Path) -> task::TaskRecord {
    let discovered = git::discover(repo.path()).unwrap();
    let adapter = ahu::harness::adapter_for("claude-code").unwrap();
    let (delivered, delivery) =
        ahu::orchestration::deliver(Some("You are chris."), "current work").unwrap();
    let command = adapter
        .launch_command(&ahu::harness::LaunchRequest {
            model: "claude-opus-5",
            prompt: &delivered,
            cwd: worktree,
            permissions: Default::default(),
        })
        .unwrap();
    task::TaskRecord {
        schema_version: task::TASK_SCHEMA_VERSION,
        task_id: task_id.to_string(),
        title: format!("task {task_id}"),
        summary: String::new(),
        created_at: format!("2026-09-12T00:00:{:02}Z", task_id.len()),
        repo_identity: discovered.identity(),
        repo_root: discovered.root.clone(),
        branch: format!("ahu/chris/{task_id}"),
        worktree: worktree.to_path_buf(),
        base_commit: discovered.head.clone(),
        identity: task::LaunchIdentity {
            mode: task::LaunchMode::Named,
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
        state: task::TaskState::Exited,
    }
}

/// Build the plan an ordinary `ahu launch` builds, with nothing set in the
/// environment, and read back where it decided the task's state goes.
fn plan_from(from: &Path) -> ahu::launch::LaunchPlan {
    let discovered = git::discover(from).unwrap();
    let loaded = ahu::config::load(&discovered.root).unwrap().unwrap();
    let agent = ahu::agent::find(&discovered.root, "chris").unwrap();
    let pair = ahu::selection::ResolvedPair {
        harness: agent.manifest.harness.clone(),
        model: agent.manifest.model.clone(),
        basis: "named agent".to_string(),
        policy_digest: loaded.digest.clone(),
        catalog_version: loaded.config.catalog_version.clone(),
    };
    ahu::launch::plan(&discovered, Some(agent), pair, "do the thing").unwrap()
}

/// The record, the prompt and the session state a task gets are chosen from the
/// worktree ahu is about to create, not from the environment.
#[test]
fn a_plain_launch_places_task_state_inside_the_worktree_it_creates() {
    let repo = fixture();
    let discovered = git::discover(repo.path()).unwrap();
    let plan = plan_from(repo.path());

    assert!(
        plan.task_dir.starts_with(&plan.worktree),
        "task state must live in the task's worktree: {} is not under {}",
        plan.task_dir.display(),
        plan.worktree.display()
    );
    assert_eq!(
        plan.task_dir,
        plan.worktree
            .join(".ahu/state/repos")
            .join(discovered.identity())
            .join("tasks")
            .join(&plan.task_id)
    );
    // And the location reaches the child through the startup command ahu
    // builds, so nothing has to be exported for the session to find it.
    let startup = ahu::cmux::startup_command(Path::new("/usr/local/bin/ahu"), &plan.task_dir);
    assert!(
        startup.contains(&plan.task_dir.to_string_lossy().to_string()),
        "{startup}"
    );
    // Planning creates nothing in the invoking checkout's own store.
    assert!(!repo.path().join(".ahu/state/repos").exists());
}

/// A launch made from inside a task worktree records into the *new* task's
/// worktree, not into the parent task's.
#[test]
fn a_nested_launch_records_into_its_own_worktree_not_its_parent() {
    let repo = fixture();
    let parent = state::worktree_dir(repo.path(), "006aa50000000000p1").unwrap();
    state::ensure_worktrees_root(repo.path()).unwrap();
    let discovered = git::discover(repo.path()).unwrap();
    git::add_worktree(&discovered, &parent, "ahu/chris/parent", "HEAD").unwrap();
    state::ensure_checkout_state(&parent).unwrap();

    let plan = plan_from(&parent);
    // `repo.path()` and Git's own answer differ by macOS's `/var` link, so the
    // containment check compares resolved paths.
    let siblings = repo.path().join(".worktrees").canonicalize().unwrap();
    assert!(
        plan.worktree.parent().unwrap().canonicalize().unwrap() == siblings,
        "a nested task is still a sibling: {}",
        plan.worktree.display()
    );
    assert!(plan.task_dir.starts_with(&plan.worktree));
    assert!(
        !plan.task_dir.starts_with(&parent),
        "the child's record must not be written into the parent task's worktree"
    );
}

/// The same live tasks are found from the primary checkout and from a sibling,
/// with nothing set in the environment.
#[test]
fn live_tasks_are_discovered_from_the_primary_checkout_and_from_a_sibling() {
    let repo = fixture();
    let first = "006aa50000000000d1";
    let second = "006aa50000000000d2";
    prepare_task(&repo, repo.path(), first);
    let second_dir = prepare_task(&repo, repo.path(), second);
    let second_worktree = repo.path().join(".worktrees").join(second);

    for from in [repo.path(), second_worktree.as_path()] {
        let listing = ahu_in(from, &["tasks"]);
        let text = text_of(&listing);
        assert!(listing.status.success(), "{text}");
        for id in [first, second] {
            assert!(
                text.contains(id),
                "`ahu tasks` from {from:?} missed {id}: {text}"
            );
        }
    }

    // Inspection resolves an id the same way from a sibling worktree.
    let inspected = ahu_in(&second_worktree, &["task", second, "--output", "json"]);
    let text = text_of(&inspected);
    assert!(inspected.status.success(), "{text}");
    let value: serde_json::Value = serde_json::from_str(text.trim()).expect("json");
    assert_eq!(value["task_id"], second);
    assert_eq!(value["worktree_exists"], true);
    assert_eq!(
        value["record_path"].as_str().unwrap(),
        second_dir.join("task.json").to_string_lossy()
    );

    // And a diff against the launch base runs in the task's own checkout.
    let diff = ahu_in(repo.path(), &["diff", first]);
    assert!(diff.status.success(), "{}", text_of(&diff));
}

/// Removing the worktree removes the task's state, and the listing stops
/// claiming the task is there.
#[test]
fn removing_a_task_worktree_removes_its_state_and_its_listing() {
    let repo = fixture();
    let gone = "006aa50000000000r1";
    let kept = "006aa50000000000r2";
    let gone_dir = prepare_task(&repo, repo.path(), gone);
    prepare_task(&repo, repo.path(), kept);
    assert!(gone_dir.join("task.json").is_file());
    assert!(gone_dir.join("prompt.txt").is_file());

    // Ordinary removal, not `--force`: the state directory ignores itself, so
    // it does not make the worktree look dirty and does not have to be cleaned
    // up by hand first.
    let worktree = repo.path().join(".worktrees").join(gone);
    common::git(
        repo.path(),
        &["worktree", "remove", "--", &worktree.to_string_lossy()],
    );
    assert!(!worktree.exists());
    assert!(!gone_dir.exists(), "the task's state outlived its worktree");

    let listing = ahu_in(repo.path(), &["tasks"]);
    let text = text_of(&listing);
    assert!(listing.status.success(), "{text}");
    assert!(
        !text.contains(gone),
        "a removed task is still listed: {text}"
    );
    assert!(text.contains(kept), "{text}");

    // A worktree deleted with `rm -rf` rather than through Git leaves no state
    // behind either, and the listing handles it without an error.
    let kept_worktree = repo.path().join(".worktrees").join(kept);
    std::fs::remove_dir_all(&kept_worktree).unwrap();
    let listing = ahu_in(repo.path(), &["tasks"]);
    let text = text_of(&listing);
    assert!(listing.status.success(), "{text}");
    assert!(!text.contains(kept), "{text}");
    assert!(text.contains(ahu::commands::NO_TASKS), "{text}");
}

/// Records written before task state moved into worktrees are still read.
#[test]
fn a_record_in_the_checkout_store_is_still_listed_beside_worktree_records() {
    let repo = fixture();
    let live = "006aa50000000000l1";
    prepare_task(&repo, repo.path(), live);
    let live_worktree = repo.path().join(".worktrees").join(live);

    let discovered = git::discover(repo.path()).unwrap();
    let identity = discovered.identity();
    let legacy_id = "006aa50000000000l2";
    let legacy_worktree = repo.path().join(".worktrees").join(legacy_id);
    let legacy_dir = state::ensure_checkout_state(repo.path())
        .unwrap()
        .join("repos")
        .join(&identity)
        .join("tasks")
        .join(legacy_id);
    task::save(
        &legacy_dir,
        &record_for(&repo, legacy_id, &legacy_worktree),
        "current work",
    )
    .unwrap();

    for from in [repo.path(), Path::new(&live_worktree)] {
        let listing = ahu_in(from, &["tasks"]);
        let text = text_of(&listing);
        assert!(listing.status.success(), "{text}");
        assert!(text.contains(live), "{text}");
        // Also from a sibling worktree: a record in the primary checkout's own
        // store is the repository's, not the invoking checkout's.
        assert!(
            text.contains(legacy_id),
            "an older record was dropped when listing from {from:?}: {text}"
        );
    }
}

/// A link where a task worktree's state should be is reported, not followed.
#[cfg(unix)]
#[test]
fn a_link_in_place_of_a_worktree_state_directory_is_reported_not_followed() {
    let repo = fixture();
    let readable = "006aa50000000000h1";
    let hostile = "006aa50000000000h2";
    prepare_task(&repo, repo.path(), readable);

    let elsewhere = tempfile::TempDir::new().unwrap();
    let discovered = git::discover(repo.path()).unwrap();
    let worktree = state::worktree_dir(repo.path(), hostile).unwrap();
    git::add_worktree(&discovered, &worktree, "ahu/chris/hostile", "HEAD").unwrap();
    // A record the link would expose if the walk followed it.
    let planted = elsewhere
        .path()
        .join("state/repos")
        .join(discovered.identity())
        .join("tasks")
        .join(hostile);
    task::save(&planted, &record_for(&repo, hostile, &worktree), "planted").unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), worktree.join(".ahu")).unwrap();

    let listing = ahu_in(repo.path(), &["tasks"]);
    let text = text_of(&listing);
    assert!(listing.status.success(), "{text}");
    assert!(text.contains(readable), "{text}");
    assert!(
        text.contains("could not be read") && text.contains(hostile),
        "the refused worktree must still be reported: {text}"
    );
    assert!(
        !text.contains("task 006aa50000000000h2 —"),
        "the planted record must not be presented as this task's: {text}"
    );
}

/// A link in place of `.worktrees/` itself is refused rather than enumerated.
#[cfg(unix)]
#[test]
fn a_link_in_place_of_the_worktrees_directory_is_refused() {
    let repo = fixture();
    let elsewhere = tempfile::TempDir::new().unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), repo.path().join(".worktrees")).unwrap();

    let listing = ahu_in(repo.path(), &["tasks"]);
    let text = text_of(&listing);
    assert!(!listing.status.success(), "{text}");
    assert!(text.contains("symlink"), "{text}");
}

/// A record found in one task's worktree must belong to that task.
#[test]
fn a_record_held_by_another_tasks_worktree_does_not_start_a_session() {
    let repo = fixture();
    let owner = "006aa50000000000o1";
    let other = "006aa50000000000o2";
    prepare_task(&repo, repo.path(), owner);
    prepare_task(&repo, repo.path(), other);

    let discovered = git::discover(repo.path()).unwrap();
    let owner_worktree = state::worktree_dir(repo.path(), owner).unwrap();
    let other_worktree = state::worktree_dir(repo.path(), other).unwrap();
    // `other`'s record, complete and self-consistent, sitting in `owner`'s
    // store. Everything it says about itself is true; the only thing wrong is
    // where it was read from.
    let planted = state::worktree_task_dir(&owner_worktree, &discovered.identity(), other);
    task::save(
        &planted,
        &record_for(&repo, other, &other_worktree),
        "current work",
    )
    .unwrap();

    let started = ahu_in(
        repo.path(),
        &["run-task", "--task-dir", &planted.to_string_lossy()],
    );
    let text = text_of(&started);
    assert!(!started.status.success(), "{text}");
    assert!(
        text.contains("a different task's checkout"),
        "the refusal must say why: {text}"
    );
}

/// A link where a task directory should be is reported, not skipped.
///
/// Skipping it would make `ahu tasks` show fewer tasks than the store has
/// entries for, which is the same false claim of absence an unreadable record
/// used to produce.
#[cfg(unix)]
#[test]
fn a_linked_task_directory_is_reported_rather_than_passed_over() {
    let repo = fixture();
    let real = "006aa50000000000s1";
    let linked = "006aa50000000000s2";
    let real_dir = prepare_task(&repo, repo.path(), real);
    std::os::unix::fs::symlink(&real_dir, real_dir.parent().unwrap().join(linked)).unwrap();

    let listing = ahu_in(repo.path(), &["tasks"]);
    let text = text_of(&listing);
    assert!(listing.status.success(), "{text}");
    assert!(text.contains(real), "{text}");
    assert!(
        text.contains(linked) && text.contains("could not be read"),
        "the linked entry must be reported: {text}"
    );
    assert!(text.contains("symlink"), "{text}");
}

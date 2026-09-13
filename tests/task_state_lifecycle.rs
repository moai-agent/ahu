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

/// `AHU_STATE_DIR` is process-wide, so the tests that touch it are serialised.
///
/// The suite can be run from inside an ahu task session, which injects the
/// variable. These tests are about what ahu does without one, so they clear it
/// for the calls that read it rather than inheriting whatever launched them.
static STATE_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn without_override<T>(f: impl FnOnce() -> T) -> T {
    let guard = STATE_ENV.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: every mutation of this variable in this binary is under the same
    // guard, and the readers that matter run inside it.
    unsafe { std::env::remove_var("AHU_STATE_DIR") };
    let result = f();
    drop(guard);
    result
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
        // Listing reconciles against cmux when it can reach one. These tests
        // are about what is on disk, so they are pointed at a cmux that is not
        // there: neither a developer's real session nor another test's stub can
        // change what they see.
        .env("AHU_CMUX_BIN", dir.join("no-such-cmux"))
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
    let target = "006aa50000000000s3";
    let real_dir = prepare_task(&repo, repo.path(), real);
    let target_dir = prepare_task(&repo, repo.path(), target);

    // A link standing where this worktree's own record belongs. It is refused
    // as that task being unreadable, not followed to whatever it points at.
    let store = real_dir.parent().unwrap().to_path_buf();
    std::fs::remove_dir_all(&real_dir).unwrap();
    std::os::unix::fs::symlink(&target_dir, store.join(real)).unwrap();
    // And a link under someone else's task id in the same store, which is not
    // a task of this worktree at all.
    let foreign = "006aa50000000000s2";
    std::os::unix::fs::symlink(&target_dir, store.join(foreign)).unwrap();

    let listed = ahu_in(repo.path(), &["tasks"]);
    let text = text_of(&listed);
    assert!(listed.status.success(), "{text}");
    assert!(text.contains(target), "{text}");
    assert!(
        text.contains(real) && text.contains("symlink"),
        "a link at the worktree's own record must be refused as that task: {text}"
    );
    assert!(
        text.contains(foreign) && text.contains("not its own task state"),
        "a link under another task's id must be named without claiming to be a task: {text}"
    );
    // Neither link produced a second row for the task it points at.
    let discovered = git::discover(repo.path()).unwrap();
    let listing = without_override(|| task::list(&discovered).unwrap());
    assert_eq!(
        listing
            .records
            .iter()
            .filter(|(_, r)| r.task_id == target)
            .count(),
        1,
        "{:?}",
        listing.records
    );
}

/// A record copied into another task's store is not presented as a task, and
/// does not make the task it names ambiguous.
#[test]
fn a_transplanted_record_is_named_but_never_listed_as_a_task() {
    let repo = fixture();
    let owner = "006aa50000000000t1";
    let other = "006aa50000000000t2";
    prepare_task(&repo, repo.path(), owner);
    prepare_task(&repo, repo.path(), other);

    let discovered = git::discover(repo.path()).unwrap();
    let owner_worktree = repo.path().join(".worktrees").join(owner);
    let other_worktree = repo.path().join(".worktrees").join(other);
    // `other`'s complete, self-consistent record, sitting in `owner`'s store.
    task::save(
        &state::worktree_task_dir(&owner_worktree, &discovered.identity(), other),
        &record_for(&repo, other, &other_worktree),
        "current work",
    )
    .unwrap();

    let listing = without_override(|| task::list(&discovered).unwrap());
    assert_eq!(
        listing
            .records
            .iter()
            .filter(|(_, r)| r.task_id == other)
            .count(),
        1,
        "the copy must not enter the listing as a second {other}"
    );
    assert_eq!(listing.records.len(), 2);
    assert!(
        listing.notes.iter().any(|note| note.contains(other)),
        "the stray record must still be named: {:?}",
        listing.notes
    );
    // And it carries no task id of its own, so nothing became ambiguous.
    assert!(listing.unreadable.is_empty(), "{:?}", listing.unreadable);

    // Exact lookup of both tasks still resolves, from the CLI.
    for id in [owner, other] {
        let inspected = ahu_in(repo.path(), &["task", id, "--output", "json"]);
        let text = text_of(&inspected);
        assert!(inspected.status.success(), "{id}: {text}");
        let value: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(value["task_id"], id);
    }
    // `task_dirs` accounts for two tasks, not three.
    assert_eq!(
        without_override(|| ahu::commands::task_dirs(&discovered).unwrap()).len(),
        2
    );

    let listed = ahu_in(repo.path(), &["tasks"]);
    let text = text_of(&listed);
    assert!(listed.status.success(), "{text}");
    assert!(
        text.contains("not its own task state"),
        "the stray record must be reported: {text}"
    );
}

/// A record whose directory name is not the task it names, and one written for
/// another repository, are both refused where a checkout store holds them.
#[test]
fn a_record_that_does_not_match_where_it_was_found_is_refused() {
    let repo = fixture();
    let discovered = git::discover(repo.path()).unwrap();
    let store = state::ensure_checkout_state(repo.path())
        .unwrap()
        .join("repos")
        .join(discovered.identity())
        .join("tasks");

    // Named for one task, holding another's record.
    let mismatched = record_for(
        &repo,
        "006aa50000000000m9",
        &repo.path().join(".worktrees/x"),
    );
    task::save(
        &store.join("006aa50000000000m1"),
        &mismatched,
        "current work",
    )
    .unwrap();
    // A valid record from a different repository.
    let mut foreign = record_for(
        &repo,
        "006aa50000000000m2",
        &repo.path().join(".worktrees/y"),
    );
    foreign.repo_identity = "0".repeat(16);
    task::save(&store.join("006aa50000000000m2"), &foreign, "current work").unwrap();

    let listing = without_override(|| task::list(&discovered).unwrap());
    assert!(listing.records.is_empty(), "{:?}", listing.records);
    let reasons: Vec<&str> = listing
        .unreadable
        .iter()
        .map(|u| u.reason.as_str())
        .collect();
    assert_eq!(listing.unreadable.len(), 2, "{reasons:?}");
    assert!(
        reasons.iter().any(|r| r.contains("is named for task")),
        "{reasons:?}"
    );
    assert!(
        reasons.iter().any(|r| r.contains("different repository")),
        "{reasons:?}"
    );
}

/// A task from the older layout has a worktree with no state in it and its
/// record in a checkout store. That is complete, not incomplete.
#[test]
fn an_older_layout_task_with_a_live_worktree_is_listed_once() {
    let repo = fixture();
    let task_id = "006aa50000000000w1";
    let discovered = git::discover(repo.path()).unwrap();
    state::ensure_worktrees_root(repo.path()).unwrap();
    let worktree = state::worktree_dir(repo.path(), task_id).unwrap();
    git::add_worktree(
        &discovered,
        &worktree,
        &format!("ahu/chris/{task_id}"),
        "HEAD",
    )
    .unwrap();
    assert!(!worktree.join(".ahu").exists(), "the older layout has none");

    // The record where the older ahu put it: the primary checkout's own store.
    task::save(
        &state::ensure_checkout_state(repo.path())
            .unwrap()
            .join("repos")
            .join(discovered.identity())
            .join("tasks")
            .join(task_id),
        &record_for(&repo, task_id, &worktree),
        "current work",
    )
    .unwrap();

    // From the primary checkout and from that worktree itself.
    for from in [repo.path(), worktree.as_path()] {
        let listed = ahu_in(from, &["tasks"]);
        let text = text_of(&listed);
        assert!(listed.status.success(), "{text}");
        assert!(text.contains(task_id), "from {from:?}: {text}");
        assert!(
            !text.contains("no record anywhere"),
            "a worktree whose record is in a checkout store is not incomplete: {text}"
        );
        assert!(!text.contains("could not be read"), "{text}");
    }
    let listing = without_override(|| task::list(&git::discover(&worktree).unwrap()).unwrap());
    assert_eq!(listing.records.len(), 1);
    assert!(listing.unreadable.is_empty(), "{:?}", listing.unreadable);
}

/// A task worktree with no record anywhere is reported, not omitted.
#[test]
fn a_worktree_with_no_record_anywhere_is_reported_as_incomplete() {
    let repo = fixture();
    let stranded = "006aa50000000000i1";
    let discovered = git::discover(repo.path()).unwrap();
    state::ensure_worktrees_root(repo.path()).unwrap();
    let worktree = state::worktree_dir(repo.path(), stranded).unwrap();
    git::add_worktree(
        &discovered,
        &worktree,
        &format!("ahu/chris/{stranded}"),
        "HEAD",
    )
    .unwrap();

    let listed = ahu_in(repo.path(), &["tasks"]);
    let text = text_of(&listed);
    assert!(listed.status.success(), "{text}");
    assert!(text.contains(stranded), "{text}");
    assert!(text.contains("no record anywhere"), "{text}");
    // And the branch it is holding is recovered for the reader.
    assert!(text.contains(&format!("ahu/chris/{stranded}")), "{text}");
}

/// The library answers from the repository it is given, wherever the process
/// happens to be standing.
#[test]
fn discovery_uses_the_repository_it_is_given_not_the_working_directory() {
    let repo = fixture();
    let live = "006aa50000000000q1";
    prepare_task(&repo, repo.path(), live);
    let sibling = repo.path().join(".worktrees").join(live);

    let legacy = "006aa50000000000q2";
    let discovered = git::discover(repo.path()).unwrap();
    task::save(
        &state::ensure_checkout_state(repo.path())
            .unwrap()
            .join("repos")
            .join(discovered.identity())
            .join("tasks")
            .join(legacy),
        &record_for(&repo, legacy, &repo.path().join(".worktrees").join(legacy)),
        "current work",
    )
    .unwrap();

    // The same primary repository, resolved while standing in three different
    // places, including one that is not a repository at all.
    let elsewhere = tempfile::TempDir::new().unwrap();
    let guard = STATE_ENV.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: guarded as above; this binary is the only mutator.
    unsafe { std::env::remove_var("AHU_STATE_DIR") };
    let original = std::env::current_dir().unwrap();
    for cwd in [repo.path(), sibling.as_path(), elsewhere.path()] {
        std::env::set_current_dir(cwd).unwrap();
        let listing = task::list(&discovered).expect("a valid repository lists from any cwd");
        let ids: Vec<&str> = listing
            .records
            .iter()
            .map(|(_, r)| r.task_id.as_str())
            .collect();
        assert!(ids.contains(&live), "from {cwd:?}: {ids:?}");
        assert!(
            ids.contains(&legacy),
            "the primary store's record must not depend on the working directory; from {cwd:?}: {ids:?}"
        );
    }
    std::env::set_current_dir(original).unwrap();
    drop(guard);
}

/// An unrelated explicit store replaces the checkout store and nothing more.
///
/// It is deliberately *not* isolation from live tasks: worktree records stay
/// visible, and `ahu tasks` may update a live task's own record in its own
/// worktree. What the override does guarantee is that no task payload is
/// written into the store the caller chose.
#[test]
fn an_explicit_store_still_sees_live_worktree_tasks() {
    let repo = fixture();
    let live = "006aa50000000000e1";
    prepare_task(&repo, repo.path(), live);

    let legacy = "006aa50000000000e2";
    let discovered = git::discover(repo.path()).unwrap();
    task::save(
        &state::ensure_checkout_state(repo.path())
            .unwrap()
            .join("repos")
            .join(discovered.identity())
            .join("tasks")
            .join(legacy),
        &record_for(&repo, legacy, &repo.path().join(".worktrees").join(legacy)),
        "current work",
    )
    .unwrap();

    let chosen = tempfile::TempDir::new().unwrap();
    let listed = std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
        .args(["tasks"])
        .current_dir(repo.path())
        .env("AHU_STATE_DIR", chosen.path())
        .output()
        .expect("ahu runs");
    let text = text_of(&listed);
    assert!(listed.status.success(), "{text}");
    assert!(
        text.contains(live),
        "an override must not hide a live task: {text}"
    );
    assert!(
        !text.contains(legacy),
        "an override replaces the checkout store, so its records are not read: {text}"
    );
    // No task payload was written into the chosen store.
    assert!(!chosen.path().join("repos").exists(), "{text}");
}

/// A cmux stand-in that answers only what `execute` asks before it creates the
/// worktree: a ping and a capability list. Anything after that is not reached
/// by these tests, which fail the launch in between.
fn stub_cmux(dir: &Path) -> PathBuf {
    let script = dir.join("stub-cmux");
    std::fs::write(
        &script,
        "#!/bin/sh\n\
         case \"$1\" in\n\
         ping) exit 0 ;;\n\
         capabilities) printf '%s' '{\"capabilities\":[\"workspace.groups.v1\",\
\"workspace.group_create.v1\",\"workspace.create_in_group.v1\"]}' ; exit 0 ;;\n\
         *) echo 'the stub does not answer that' >&2 ; exit 1 ;;\n\
         esac\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    script
}

/// Run `execute` against the stub, with no state override, serialised because
/// both settings are process-wide.
fn execute_with_stub(
    repo: &TestRepo,
    cmux: &Path,
    plan: &ahu::launch::LaunchPlan,
) -> ahu::util::Result<ahu::launch::Launched> {
    let discovered = git::discover(repo.path()).unwrap();
    let loaded = ahu::config::load(&discovered.root).unwrap().unwrap();
    let guard = STATE_ENV.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: guarded as everywhere else in this binary.
    unsafe {
        std::env::remove_var("AHU_STATE_DIR");
        std::env::set_var("AHU_CMUX_BIN", cmux);
    }
    let result = ahu::launch::execute(&discovered, &loaded, plan, "do the thing", false);
    unsafe { std::env::remove_var("AHU_CMUX_BIN") };
    drop(guard);
    result
}

/// A launch that fails after the worktree exists, on a worktree Git will not
/// remove, says so and leaves the checkout discoverable.
#[cfg(unix)]
#[test]
fn a_failure_after_materialization_reports_the_worktree_it_could_not_remove() {
    let repo = fixture();
    // Committed so the new worktree gets it from HEAD: a state ignore file that
    // ignores nothing, which state preparation refuses.
    repo.write(".ahu/.gitignore", "# ignores nothing\n");
    repo.commit("state ignore that ignores nothing");
    // Repaired only in the invoking checkout, so its own store still works.
    std::fs::write(
        repo.path().join(".ahu/.gitignore"),
        "# Local ahu session state. Never commit.\n*\n",
    )
    .unwrap();
    // Uncommitted agent configuration, copied into the worktree by
    // materialization, which is what makes the new checkout dirty.
    repo.write("CLAUDE.md", "uncommitted guidance\n");

    let plan = plan_from(repo.path());
    let scratch = tempfile::TempDir::new().unwrap();
    let error = execute_with_stub(&repo, &stub_cmux(scratch.path()), &plan)
        .expect_err("state preparation must fail on the committed ignore file")
        .to_string();

    assert!(error.contains("must ignore all state files"), "{error}");
    assert!(
        error.contains("could not be removed"),
        "the refusal to clean up must be reported: {error}"
    );
    assert!(error.contains(&plan.branch), "{error}");
    assert!(
        error.contains(&plan.worktree.to_string_lossy().to_string()),
        "{error}"
    );
    assert!(plan.worktree.is_dir(), "dirty work must be preserved");

    // And the retained checkout is not omitted from the listing.
    let listed = ahu_in(repo.path(), &["tasks"]);
    let text = text_of(&listed);
    assert!(listed.status.success(), "{text}");
    assert!(text.contains(&plan.task_id), "{text}");
    assert!(text.contains("no record anywhere"), "{text}");
    assert!(text.contains(&plan.branch), "{text}");
}

/// The cleanup that follows a failed launch is not a way out of the checkout.
///
/// The failure being cleaned up here is a committed link at the worktree's
/// `.ahu`. Removing the task directory by name would follow that same link.
#[cfg(unix)]
#[test]
fn a_failed_rollback_does_not_delete_through_a_redirected_state_path() {
    let repo = fixture();
    let external = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(external.path().join("state/repos")).unwrap();
    std::fs::write(external.path().join("state/keep.txt"), "untouched\n").unwrap();

    // Committed link at `.ahu`, plus uncommitted configuration so the new
    // checkout is dirty and Git refuses to remove it.
    std::os::unix::fs::symlink(external.path(), repo.path().join(".ahu")).unwrap();
    repo.commit("state directory that is a link");
    std::fs::remove_file(repo.path().join(".ahu")).unwrap();
    repo.write("CLAUDE.md", "uncommitted guidance\n");

    let plan = plan_from(repo.path());
    let scratch = tempfile::TempDir::new().unwrap();
    let error = execute_with_stub(&repo, &stub_cmux(scratch.path()), &plan)
        .expect_err("a linked state directory must fail the launch")
        .to_string();
    assert!(error.contains("refusing ahu state path"), "{error}");

    // The external directory is exactly as it was: same entries, same bytes.
    let mut entries: Vec<String> = std::fs::read_dir(external.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        vec!["state".to_string()],
        "external directory changed"
    );
    assert_eq!(
        std::fs::read_to_string(external.path().join("state/keep.txt")).unwrap(),
        "untouched\n"
    );
    assert!(external.path().join("state/repos").is_dir());
}

/// A stray record in a worktree's store does not stand in for the record that
/// worktree never got, and the note survives an otherwise empty listing.
#[test]
fn a_stray_record_does_not_hide_a_worktree_with_no_record_of_its_own() {
    let repo = fixture();
    let owner = "006aa50000000000y1";
    let stray = "006aa50000000000y2";
    let discovered = git::discover(repo.path()).unwrap();
    state::ensure_worktrees_root(repo.path()).unwrap();
    let worktree = state::worktree_dir(repo.path(), owner).unwrap();
    git::add_worktree(
        &discovered,
        &worktree,
        &format!("ahu/chris/{owner}"),
        "HEAD",
    )
    .unwrap();
    state::ensure_checkout_state(&worktree).unwrap();
    // Someone else's record, and nothing of the worktree's own.
    task::save(
        &state::worktree_task_dir(&worktree, &discovered.identity(), stray),
        &record_for(&repo, stray, &repo.path().join(".worktrees").join(stray)),
        "current work",
    )
    .unwrap();

    let listing = without_override(|| task::list(&discovered).unwrap());
    assert!(listing.records.is_empty(), "{:?}", listing.records);
    assert_eq!(listing.unreadable.len(), 1, "{:?}", listing.unreadable);
    assert_eq!(listing.unreadable[0].task_id, owner);
    assert!(
        listing.unreadable[0].reason.contains("no record anywhere"),
        "{}",
        listing.unreadable[0].reason
    );
    assert!(
        listing.notes.iter().any(|note| note.contains(stray)),
        "{:?}",
        listing.notes
    );

    let listed = ahu_in(repo.path(), &["tasks"]);
    let text = text_of(&listed);
    assert!(listed.status.success(), "{text}");
    assert!(
        text.contains(stray) && text.contains("not listed as a task"),
        "{text}"
    );
    assert!(
        text.contains(owner) && text.contains("no record anywhere"),
        "{text}"
    );
}

/// A note is printed even when there is nothing else to print.
#[test]
fn a_note_survives_a_listing_with_no_tasks_in_it() {
    let repo = fixture();
    let owner = "006aa50000000000z1";
    let stray = "006aa50000000000z2";
    let discovered = git::discover(repo.path()).unwrap();
    let worktree = state::worktree_dir(repo.path(), owner).unwrap();
    state::ensure_worktrees_root(repo.path()).unwrap();
    git::add_worktree(
        &discovered,
        &worktree,
        &format!("ahu/chris/{owner}"),
        "HEAD",
    )
    .unwrap();
    task::save(
        &state::worktree_task_dir(&worktree, &discovered.identity(), stray),
        &record_for(&repo, stray, &repo.path().join(".worktrees").join(stray)),
        "current work",
    )
    .unwrap();
    // Remove the worktree's Git registration but keep the directory, so it is
    // not a task and produces no row -- only the note is left to print.
    std::fs::remove_dir_all(&worktree).unwrap();
    std::fs::create_dir_all(worktree.join(".ahu/state")).unwrap();
    std::fs::write(worktree.join(".ahu/.gitignore"), "*\n").unwrap();
    task::save(
        &state::worktree_task_dir(&worktree, &discovered.identity(), stray),
        &record_for(&repo, stray, &repo.path().join(".worktrees").join(stray)),
        "current work",
    )
    .unwrap();

    let listed = ahu_in(repo.path(), &["tasks"]);
    let text = text_of(&listed);
    assert!(listed.status.success(), "{text}");
    assert!(
        text.contains(stray) && text.contains("not listed as a task"),
        "a note must not be swallowed by an empty listing: {text}"
    );
}

/// A chosen store inside a repository is the caller's, not ahu's own wiring.
#[test]
fn a_store_below_a_checkout_root_is_treated_as_a_chosen_store() {
    let repo = fixture();
    let discovered = git::discover(repo.path()).unwrap();
    let inside = repo.path().join("sub/.ahu/state");
    std::fs::create_dir_all(&inside).unwrap();

    let guard = STATE_ENV.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: guarded as everywhere else in this binary.
    unsafe { std::env::set_var("AHU_STATE_DIR", &inside) };
    let chosen = state::isolated_store(&discovered).unwrap();
    // And the real wiring, a checkout root's own store, is not mistaken for one.
    unsafe { std::env::set_var("AHU_STATE_DIR", repo.path().join(".ahu/state")) };
    let wiring = state::isolated_store(&discovered).unwrap();
    unsafe { std::env::remove_var("AHU_STATE_DIR") };
    drop(guard);

    assert_eq!(
        chosen.as_deref(),
        Some(inside.as_path()),
        "a store under a subdirectory is a chosen store, not ahu's own"
    );
    assert_eq!(wiring, None, "a checkout root's own store is ahu's wiring");
}

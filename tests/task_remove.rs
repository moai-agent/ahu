//! `ahu remove` takes a finished task's record, worktree and branch away in
//! one explicit action.
//!
//! Every gate runs before anything is removed. A live task is a cancellation,
//! not a removal, and a dirty worktree or a branch holding commits the primary
//! checkout does not have keeps its work for review.

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
    common::ahu()
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

/// The worktree ahu would create for `task_id` in this repository.
fn worktree_of(repo: &TestRepo, task_id: &str) -> PathBuf {
    state::worktree_dir(&git::discover(repo.path()).unwrap().root, task_id).unwrap()
}

/// The branch ahu would create for `task_id`.
fn branch_of(task_id: &str) -> String {
    format!("ahu/chris/{task_id}")
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

/// Create a task worktree and the record that belongs to it, the way `execute`
/// does: worktree first, then its own state directory, then the record. The
/// record's state is the caller's to choose.
fn prepare_task_in_state(repo: &TestRepo, task_id: &str, task_state: task::TaskState) -> PathBuf {
    let discovered = git::discover(repo.path()).unwrap();
    state::ensure_worktrees_root(&discovered.root).unwrap();
    let worktree = state::worktree_dir(&discovered.root, task_id).unwrap();
    let branch = branch_of(task_id);
    git::add_worktree(&discovered, &worktree, &branch, "HEAD").unwrap();
    state::ensure_checkout_state(&worktree).unwrap();

    let identity = discovered.identity();
    let dir = state::worktree_task_dir(&worktree, &identity, task_id);
    let mut record = record_for(repo, task_id, &worktree);
    record.state = task_state;
    task::save(&dir, &record, "current work").unwrap();
    dir
}

/// A finished task, ready to be removed.
fn prepare_task(repo: &TestRepo, task_id: &str) -> PathBuf {
    prepare_task_in_state(repo, task_id, task::TaskState::Exited)
}

/// Removal clears the record, the worktree and the branch together, and the
/// task is gone from the listing afterwards.
#[test]
fn remove_clears_record_worktree_and_branch_together() {
    let repo = fixture();
    let id = "006aa50000000000m1";
    let record_dir = prepare_task(&repo, id);
    let worktree = worktree_of(&repo, id);
    let branch = branch_of(id);

    let out = ahu_in(repo.path(), &["remove", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains(&format!("removed task {id}")), "{text}");
    assert!(text.contains("  worktree  removed\n"), "{text}");
    assert!(text.contains("  branch    deleted\n"), "{text}");
    assert!(
        text.contains("  record    removed with its worktree\n"),
        "{text}"
    );
    assert!(!worktree.exists(), "the worktree outlived removal");
    assert!(!record_dir.exists(), "the record outlived removal");
    assert_eq!(
        common::git(repo.path(), &["branch", "--list", &branch]),
        "",
        "the branch outlived removal"
    );

    let out = ahu_in(repo.path(), &["tasks"]);
    let text = text_of(&out);
    assert!(out.status.success(), "{text}");
    assert!(text.contains(ahu::commands::NO_TASKS), "{text}");
    assert!(!text.contains(id), "{text}");
}

/// A live task is a cancellation, not a removal, and is told so.
#[test]
fn remove_refuses_a_live_task_and_points_at_cancel() {
    let repo = fixture();
    let id = "006aa50000000000m2";
    let record_dir = prepare_task_in_state(&repo, id, task::TaskState::Running);
    let worktree = worktree_of(&repo, id);
    let branch = branch_of(id);

    let out = ahu_in(repo.path(), &["remove", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(5), "{text}");
    assert!(text.contains(&format!("task {id} is running")), "{text}");
    assert!(text.contains("not a terminal state"), "{text}");
    assert!(text.contains(&format!("ahu cancel {id}")), "{text}");
    assert!(text.contains("nothing was removed"), "{text}");
    assert!(worktree.is_dir(), "a live task lost its worktree");
    assert!(record_dir.is_dir(), "a live task lost its record");
    assert!(
        git::branch_exists(&git::discover(repo.path()).unwrap(), &branch).unwrap(),
        "a live task lost its branch"
    );
}

/// Uncommitted changes are reviewable work; removal waits until they are gone.
#[test]
fn remove_refuses_a_dirty_worktree_until_it_is_clean() {
    let repo = fixture();
    let id = "006aa50000000000m3";
    let record_dir = prepare_task(&repo, id);
    let worktree = worktree_of(&repo, id);
    let branch = branch_of(id);
    std::fs::write(worktree.join("scratch.txt"), "unreviewed").unwrap();

    let out = ahu_in(repo.path(), &["remove", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(5), "{text}");
    assert!(text.contains("has uncommitted changes"), "{text}");
    assert!(text.contains("nothing was removed"), "{text}");
    assert!(worktree.is_dir(), "a dirty worktree was removed");
    assert!(record_dir.is_dir(), "a dirty worktree's record was removed");
    assert!(
        git::branch_exists(&git::discover(repo.path()).unwrap(), &branch).unwrap(),
        "a dirty worktree's branch was deleted"
    );

    std::fs::remove_file(worktree.join("scratch.txt")).unwrap();
    let out = ahu_in(repo.path(), &["remove", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(
        !worktree.exists(),
        "a clean worktree was kept after removal"
    );
    assert!(!record_dir.exists(), "a clean worktree's record was kept");
}

/// Commits that exist only on the task's branch are kept; removal waits until
/// they are merged.
#[test]
fn remove_refuses_a_branch_with_unmerged_commits_until_it_is_merged() {
    let repo = fixture();
    let id = "006aa50000000000m4";
    let record_dir = prepare_task(&repo, id);
    let worktree = worktree_of(&repo, id);
    let branch = branch_of(id);
    common::git(&worktree, &["commit", "--allow-empty", "-m", "work"]);

    let out = ahu_in(repo.path(), &["remove", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(5), "{text}");
    assert!(
        text.contains("commits that are not in the primary checkout's current branch"),
        "{text}"
    );
    assert!(text.contains("nothing was removed"), "{text}");
    assert!(
        worktree.is_dir(),
        "an unmerged branch's worktree was removed"
    );
    assert!(
        record_dir.is_dir(),
        "an unmerged branch's record was removed"
    );
    assert!(
        git::branch_exists(&git::discover(repo.path()).unwrap(), &branch).unwrap(),
        "an unmerged branch was deleted"
    );

    common::git(repo.path(), &["merge", "--ff-only", &branch]);
    let out = ahu_in(repo.path(), &["remove", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(!worktree.exists(), "a merged branch's worktree was kept");
    assert!(!record_dir.exists(), "a merged branch's record was kept");
    assert_eq!(
        common::git(repo.path(), &["branch", "--list", &branch]),
        "",
        "a merged branch was kept after removal"
    );
}

/// A task that was never launched does not exist to be removed.
#[test]
fn remove_names_the_missing_task_as_a_usage_error() {
    let repo = fixture();
    let id = "006aa50000000000m5";
    let record_dir = prepare_task(&repo, id);

    let out = ahu_in(repo.path(), &["remove", "006aa50000000000q5"]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(2), "{text}");
    assert!(text.contains("no task matching"), "{text}");
    assert!(record_dir.is_dir(), "an unknown id removed a task");
}

/// Whatever is already gone is reported as such, and the rest is still removed.
#[test]
fn remove_reports_a_branch_that_is_already_absent() {
    let repo = fixture();
    let id = "006aa50000000000m6";
    let record_dir = prepare_task(&repo, id);
    let worktree = worktree_of(&repo, id);
    let branch = branch_of(id);
    common::git(&worktree, &["checkout", "--detach"]);
    common::git(repo.path(), &["branch", "-d", &branch]);

    let out = ahu_in(repo.path(), &["remove", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("  worktree  removed\n"), "{text}");
    assert!(text.contains("  branch    already absent\n"), "{text}");
    assert!(
        text.contains("  record    removed with its worktree\n"),
        "{text}"
    );
    assert!(!worktree.exists(), "the worktree outlived removal");
    assert!(!record_dir.exists(), "the record outlived removal");
}

/// Removing a task from inside its own worktree would take the ground out
/// from under the command; ahu refuses.
#[test]
fn remove_refuses_to_run_inside_the_worktree_being_removed() {
    let repo = fixture();
    let id = "006aa50000000000m7";
    let record_dir = prepare_task(&repo, id);
    let worktree = worktree_of(&repo, id);

    let out = ahu_in(&worktree, &["remove", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(5), "{text}");
    assert!(
        text.contains("running inside the worktree being removed"),
        "{text}"
    );
    assert!(text.contains("from another checkout"), "{text}");
    assert!(
        worktree.is_dir(),
        "the worktree was removed from under itself"
    );
    assert!(record_dir.is_dir(), "the record outlived the refusal");
}

/// A record left in a checkout store by an older ahu is removed from there,
/// with the worktree and branch it names reported as already absent.
#[test]
fn remove_clears_a_record_left_in_the_checkout_store() {
    let repo = fixture();
    let id = "006aa50000000000m8";
    let discovered = git::discover(repo.path()).unwrap();
    let identity = discovered.identity();
    let dir = state::worktree_task_dir(repo.path(), &identity, id);
    let mut record = record_for(&repo, id, repo.path());
    record.worktree = state::worktree_dir(repo.path(), id).unwrap();
    task::save(&dir, &record, "current work").unwrap();
    assert!(dir.is_dir(), "the record was not written");

    let out = ahu_in(repo.path(), &["remove", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("  worktree  already absent\n"), "{text}");
    assert!(text.contains("  branch    already absent\n"), "{text}");
    assert!(text.contains("  record    removed\n"), "{text}");
    assert!(!dir.exists(), "the record outlived removal");
}

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
            release_id: None,
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
        session_host: None,
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

#[test]
fn removed_task_handles_stay_reserved_and_cannot_retarget_a_new_task() {
    let repo = fixture();
    let id = ahu::task::new_task_id().unwrap();
    prepare_task(&repo, &id);
    let discovered = ahu::git::discover(repo.path()).unwrap();
    ahu::task_handles::reserve(&discovered, &id, Some("old-task"), "").unwrap();
    let output = ahu_in(repo.path(), &["remove", "@old-task"]);
    assert!(output.status.success(), "{}", text_of(&output));
    assert!(!worktree_of(&repo, &id).exists());
    assert_eq!(
        ahu::task_handles::resolve(&discovered, "@old-task").unwrap(),
        id
    );
    let output = ahu_in(repo.path(), &["task", "@old-task"]);
    assert!(!output.status.success());
    assert!(text_of(&output).contains("no task matching"));
    assert!(
        ahu::task_handles::reserve(
            &discovered,
            &ahu::task::new_task_id().unwrap(),
            Some("old-task"),
            ""
        )
        .is_err()
    );
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

/// A task whose CMUX workspace has disappeared is terminal even when its old
/// record still says running. Removal reconciles that evidence first, then
/// still applies the clean-worktree and merged-branch gates.
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

#[test]
fn unavailable_plugin_preserves_legacy_cmux_records_and_owned_resources() {
    let repo = fixture();
    let id = "006aa50000000000plugin";
    let dir = prepare_task(&repo, id);
    let worktree = worktree_of(&repo, id);
    let mut record = task::load(&dir).unwrap();
    record.cmux_workspace_id = Some("legacy-workspace".into());
    record.cmux_group_id = Some("legacy-group".into());
    record.cmux_window_id = Some("legacy-window".into());
    task::save(&dir, &record, "current work").unwrap();
    let before = std::fs::read(dir.join("task.json")).unwrap();
    for command in ["focus", "remove"] {
        let output = common::ahu()
            .current_dir(repo.path())
            .args([command, id])
            .env("AHU_INTERACTIVE_PLUGIN", "none")
            .output()
            .unwrap();
        let text = text_of(&output);
        assert!(!output.status.success(), "{text}");
        assert!(
            text.contains("legacy task has no recorded external session-host binding"),
            "{text}"
        );
        assert_eq!(std::fs::read(dir.join("task.json")).unwrap(), before);
        assert!(worktree.is_dir());
        assert!(git::branch_exists(&git::discover(repo.path()).unwrap(), &branch_of(id)).unwrap());
    }
}

fn prepare_headless_task(repo: &TestRepo, id: &str) -> PathBuf {
    use std::os::unix::fs::OpenOptionsExt;
    let old = prepare_task(repo, id);
    let record = task::load(&old).unwrap();
    std::fs::remove_dir_all(old).unwrap();
    let dir = ahu::headless::store(&git::discover(repo.path()).unwrap())
        .unwrap()
        .join(id);
    task::save(&dir, &record, "current work").unwrap();
    state::write_json(
        &dir.join("headless.json"),
        &serde_json::json!({
            "schema_version": 2, "options": ahu::headless::Options::default(),
            "harness_version": "synthetic", "executable_digest": "synthetic",
            "parent_task": null, "root_task": null, "depth": 0, "attempt": 1,
            "session": "synthetic", "native_controls": [], "gaps": []
        }),
    )
    .unwrap();
    state::create_private_dir_all(&dir.join("attempt-1")).unwrap();
    state::write_json(
        &dir.join("attempt-1/result.json"),
        &serde_json::json!({
            "schema_version": 2, "task_id": id, "attempt": 1, "outcome": "succeeded",
            "process": {"exit_code": 0}, "harness": {"terminal": true, "failed": false}
        }),
    )
    .unwrap();
    std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(dir.join("owner.lock"))
        .unwrap();
    dir
}

fn hold_owner(dir: &Path) -> std::fs::File {
    use std::os::fd::AsRawFd;
    let file = std::fs::File::open(dir.join("owner.lock")).unwrap();
    assert_eq!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    file
}

fn assert_task_preserved(repo: &TestRepo, id: &str, dir: &Path, before: &[u8]) {
    assert_eq!(std::fs::read(dir.join("task.json")).unwrap(), before);
    assert!(dir.join("headless.json").is_file());
    assert!(dir.join("attempt-1/result.json").is_file());
    assert!(worktree_of(repo, id).is_dir());
    assert!(git::branch_exists(&git::discover(repo.path()).unwrap(), &branch_of(id)).unwrap());
}

#[test]
fn remove_headless_refuses_resume_owner_without_removing_resources() {
    let repo = fixture();
    let id = "00000000-0000-7000-8000-000000000001";
    let dir = prepare_headless_task(&repo, id);
    let before = std::fs::read(dir.join("task.json")).unwrap();
    // Deterministically model resume after it acquires ownership, before it
    // publishes Starting. The public removal sees a terminal record.
    let held = hold_owner(&dir);
    let out = ahu_in(repo.path(), &["remove", id]);
    assert!(!out.status.success(), "{}", text_of(&out));
    assert!(text_of(&out).contains("owned"), "{}", text_of(&out));
    assert_task_preserved(&repo, id, &dir, &before);
    drop(held);
    let out = ahu_in(repo.path(), &["remove", id]);
    assert!(out.status.success(), "{}", text_of(&out));
    assert!(!dir.exists());
    assert!(!worktree_of(&repo, id).exists());
    assert!(!git::branch_exists(&git::discover(repo.path()).unwrap(), &branch_of(id)).unwrap());
}

#[test]
fn remove_headless_preserves_coordination_when_git_refuses_worktree_removal() {
    let repo = fixture();
    let id = "00000000-0000-7000-8000-000000000002";
    let dir = prepare_headless_task(&repo, id);
    let before = std::fs::read(dir.join("task.json")).unwrap();
    let worktree = worktree_of(&repo, id);
    common::git(
        repo.path(),
        &["worktree", "lock", worktree.to_str().unwrap()],
    );
    let out = ahu_in(repo.path(), &["remove", id]);
    assert!(!out.status.success(), "{}", text_of(&out));
    assert_task_preserved(&repo, id, &dir, &before);
    common::git(
        repo.path(),
        &["worktree", "unlock", worktree.to_str().unwrap()],
    );
    let out = ahu_in(repo.path(), &["remove", id]);
    assert!(out.status.success(), "{}", text_of(&out));
}

#[test]
fn remove_headless_does_not_block_on_unrelated_ownership() {
    let repo = fixture();
    let id = "00000000-0000-7000-8000-000000000003";
    let other = "00000000-0000-7000-8000-000000000004";
    let dir = prepare_headless_task(&repo, id);
    let other_dir = prepare_headless_task(&repo, other);
    task::set_state(&other_dir, task::TaskState::Running).unwrap();
    let before = std::fs::read(other_dir.join("task.json")).unwrap();
    let _held = hold_owner(&other_dir);
    let out = ahu_in(repo.path(), &["remove", id]);
    assert!(out.status.success(), "{}", text_of(&out));
    assert!(!dir.exists());
    assert_task_preserved(&repo, other, &other_dir, &before);
}

#[test]
fn remove_headless_refuses_uncertain_metadata_without_side_effects() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let repo = fixture();
    let id = "00000000-0000-7000-8000-000000000005";
    let dir = prepare_headless_task(&repo, id);
    let before = std::fs::read(dir.join("task.json")).unwrap();
    let spec = std::fs::read(dir.join("headless.json")).unwrap();
    let result = std::fs::read(dir.join("attempt-1/result.json")).unwrap();
    for variant in [
        "missing-lock",
        "symlink-lock",
        "hardlink-lock",
        "public-lock",
        "missing-spec",
        "malformed-spec",
        "missing-result",
        "malformed-result",
        "wrong-attempt",
        "uncertain-result",
    ] {
        let lock = dir.join("owner.lock");
        match variant {
            "missing-lock" => std::fs::remove_file(&lock).unwrap(),
            "symlink-lock" => {
                std::fs::remove_file(&lock).unwrap();
                symlink(dir.join("prompt.txt"), &lock).unwrap();
            }
            "hardlink-lock" => std::fs::hard_link(&lock, dir.join("alias.lock")).unwrap(),
            "public-lock" => {
                std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o644)).unwrap()
            }
            "missing-spec" => std::fs::remove_file(dir.join("headless.json")).unwrap(),
            "malformed-spec" => std::fs::write(dir.join("headless.json"), "{").unwrap(),
            "missing-result" => std::fs::remove_file(dir.join("attempt-1/result.json")).unwrap(),
            "malformed-result" => std::fs::write(dir.join("attempt-1/result.json"), "{").unwrap(),
            "wrong-attempt" | "uncertain-result" => {
                let mut value: serde_json::Value = serde_json::from_slice(&result).unwrap();
                if variant == "wrong-attempt" {
                    value["attempt"] = 2.into();
                } else {
                    value["outcome"] = "supervisor_error".into();
                }
                state::write_json(&dir.join("attempt-1/result.json"), &value).unwrap();
            }
            _ => unreachable!(),
        }
        let out = ahu_in(repo.path(), &["remove", id]);
        assert!(!out.status.success(), "{variant}: {}", text_of(&out));
        assert_eq!(
            std::fs::read(dir.join("task.json")).unwrap(),
            before,
            "{variant}"
        );
        assert!(worktree_of(&repo, id).is_dir(), "{variant}");
        assert!(
            git::branch_exists(&git::discover(repo.path()).unwrap(), &branch_of(id)).unwrap(),
            "{variant}"
        );
        if variant == "missing-lock" {
            assert!(!lock.exists());
        }
        if variant == "hardlink-lock" {
            std::fs::remove_file(dir.join("alias.lock")).unwrap();
        }
        if std::fs::symlink_metadata(&lock).is_ok() {
            std::fs::remove_file(&lock).unwrap();
        }
        state::write_private_file(&lock, b"").unwrap();
        state::write_private_file(&dir.join("headless.json"), &spec).unwrap();
        state::write_private_file(&dir.join("attempt-1/result.json"), &result).unwrap();
    }
    let out = ahu_in(repo.path(), &["remove", id]);
    assert!(out.status.success(), "{}", text_of(&out));
}

#[test]
fn remove_headless_cleans_up_a_proven_prelaunch_failure() {
    let repo = fixture();
    let id = "00000000-0000-7000-8000-000000000006";
    let dir = prepare_headless_task(&repo, id);
    let worktree = worktree_of(&repo, id);
    let mut result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("attempt-1/result.json")).unwrap()).unwrap();
    result["outcome"] = "supervisor_error".into();
    result["failure_phase"] = "frozen_execution_validation".into();
    state::write_json(&dir.join("attempt-1/result.json"), &result).unwrap();
    task::set_state(&dir, task::TaskState::Failed).unwrap();

    let out = ahu_in(repo.path(), &["remove", id]);
    let text = text_of(&out);
    assert!(out.status.success(), "{text}");
    assert!(!worktree.exists(), "the proven prelaunch worktree remains");
    assert!(!dir.exists(), "the proven prelaunch task record remains");
    assert!(!git::branch_exists(&git::discover(repo.path()).unwrap(), &branch_of(id)).unwrap());
}

#[test]
fn remove_headless_keeps_prelaunch_failure_with_spawn_evidence() {
    let repo = fixture();
    let id = "00000000-0000-7000-8000-000000000007";
    let dir = prepare_headless_task(&repo, id);
    let mut result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("attempt-1/result.json")).unwrap()).unwrap();
    result["outcome"] = "supervisor_error".into();
    result["failure_phase"] = "frozen_execution_validation".into();
    state::write_json(&dir.join("attempt-1/result.json"), &result).unwrap();
    state::write_json(
        &dir.join("attempt-1/spawn-intent.json"),
        &serde_json::json!({"attempt":1}),
    )
    .unwrap();
    task::set_state(&dir, task::TaskState::Failed).unwrap();
    let before = std::fs::read(dir.join("task.json")).unwrap();

    let out = ahu_in(repo.path(), &["remove", id]);
    assert!(!out.status.success(), "{}", text_of(&out));
    assert_task_preserved(&repo, id, &dir, &before);
}

/// A configuration file the parent working tree holds but never committed.
/// A user's global ignore rule commonly hides exactly this from their own
/// `git status`; ahu reads Git with no global configuration and sees it.
const LOCAL_CONFIG: &str = ".claude/settings.local.json";

/// A finished task whose worktree holds the configuration ahu copied out of
/// the parent working tree, recorded the way a launch records it.
fn prepare_task_with_copied_config(repo: &TestRepo, task_id: &str) -> (PathBuf, PathBuf) {
    let discovered = git::discover(repo.path()).unwrap();
    state::ensure_worktrees_root(&discovered.root).unwrap();
    let worktree = state::worktree_dir(&discovered.root, task_id).unwrap();
    git::add_worktree(&discovered, &worktree, &branch_of(task_id), "HEAD").unwrap();
    state::ensure_checkout_state(&worktree).unwrap();

    let snapshot = ahu::snapshot::collect(&discovered.root).unwrap();
    let materialized = ahu::snapshot::materialize(&discovered.root, &snapshot, &worktree).unwrap();
    assert!(
        materialized.written.iter().any(|p| p == LOCAL_CONFIG),
        "the fixture did not copy the uncommitted parent configuration: {materialized:?}"
    );
    assert!(
        worktree.join(LOCAL_CONFIG).is_file(),
        "the copied configuration is missing from the task worktree"
    );

    let identity = discovered.identity();
    let dir = state::worktree_task_dir(&worktree, &identity, task_id);
    let mut record = record_for(repo, task_id, &worktree);
    record.config_snapshot_digest = snapshot.digest();
    record.config_snapshot = snapshot;
    record.materialize = materialized;
    task::save(&dir, &record, "current work").unwrap();
    (dir, worktree)
}

/// Configuration ahu copied in and nobody touched is not the task's work.
///
/// The copy is untracked in the task worktree for the whole life of the task,
/// and ahu reads Git with no global configuration, so it is reported even
/// where the user's own ignore rules hide the same file in the parent. A task
/// that changed nothing must still be removable.
#[test]
fn remove_takes_a_worktree_holding_only_unchanged_copied_config() {
    let repo = fixture();
    repo.write(LOCAL_CONFIG, "{\"permissions\":{}}\n");
    let id = "006aa50000000000c1";
    let (record_dir, worktree) = prepare_task_with_copied_config(&repo, id);
    let branch = branch_of(id);

    let out = ahu_in(repo.path(), &["remove", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("  worktree  removed\n"), "{text}");
    assert!(!worktree.exists(), "the worktree outlived removal");
    assert!(!record_dir.exists(), "the record outlived removal");
    assert_eq!(
        common::git(repo.path(), &["branch", "--list", &branch]),
        "",
        "the branch outlived removal"
    );
    // Removal reads the parent; it never writes to it.
    assert_eq!(
        repo.read(LOCAL_CONFIG),
        "{\"permissions\":{}}\n",
        "removal disturbed the parent checkout's configuration"
    );
}

/// A copied configuration file the task edited is reviewable work, and so is
/// an untracked file no snapshot accounts for. Neither is removed, and the
/// refusal names them.
#[test]
fn remove_keeps_a_worktree_whose_copied_config_changed_or_holds_extra_files() {
    let repo = fixture();
    repo.write(LOCAL_CONFIG, "{\"permissions\":{}}\n");
    let id = "006aa50000000000c2";
    let (record_dir, worktree) = prepare_task_with_copied_config(&repo, id);
    let branch = branch_of(id);

    // The task changed the copy it inherited.
    std::fs::write(
        worktree.join(LOCAL_CONFIG),
        "{\"permissions\":{\"allow\":[]}}\n",
    )
    .unwrap();
    let out = ahu_in(repo.path(), &["remove", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(5), "{text}");
    assert!(text.contains("has uncommitted changes"), "{text}");
    assert!(text.contains(LOCAL_CONFIG), "{text}");
    assert!(worktree.is_dir(), "changed configuration was removed");
    assert!(record_dir.is_dir(), "changed configuration lost its record");
    assert!(
        git::branch_exists(&git::discover(repo.path()).unwrap(), &branch).unwrap(),
        "changed configuration lost its branch"
    );

    // Restored to what ahu copied, the worktree is removable again -- except
    // for an untracked file of the task's own that nothing accounts for.
    std::fs::write(worktree.join(LOCAL_CONFIG), "{\"permissions\":{}}\n").unwrap();
    std::fs::create_dir_all(worktree.join(".claude/notes")).unwrap();
    std::fs::write(worktree.join(".claude/notes/scratch.md"), "findings\n").unwrap();
    let out = ahu_in(repo.path(), &["remove", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(5), "{text}");
    assert!(text.contains("has uncommitted changes"), "{text}");
    assert!(text.contains(".claude/notes/scratch.md"), "{text}");
    assert!(
        worktree.join(LOCAL_CONFIG).is_file(),
        "a refused removal deleted the copied configuration"
    );
    assert!(worktree.is_dir(), "an untracked file was removed");
    assert!(record_dir.is_dir(), "an untracked file lost its record");

    std::fs::remove_dir_all(worktree.join(".claude/notes")).unwrap();
    let out = ahu_in(repo.path(), &["remove", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(!worktree.exists(), "the clean worktree was kept");
    assert!(!record_dir.exists(), "the clean worktree's record was kept");
}

/// An inherited ignore file can hide another copied config file from Git's
/// status output. The removal gate must still account for every copy recorded
/// at launch before it starts deleting them.
#[test]
fn remove_accounts_for_copies_hidden_by_an_inherited_ignore_file() {
    let repo = fixture();
    repo.write(".claude/.gitignore", "settings.local.json\n");
    repo.write(LOCAL_CONFIG, "{\"permissions\":{}}\n");
    let id = "006aa50000000000c3";
    let (record_dir, worktree) = prepare_task_with_copied_config(&repo, id);
    let branch = branch_of(id);

    let discovered = git::discover(&worktree).unwrap();
    let changes = git::changes(&discovered).unwrap();
    assert!(
        !changes.untracked.iter().any(|path| path == LOCAL_CONFIG),
        "fixture must hide the copied settings file from ordinary untracked status: {changes:?}"
    );

    let out = ahu_in(repo.path(), &["remove", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(!worktree.exists(), "the worktree outlived removal");
    assert!(!record_dir.exists(), "the record outlived removal");
    assert_eq!(
        common::git(repo.path(), &["branch", "--list", &branch]),
        "",
        "the branch outlived removal"
    );
    assert_eq!(repo.read(LOCAL_CONFIG), "{\"permissions\":{}}\n");
    assert_eq!(repo.read(".claude/.gitignore"), "settings.local.json\n");
}

/// If another ignored file prevents Git from removing the worktree after
/// copied configuration is discarded, keep the task record and branch so the
/// operator can inspect and retry the partial cleanup.
#[test]
fn remove_preserves_task_record_and_branch_when_git_refuses_worktree_cleanup() {
    let repo = fixture();
    repo.write(".claude/.gitignore", "settings.local.json\nnotes.md\n");
    repo.write(LOCAL_CONFIG, "{\"permissions\":{}}\n");
    let id = "006aa50000000000c4";
    let (record_dir, worktree) = prepare_task_with_copied_config(&repo, id);
    let branch = branch_of(id);
    std::fs::write(worktree.join(".claude/notes.md"), "task work\n").unwrap();

    let out = ahu_in(repo.path(), &["remove", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(5), "{text}");
    assert!(
        text.contains("Task record and branch were preserved"),
        "{text}"
    );
    assert!(
        worktree.is_dir(),
        "the refused worktree removal deleted its checkout"
    );
    assert!(
        record_dir.is_dir(),
        "a failed cleanup deleted its task record"
    );
    assert!(worktree.join(".claude/notes.md").is_file());
    assert!(
        git::branch_exists(&git::discover(repo.path()).unwrap(), &branch).unwrap(),
        "a failed cleanup deleted its branch"
    );

    // Once the task's ignored artifact is removed, the exposed status is
    // reviewable and must be handled before cleanup can finish.
    std::fs::remove_file(worktree.join(".claude/notes.md")).unwrap();
    let out = ahu_in(repo.path(), &["remove", id]);
    let text = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(!worktree.exists());
    assert!(!record_dir.exists());
}

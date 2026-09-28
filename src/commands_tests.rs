//! Command behavior with isolated repositories and scripted terminal input.
use super::*;
use std::process::Command;

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn repository() -> (tempfile::TempDir, Repo) {
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-q"]);
    git(
        root.path(),
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "fixture",
        ],
    );
    let repo = crate::git::discover(root.path()).unwrap();
    (root, repo)
}

fn configured(repo: &Repo) {
    config::write_new(
        &repo.root,
        &config::ProjectConfig {
            schema_version: 1,
            harness_preferences: vec!["claude-code".into()],
            model_selection: "project-ranked".into(),
            catalog_version: catalog::CATALOG_VERSION.into(),
            model_rankings: BTreeMap::from([("claude-code".into(), vec!["claude-opus-5".into()])]),
            knowledge: Default::default(),
            telemetry: Default::default(),
        },
    )
    .unwrap();
}

fn registered(repo: &Repo) -> ResolvedAgent {
    let manifests = repo.root.join(config::AGENTS_RELATIVE_DIR);
    std::fs::create_dir_all(&manifests).unwrap();
    std::fs::write(
        manifests.join("reviewer.md"),
        "---\nokf_version: 0.2\ntype: ahu:agent\ntitle: reviewer\nversion: 1.0.0\nharness: claude-code\nmodel: claude-opus-5\n---\nReview carefully.\n",
    )
    .unwrap();
    agent::load_all(&repo.root).unwrap().remove(0)
}

fn scripted(input: &str, f: impl FnOnce(&mut Console<'_>) -> Result<i32>) -> (Result<i32>, String) {
    let mut input = std::io::Cursor::new(input.as_bytes());
    let mut output = Vec::new();
    let result = f(&mut Console {
        input: &mut input,
        output: &mut output,
        interactive: true,
    });
    (result, String::from_utf8(output).unwrap())
}

fn saved(repo: &Repo, id: &str) -> (PathBuf, task::TaskRecord) {
    let adapter = harness::adapter_for("claude-code").unwrap();
    let record = task::TaskRecord {
        schema_version: crate::task::TASK_SCHEMA_VERSION,
        task_id: id.to_string(),
        title: "Synthetic task title".to_string(),
        summary: String::new(),
        created_at: crate::task::now_rfc3339(),
        repo_identity: repo.identity(),
        repo_root: repo.root.clone(),
        branch: "ahu/auto/gone0001".to_string(),
        worktree: repo.root.clone(),
        base_commit: repo.head.clone(),
        identity: crate::task::LaunchIdentity {
            mode: crate::task::LaunchMode::Automatic,
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
        catalog_version: crate::catalog::CATALOG_VERSION.to_string(),
        config_snapshot: Default::default(),
        config_snapshot_digest: "0".repeat(64),
        hooks: Default::default(),
        hooks_digest: String::new(),
        delivery: crate::orchestration::deliver(None, "prompt").unwrap().1,
        prompt_digest: String::new(),
        harness_executable: std::path::PathBuf::from("claude"),
        materialize: Default::default(),
        launch_command: adapter
            .launch_command(&crate::harness::LaunchRequest {
                model: "claude-opus-5",
                prompt: "synthetic prompt",
                cwd: &repo.root,
                permissions: Default::default(),
            })
            .unwrap(),
        reliability_warning: None,
        enforcement: adapter
            .enforcement("claude-opus-5", Default::default())
            .unwrap(),
        cmux_group_id: None,
        cmux_workspace_id: None,
        cmux_window_id: None,
        state: crate::task::TaskState::Exited,
    };
    let dir = repo
        .root
        .join(".ahu/state/repos")
        .join(repo.identity())
        .join("tasks")
        .join(id);
    task::save(&dir, &record, "synthetic prompt").unwrap();
    (dir, record)
}

#[test]
fn onboarding_confirmation_registration_and_removal_preserve_native_file_and_index() {
    let (_root, repo) = repository();
    let native = repo.root.join(".claude/agents/reviewer.md");
    std::fs::create_dir_all(native.parent().unwrap()).unwrap();
    let contents = "---\nmodel: claude-opus-5\ndescription: Review code\n---\nReview carefully.\n";
    std::fs::write(&native, contents).unwrap();
    let manifest = repo.root.join(".agents/ahu/agents/reviewer.md");
    let (result, preview) = scripted("", |c| onboard_cmd(c, &repo, None, None, None, "1.0.0"));
    assert_eq!(result.unwrap(), 0);
    assert!(preview.contains("reviewer"));
    for answer in ["", "n\n"] {
        let (result, output) = scripted(answer, |c| {
            onboard_cmd(c, &repo, Some("reviewer"), None, None, "1.0.0")
        });
        assert_eq!(result.unwrap(), 1);
        assert!(output.contains("Cancelled"));
        assert!(!manifest.exists());
    }
    let (result, output) = scripted("y\n", |c| {
        onboard_cmd(c, &repo, Some("reviewer"), None, None, "1.0.0")
    });
    assert_eq!(result.unwrap(), 0);
    assert!(output.contains("Wrote"));
    let registered = std::fs::read(&manifest).unwrap();
    let (result, output) = scripted("", |c| {
        onboard_cmd(c, &repo, Some("reviewer"), None, None, "2.0.0")
    });
    assert_eq!(result.unwrap(), 0);
    assert!(output.contains("already registered"));
    assert_eq!(std::fs::read(&manifest).unwrap(), registered);
    let (result, output) = scripted("", |c| agents(c, &repo));
    assert_eq!(result.unwrap(), 0);
    assert!(output.contains("@reviewer 1.0.0"));
    let (result, output) = scripted("", |c| {
        onboard_cmd(c, &repo, None, Some("reviewer"), None, "1.0.0")
    });
    assert_eq!(result.unwrap(), 0);
    assert!(output.contains("Removed"));
    assert!(!manifest.exists());
    assert_eq!(std::fs::read_to_string(native).unwrap(), contents);
    git(&repo.root, &["diff", "--cached", "--exit-code"]);
}

#[test]
fn onboarding_rejects_missing_unknown_and_blocked_models_before_confirmation() {
    let (_root, repo) = repository();
    let native = repo.root.join(".claude/agents/reviewer.md");
    std::fs::create_dir_all(native.parent().unwrap()).unwrap();
    for (text, expected) in [
        ("---\nmodel: inherit\n---\nReview", "needs an explicit one"),
        (
            "---\nmodel: nonexistent-model\n---\nReview",
            "cannot register",
        ),
    ] {
        std::fs::write(&native, text).unwrap();
        let (result, output) = scripted("y\n", |c| {
            onboard_cmd(c, &repo, Some("reviewer"), None, None, "1.0.0")
        });
        assert!(result.unwrap_err().to_string().contains(expected));
        assert!(output.is_empty());
        assert!(!repo.root.join(".agents/ahu/agents/reviewer.md").exists());
    }
    let (result, _) = scripted("y\n", |c| {
        onboard_cmd(c, &repo, Some("absent"), None, None, "1.0.0")
    });
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("no native definition")
    );
    std::fs::write(native, "---\nmodel: inherit\n---\nReview").unwrap();
    let (result, _) = scripted("y\n", |c| {
        onboard_cmd(
            c,
            &repo,
            Some("reviewer"),
            None,
            Some("claude-opus-5"),
            "1.0.0",
        )
    });
    assert_eq!(result.unwrap(), 0);
}

#[test]
fn lock_update_requires_commit_and_detects_context_changes() {
    let (_root, repo) = repository();
    let (result, output) = scripted("", |c| lock_cmd(c, &repo, false));
    assert!(result.is_err());
    assert!(output.contains("Context is not launchable"));
    std::fs::write(repo.root.join("AGENTS.md"), "Review carefully.").unwrap();
    let (result, output) = scripted("", |c| lock_cmd(c, &repo, true));
    assert_eq!(result.unwrap(), 0);
    assert!(output.contains("Wrote"));
    git(&repo.root, &["diff", "--cached", "--exit-code"]);
    assert!(scripted("", |c| lock_cmd(c, &repo, false)).0.is_err());
    git(&repo.root, &["add", "AGENTS.md", "ahu.lock"]);
    git(
        &repo.root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "context",
        ],
    );
    assert_eq!(scripted("", |c| lock_cmd(c, &repo, false)).0.unwrap(), 0);
    std::fs::write(repo.root.join("AGENTS.md"), "Changed instructions.").unwrap();
    assert!(scripted("", |c| lock_cmd(c, &repo, false)).0.is_err());
}

#[test]
fn commands_report_missing_setup_and_empty_agents_without_creating_config() {
    let (_root, repo) = repository();
    let (result, text) = scripted("", |c| agents(c, &repo));
    assert_eq!(result.unwrap(), 0);
    assert!(text.contains("No ahu agents"));
    for result in [
        scripted("", |c| interactive(c, &repo, false, None)).0,
        scripted("", |c| knowledge_lint(c, &repo, false)).0,
        scripted("", |c| {
            launch_cmd(
                c,
                &repo,
                "reviewer",
                "task",
                false,
                true,
                false,
                &Default::default(),
            )
        })
        .0,
    ] {
        assert!(result.unwrap_err().to_string().contains("setup"));
    }
    assert!(!config::config_path(&repo.root).exists());
    configured(&repo);
    let (result, output) = scripted("", |c| knowledge_lint(c, &repo, false));
    assert!(result.unwrap_err().to_string().contains("bundle"));
    assert!(output.is_empty());
}

#[test]
fn task_inspection_renders_bounded_artifacts_and_keeps_json_metadata_only() {
    let (_root, repo) = repository();
    let (dir, record) = saved(&repo, "inspect01");
    for name in ["result.md", "question.md"] {
        std::fs::write(dir.join(name), "synthetic-secret\u{202e}").unwrap();
    }
    let (result, output) = scripted("", |c| task_cmd(c, &repo, "inspect01", false));
    assert_eq!(result.unwrap(), 0);
    assert!(output.contains("result:"));
    assert!(output.contains("question:"));
    assert!(output.contains("\\u{202e}"));
    assert!(!output.contains('\u{202e}'));
    let (result, output) = scripted("", |c| task_cmd(c, &repo, "inspect01", true));
    assert_eq!(result.unwrap(), 0);
    let json: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(json["task_id"], record.task_id);
    assert_eq!(json["session_state"], "exited");
    assert_eq!(json["completion_verified"], false);
    assert!(!output.contains("synthetic-secret"));
    for name in ["result.md", "question.md"] {
        std::fs::write(dir.join(name), vec![b'x'; TASK_ARTIFACT_LIMIT as usize + 1]).unwrap();
    }
    let (result, output) = scripted("", |c| task_cmd(c, &repo, "inspect01", false));
    assert_eq!(result.unwrap(), 0);
    assert_eq!(
        output
            .matches("larger than the 1 MiB display bound")
            .count(),
        2
    );
    for name in ["result.md", "question.md"] {
        std::fs::remove_file(dir.join(name)).unwrap();
        std::fs::create_dir(dir.join(name)).unwrap();
    }
    let (result, output) = scripted("", |c| task_cmd(c, &repo, "inspect01", false));
    assert_eq!(result.unwrap(), 0);
    assert!(output.contains("cannot safely read result.md"));
    assert!(output.contains("cannot safely read question.md"));
}

#[test]
fn task_resolution_distinguishes_missing_ambiguous_and_unreadable_records() {
    let (_root, repo) = repository();
    let (dir, _) = saved(&repo, "record01");
    saved(&repo, "record02");
    assert_eq!(inspect_task(&repo, "record01").unwrap().0, dir);
    assert!(
        inspect_task(&repo, "record")
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
    );
    assert!(
        scripted("", |c| task_cmd(c, &repo, "record", false))
            .0
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
    );
    assert!(
        scripted("", |c| focus(c, &repo, "record01"))
            .0
            .unwrap_err()
            .to_string()
            .contains("no recorded cmux session")
    );
    assert!(
        scripted("", |c| focus(c, &repo, "absent"))
            .0
            .unwrap_err()
            .to_string()
            .contains("no task matching")
    );
    std::fs::write(dir.join("task.json"), "{}").unwrap();
    assert!(
        inspect_task(&repo, "record01")
            .unwrap_err()
            .to_string()
            .contains("unreadable")
    );
    assert!(
        scripted("", |c| task_cmd(c, &repo, "record01", false))
            .0
            .unwrap_err()
            .to_string()
            .contains("unreadable")
    );
    assert!(
        scripted("", |c| remove_cmd(c, &repo, "record01"))
            .0
            .unwrap_err()
            .to_string()
            .contains("unreadable")
    );
    assert!(
        scripted("", |c| focus(c, &repo, "record01"))
            .0
            .unwrap_err()
            .to_string()
            .contains("cannot read its record")
    );
    assert!(
        scripted("", |c| focus(c, &repo, "absent"))
            .0
            .unwrap_err()
            .to_string()
            .contains("other task record")
    );
    assert!(
        scripted("", |c| task_cmd(c, &repo, "absent", false))
            .0
            .unwrap_err()
            .to_string()
            .contains("no task matching")
    );
    assert!(
        scripted("", |c| remove_cmd(c, &repo, "absent"))
            .0
            .unwrap_err()
            .to_string()
            .contains("no task matching")
    );
    assert!(
        inspect_task(&repo, "absent")
            .unwrap_err()
            .to_string()
            .contains("no task matching")
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("task.json")).unwrap(),
        "{}"
    );
}

#[test]
fn removal_refuses_live_tasks_and_current_checkout_but_cleans_absent_terminal_work() {
    let (_root, repo) = repository();
    let (dir, mut record) = saved(&repo, "remove01");
    record.state = task::TaskState::Running;
    task::save(&dir, &record, "synthetic prompt").unwrap();
    assert!(
        scripted("", |c| remove_cmd(c, &repo, "remove01"))
            .0
            .unwrap_err()
            .to_string()
            .contains("not a terminal state")
    );
    assert!(dir.exists());
    record.state = task::TaskState::Exited;
    task::save(&dir, &record, "synthetic prompt").unwrap();
    assert!(
        scripted("", |c| remove_cmd(c, &repo, "remove01"))
            .0
            .unwrap_err()
            .to_string()
            .contains("running inside the worktree")
    );
    assert!(dir.exists());
    record.worktree = repo.root.join(".worktrees/absent");
    task::save(&dir, &record, "synthetic prompt").unwrap();
    let (result, output) = scripted("", |c| remove_cmd(c, &repo, "remove01"));
    assert_eq!(result.unwrap(), 0);
    assert!(output.contains("record    removed"));
    assert_eq!(output.matches("already absent").count(), 2);
    assert!(!dir.exists());
    assert!(repo.root.join(".git").exists());
}

#[test]
fn indexed_task_loading_rejects_stale_corrupt_and_mismatched_records() {
    let (_root, repo) = repository();
    let (dir, record) = saved(&repo, "indexed01");
    let entry = crate::task_index::Entry {
        schema_version: crate::task_index::INDEX_SCHEMA_VERSION,
        task_id: record.task_id.clone(),
        repo_identity: repo.identity(),
        checkout: repo.root.clone(),
        store: crate::task_index::StoreKind::Worktree,
    };
    assert_eq!(load_indexed_task(&entry).unwrap().0, dir);
    for changed in ["task", "repository", "checkout"] {
        let mut mismatched = record.clone();
        match changed {
            "task" => mismatched.task_id = "different01".into(),
            "repository" => mismatched.repo_identity = "different".into(),
            "checkout" => mismatched.worktree = repo.root.join("absent"),
            _ => unreachable!(),
        }
        task::save(&dir, &mismatched, "synthetic prompt").unwrap();
        assert!(
            load_indexed_task(&entry)
                .unwrap_err()
                .to_string()
                .contains("different task")
        );
    }
    std::fs::write(dir.join("task.json"), "{}").unwrap();
    assert!(
        load_indexed_task(&entry)
            .unwrap_err()
            .to_string()
            .contains("unreadable record")
    );
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        load_indexed_task(&entry)
            .unwrap_err()
            .to_string()
            .contains("task directory")
    );
    let mut stale = entry.clone();
    stale.checkout = repo.root.join("absent");
    assert!(
        load_indexed_task(&stale)
            .unwrap_err()
            .to_string()
            .contains("checkout no longer exists")
    );
    let mut headless = entry;
    headless.store = crate::task_index::StoreKind::Headless;
    headless.repo_identity = "different".into();
    assert!(
        load_indexed_task(&headless)
            .unwrap_err()
            .to_string()
            .contains("repository identity mismatch")
    );
}

#[test]
fn listing_limit_preserves_questions_and_unreadable_task_disclosure() {
    let (_root, repo) = repository();
    let (result, text) = scripted("", |c| tasks(c, &repo));
    assert_eq!(result.unwrap(), 0);
    assert!(text.contains(NO_TASKS));
    let (first, _) = saved(&repo, "listing01");
    let (second, _) = saved(&repo, "listing02");
    std::fs::write(first.join("question.md"), "First question\nMore detail").unwrap();
    std::fs::write(second.join("question.md"), "Second question").unwrap();
    let (result, text) = scripted("", |c| tasks_limited_at(c, &repo, 240, Some(1)));
    assert_eq!(result.unwrap(), 0);
    assert!(text.contains("Showing 1 of 2 tasks"));
    assert_eq!(text.matches("question  ").count(), 1);
    assert!(!text.contains("More detail"));
    let (result, text) = scripted("", |c| tasks_with_limit(c, &repo, None));
    assert_eq!(result.unwrap(), 0);
    assert_eq!(text.matches("question  ").count(), 2);
    std::fs::write(second.join("task.json"), "{}").unwrap();
    let (result, text) = scripted("", |c| tasks_at(c, &repo, 80));
    assert_eq!(result.unwrap(), 0);
    assert!(text.contains("listing02 [unreadable]"));
    assert!(!text.contains(NO_TASKS));
}

#[test]
fn unreadable_listing_recovers_branch_and_reports_lost_git_metadata() {
    let (_root, repo) = repository();
    git(&repo.root, &["branch", "ahu/auto/recover01"]);
    let unreadable = ["recover01", "recover02"].map(|id| task::UnreadableTask {
        dir: repo.root.join("synthetic-record"),
        task_id: id.into(),
        reason: "unsupported record".into(),
    });
    let output = render_unreadable_tasks(&repo, &unreadable);
    assert!(output.contains("branch    ahu/auto/recover01"));
    assert!(output.contains("no longer on disk"));
    std::fs::remove_dir_all(repo.root.join(".git")).unwrap();
    let output = render_unreadable_tasks(&repo, &unreadable);
    assert!(output.contains("could not be derived from the task id"));
}

#[test]
fn commands_propagate_output_failures() {
    struct BrokenOutput;
    impl std::io::Write for BrokenOutput {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "closed output",
            ))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let (_root, repo) = repository();
    let mut input = std::io::Cursor::new(b"");
    let mut output = BrokenOutput;
    let mut console = Console {
        input: &mut input,
        output: &mut output,
        interactive: false,
    };
    assert!(
        agents(&mut console, &repo)
            .unwrap_err()
            .to_string()
            .contains("closed output")
    );
    assert!(
        tasks(&mut console, &repo)
            .unwrap_err()
            .to_string()
            .contains("closed output")
    );
    saved(&repo, "output01");
    assert!(
        task_cmd(&mut console, &repo, "output01", true)
            .unwrap_err()
            .to_string()
            .contains("closed output")
    );
}

#[test]
fn launch_refuses_widened_approvals_even_during_dry_run() {
    let (_root, repo) = repository();
    configured(&repo);
    let manifests = repo.root.join(config::AGENTS_RELATIVE_DIR);
    std::fs::create_dir_all(&manifests).unwrap();
    for permissions in ["auto", "accept-edits"] {
        std::fs::write(manifests.join("reviewer.md"), format!(
            "---\nokf_version: 0.2\ntype: ahu:agent\ntitle: reviewer\nversion: 1.0.0\nharness: claude-code\nmodel: claude-opus-5\npermissions: {permissions}\n---\nReview carefully.\n"
        )).unwrap();
        for dry_run in [false, true] {
            let (result, output) = scripted("y\n", |c| {
                launch_cmd(
                    c,
                    &repo,
                    "reviewer",
                    "synthetic task",
                    false,
                    dry_run,
                    false,
                    &Default::default(),
                )
            });
            let error = result.unwrap_err().to_string();
            assert!(error.contains("--allow-widened-approvals"), "{error}");
            assert!(error.contains(permissions));
            assert!(output.is_empty());
            assert!(task_dirs(&repo).unwrap().is_empty());
            assert!(!repo.root.join(".worktrees").exists());
        }
    }
}

#[test]
fn agents_distinguish_drift_from_unreadable_history() {
    let (_root, repo) = repository();
    configured(&repo);
    let agent = registered(&repo);
    let (dir, mut record) = saved(&repo, "drift001");
    record.identity.mode = task::LaunchMode::Named;
    record.identity.agent = agent.manifest.name.clone();
    record.identity.agent_version = Some(agent.manifest.version.clone());
    task::save(&dir, &record, "synthetic prompt").unwrap();

    let (result, text) = scripted("", |c| agents(c, &repo));
    assert_eq!(result.unwrap(), 0);
    assert!(text.contains("@reviewer 1.0.0 [drifted]"), "{text}");
    let detail = agent_drift_details(&repo, &[agent]).unwrap();
    assert_eq!(detail.len(), 1);
    assert_eq!(detail[0].agent_name, "reviewer");
    assert!(detail[0].previous_task.contains("drift001"));
    assert!(!detail[0].drift.changes.is_empty());

    std::fs::write(dir.join("task.json"), "{}").unwrap();
    let (result, text) = scripted("", |c| agents(c, &repo));
    assert_eq!(result.unwrap(), 0);
    assert!(text.contains("drift could not be checked"));
    assert!(text.contains("1 earlier task record(s) could not be read"));
    assert!(text.contains("@reviewer 1.0.0"));
    assert!(!text.contains("[drifted]"));
}

#[test]
fn removal_reports_partial_cleanup_when_a_worktree_is_locked() {
    let (_root, repo) = repository();
    let (dir, mut record) = saved(&repo, "locked01");
    record.worktree = repo.root.join(".worktrees/locked01");
    git(
        &repo.root,
        &[
            "worktree",
            "add",
            "-b",
            &record.branch,
            record.worktree.to_str().unwrap(),
            "HEAD",
        ],
    );
    git(
        &repo.root,
        &["worktree", "lock", record.worktree.to_str().unwrap()],
    );
    task::save(&dir, &record, "synthetic prompt").unwrap();

    let (result, output) = scripted("", |c| remove_cmd(c, &repo, &record.task_id));
    let error = result.unwrap_err().to_string();
    assert!(error.contains("was only partly removed"), "{error}");
    assert!(error.contains("Completed:\n  record    removed"), "{error}");
    assert!(error.contains("Not removed:\n  worktree"), "{error}");
    assert!(error.contains("branch"));
    assert!(error.contains("Delete the branch yourself"));
    assert!(output.is_empty());
    assert!(!dir.exists());
    assert!(record.worktree.join(".git").exists());
    assert!(git::branch_exists(&repo, &record.branch).unwrap());
    git(
        &repo.root,
        &["worktree", "unlock", record.worktree.to_str().unwrap()],
    );
    git(
        &repo.root,
        &["worktree", "remove", record.worktree.to_str().unwrap()],
    );
    git(&repo.root, &["branch", "-d", &record.branch]);
}

#[test]
fn removal_refuses_foreign_repository_and_non_checkout_root() {
    let (_root, repo) = repository();
    let (_foreign_root, foreign) = repository();
    let (dir, mut record) = saved(&repo, "foreign01");
    let nested = repo.root.join("nested");
    std::fs::create_dir(&nested).unwrap();
    for target in [&foreign.root, &nested] {
        record.worktree = target.clone();
        task::save(&dir, &record, "synthetic prompt").unwrap();
        let before = std::fs::read(dir.join("task.json")).unwrap();
        let (result, output) = scripted("", |c| remove_cmd(c, &repo, &record.task_id));
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("does not belong to this repository or is not a checkout root")
        );
        assert!(output.is_empty());
        assert!(target.exists());
        assert_eq!(std::fs::read(dir.join("task.json")).unwrap(), before);
    }
    assert!(foreign.root.join(".git").exists());
}

#[test]
fn task_without_a_launch_base_offers_a_quoted_branch_review() {
    let (_root, repo) = repository();
    let (dir, mut record) = saved(&repo, "nobase01");
    record.base_commit = None;
    record.branch = "reviewer's-branch".into();
    task::save(&dir, &record, "synthetic prompt").unwrap();
    let (result, text) = scripted("", |c| task_cmd(c, &repo, &record.task_id, false));
    assert_eq!(result.unwrap(), 0);
    assert!(text.contains("base      unknown"));
    assert!(text.contains(" log 'reviewer'\\''s-branch'"), "{text}");
    assert!(!text.contains(" diff "));
}

#[test]
fn interactive_preselection_rejects_unknown_agent_before_reading_prompt() {
    let (_root, repo) = repository();
    configured(&repo);
    let (result, text) = scripted("this must not become a task\n", |c| {
        interactive(c, &repo, false, Some("absent"))
    });
    assert!(result.unwrap_err().to_string().contains("absent"));
    assert!(text.contains("Preselected agent: @absent"));
    assert!(!text.contains("Resolved for this task"));
    assert!(task::list(&repo).unwrap().is_empty());
    assert!(!repo.root.join(".worktrees").exists());
}

#[test]
fn unreadable_task_prefix_resolves_to_the_diagnostic_record() {
    let (_root, repo) = repository();
    let (dir, _) = saved(&repo, "broken01");
    std::fs::write(dir.join("task.json"), "{}").unwrap();
    let (result, text) = scripted("", |c| task_cmd(c, &repo, "broken", false));
    let error = result.unwrap_err().to_string();
    assert!(error.contains("broken01"));
    assert!(error.contains("unreadable"));
    assert!(text.is_empty());
    assert_eq!(
        std::fs::read_to_string(dir.join("task.json")).unwrap(),
        "{}"
    );
}

#[test]
fn doctor_reports_invalid_agents_and_lock_without_changing_them() {
    let (_root, repo) = repository();
    configured(&repo);
    let config_before = std::fs::read(config::config_path(&repo.root)).unwrap();
    let manifests = repo.root.join(config::AGENTS_RELATIVE_DIR);
    std::fs::create_dir_all(&manifests).unwrap();
    let manifest = manifests.join("broken.md");
    std::fs::write(&manifest, "not an agent manifest").unwrap();
    let lock = repo.root.join("ahu.lock");
    std::fs::write(&lock, "not a context lock").unwrap();
    let repo = Ok(repo);
    for verbose in [false, true] {
        let (result, text) = scripted("", |c| {
            if verbose {
                doctor_with_verbosity(c, &repo, true)
            } else {
                doctor(c, &repo)
            }
        });
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("doctor found blocking problems")
        );
        assert!(text.contains("agents       invalid:"));
        assert!(text.contains("context lock invalid"));
        if !verbose {
            assert!(text.contains("context lock invalid; run `ahu lock` for details"));
        }
    }
    assert_eq!(
        std::fs::read_to_string(manifest).unwrap(),
        "not an agent manifest"
    );
    assert_eq!(std::fs::read_to_string(lock).unwrap(), "not a context lock");
    assert_eq!(
        std::fs::read(config::config_path(&repo.unwrap().root)).unwrap(),
        config_before
    );
}

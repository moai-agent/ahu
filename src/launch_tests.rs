use super::*;
use crate::fixture_runtime as runtime;

const PROMPT: &str = "Inspect café\n$(touch never-executed)";

// Construct the frozen boundary directly: no installed harness, user
// configuration, cmux session or additional Git worktree is needed.
fn fixture() -> (tempfile::TempDir, Repo, LoadedConfig, LaunchPlan) {
    fixture_with_runtime(true)
}
fn fixture_with_runtime(installed: bool) -> (tempfile::TempDir, Repo, LoadedConfig, LaunchPlan) {
    let temp = tempfile::tempdir().unwrap();
    git::run_ok(temp.path(), &["init", "-q"]).unwrap();
    git::run_ok(temp.path(), &["config", "user.name", "Fixture"]).unwrap();
    git::run_ok(
        temp.path(),
        &["config", "user.email", "fixture@example.invalid"],
    )
    .unwrap();
    if installed {
        runtime::install(
            temp.path(),
            "codex",
            "codex",
            ".codex/agents",
            "codex-agent",
            &["gpt-6"],
        );
    }
    let repo = git::discover(temp.path()).unwrap();
    let binding_digest = if installed {
        crate::harness_runtime::load(&repo, "codex")
            .unwrap()
            .binding_digest()
    } else {
        "synthetic-binding".into()
    };
    let config = crate::config::ProjectConfig {
        schema_version: 1,
        harness_preferences: vec!["codex".into()],
        model_selection: "project-ranked".into(),
        harness_version_pins: Default::default(),
        model_rankings: [("codex".into(), vec!["gpt-6".into()])].into(),
        knowledge: Default::default(),
        headless: Default::default(),
        telemetry: Default::default(),
    };
    let loaded = LoadedConfig {
        digest: crate::util::digest_bytes(crate::config::render(&config).as_bytes()),
        config,
        path: repo.root.join(crate::config::CONFIG_RELATIVE_PATH),
    };
    let task_id = task::new_task_id().unwrap();
    let worktree = repo.root.join(".worktrees").join(&task_id);
    let task_dir = state::worktree_task_dir(&worktree, &repo.identity(), &task_id);
    let (delivered, delivery) = crate::orchestration::deliver_composed(
        None,
        PROMPT,
        crate::orchestration::Composition::interactive(Some(crate::orchestration::Metadata {
            task_id: task_id.clone(),
            agent: "auto".into(),
            harness: "codex".into(),
            model: "gpt-6".into(),
            permissions: Default::default(),
        })),
    )
    .unwrap();
    let command = LaunchCommand {
        program: "codex".into(),
        args: vec!["--model".into(), "gpt-6".into(), "--".into(), delivered],
        prompt_arg: Some(3),
        planner_binding_digest: Some(binding_digest.clone()),
        native_eval_otlp: true,
    };
    let plan = LaunchPlan {
        release_id: None,
        mode: LaunchMode::Automatic,
        agent: None,
        pair: ResolvedPair {
            harness: "codex".into(),
            model: "gpt-6".into(),
            basis: "project ranking".into(),
            policy_digest: loaded.digest.clone(),
            runtime_binding_digest: binding_digest,
        },
        enforcement: EnforcementReport {
            harness: "codex".into(),
            harness_version: Some("1.2.3".into()),
            model_fixed_for_session: true,
            gaps: vec![DELIVERY_IS_NOT_ENFORCEMENT.into()],
            applied_controls: vec!["model flag".into()],
        },
        snapshot: Default::default(),
        hooks: Default::default(),
        native_admission: crate::native_admission::inspect(&repo.root, "codex"),
        base_commit: None,
        parent_dirty: false,
        branch: format!("ahu/auto/{task_id}"),
        task_id,
        task_name: None,
        worktree,
        task_dir,
        title: "Inspect café".into(),
        summary: "Fixture summary".into(),
        command,
        delivery,
        permissions: Default::default(),
        harness_executable: "/synthetic/codex".into(),
    };
    (temp, repo, loaded, plan)
}

#[test]
fn display_overrides_keep_assignment_frozen_and_reject_invisible_text() {
    let (_temp, _, _, mut plan) = fixture();
    let command = plan.command.clone();
    let delivery = plan.delivery.clone();
    plan.apply_display(&DisplayMetadata {
        title: Some("Public title".into()),
        name: Some("review".into()),
        summary: None,
    })
    .unwrap();
    assert_eq!(plan.title, "Public title");
    assert_eq!(plan.summary, "Public title");
    assert_eq!(plan.task_name.as_deref(), Some("review"));
    plan.apply_display(&DisplayMetadata {
        summary: Some("Short\nsummary".into()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(plan.summary, "Short summary");
    assert_eq!(plan.command, command);
    assert_eq!(plan.delivery, delivery);
    for (title, summary, flag) in [
        (Some(" \n\t".into()), None, "--title"),
        (None, Some(" \n\t".into()), "--summary"),
    ] {
        let error = plan
            .apply_display(&DisplayMetadata {
                title,
                summary,
                name: None,
            })
            .unwrap_err();
        assert_eq!(error.kind(), crate::util::ErrorKind::Usage);
        assert!(error.to_string().contains(flag));
    }
    assert!(
        plan.apply_display(&DisplayMetadata {
            name: Some("../escape".into()),
            ..Default::default()
        })
        .is_err()
    );
}

#[test]
fn json_preview_redacts_prompt_and_discloses_each_inventory_gap() {
    let (_temp, _, _, mut plan) = fixture();
    let clean: serde_json::Value =
        serde_json::from_str(&render_json(&plan, PROMPT).unwrap()).unwrap();
    assert_eq!(clean["warnings"], serde_json::json!([]));
    assert!(clean["agent"].is_null());
    assert_eq!(plan.agent_label(), "auto");
    plan.permissions = crate::agent::Permissions::Auto;
    plan.parent_dirty = true;
    plan.task_name = Some("review".into());
    plan.hooks.hooks.push(crate::hooks::Hook {
        event: "Stop".into(),
        matcher: None,
        kind: "command".into(),
        command: Some("synthetic-hook-secret".into()),
        command_digest: "digest".into(),
        scope: crate::hooks::Scope::User,
        source: "user-settings.json".into(),
    });
    plan.hooks.unreadable.push("broken-settings.json".into());
    plan.snapshot.skipped_directories.push("vendor".into());
    plan.snapshot
        .unscanned_config
        .push("vendor/AGENTS.md".into());
    plan.snapshot.symlinks.push(".claude/settings.json".into());
    let rendered = render_json(&plan, PROMPT).unwrap();
    assert!(!rendered.contains("never-executed"));
    assert!(!rendered.contains("synthetic-hook-secret"));
    let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["executed"], false);
    assert_eq!(value["task_handle_reserved"], false);
    assert_eq!(value["task_handle_candidate"], "@review");
    assert_eq!(value["prompt_bytes"], PROMPT.len());
    assert_eq!(
        value["prompt_digest"],
        crate::util::digest_bytes(PROMPT.as_bytes())
    );
    assert_eq!(value["argv"][0], "codex");
    let warnings = value["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 7);
    for expected in [
        crate::hooks::NON_PROJECT_HOOK_WARNING,
        plan.permissions.disclosure(),
        "uncommitted changes",
        "broken-settings.json",
        "Directories not scanned or inventoried: vendor",
        "Configuration carried by the checkout but not inventoried: vendor/AGENTS.md",
        "Configuration symlinks not followed or inherited: .claude/settings.json",
    ] {
        assert!(
            warnings
                .iter()
                .any(|w| w.as_str().unwrap().contains(expected)),
            "missing {expected}"
        );
    }
}

#[test]
fn prepared_record_freezes_identity_and_redacts_assignment() {
    let (_temp, repo, loaded, mut plan) = fixture();
    plan.release_id = Some("candidate-a".into());
    let record = prepared_record(&repo, &loaded, &plan, PROMPT, Default::default());
    assert_eq!(record.identity.release_id.as_deref(), Some("candidate-a"));
    let preview: serde_json::Value =
        serde_json::from_str(&render_json(&plan, PROMPT).unwrap()).unwrap();
    assert_eq!(preview["release_id"], "candidate-a");
    assert_eq!(record.state, TaskState::Starting);
    assert_eq!(record.identity.mode, LaunchMode::Automatic);
    assert_eq!(
        record.identity.selection_basis.as_deref(),
        Some("project ranking")
    );
    assert_eq!(record.repo_identity, repo.identity());
    assert_eq!(record.policy_digest, loaded.digest);
    assert_eq!(record.config_snapshot_digest, plan.snapshot.digest());
    assert_eq!(record.hooks_digest, plan.hooks.digest());
    assert_eq!(record.launch_command, plan.command.redacted());
    assert_eq!(record.delivery, plan.delivery);
    assert_eq!(
        record.prompt_digest,
        crate::util::digest_bytes(PROMPT.as_bytes())
    );
    assert!(
        !serde_json::to_string(&record)
            .unwrap()
            .contains("never-executed")
    );
    assert!(record.session_ref.is_none());
}

#[test]
fn verify_refuses_missing_record_without_destroying_work() {
    let (_temp, _, _, plan) = fixture();
    std::fs::create_dir_all(&plan.worktree).unwrap();
    let sentinel = plan.worktree.join("work.txt");
    std::fs::write(&sentinel, "keep me").unwrap();
    let error = verify_task(&plan.task_dir, None).unwrap_err().to_string();
    assert!(
        error.contains("re-submit the work as a new task"),
        "{error}"
    );
    assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "keep me");
}

#[test]
fn verify_refuses_prompt_and_repository_identity_tampering() {
    let (_temp, repo, loaded, plan) = fixture();
    let original = prepared_record(&repo, &loaded, &plan, PROMPT, Default::default());
    // Use a legacy, primary-owned record so a missing worktree is testable.
    let dir = repo.root.join(".ahu/state/fixture");
    task::save(&dir, &original, PROMPT).unwrap();
    let error = verify_task(&dir, None).unwrap_err().to_string();
    assert!(
        error.contains("task worktree") && error.contains("is missing"),
        "{error}"
    );
    std::fs::create_dir_all(&plan.worktree).unwrap();
    for (field, replacement, expected) in [
        ("prompt_digest", "", "no recorded prompt digest"),
        (
            "prompt_digest",
            "changed",
            "does not match the digest recorded",
        ),
        ("repo_identity", "foreign", "different repository"),
    ] {
        let mut value = serde_json::to_value(&original).unwrap();
        value[field] = replacement.into();
        state::write_json(&dir.join("task.json"), &value).unwrap();
        let error = verify_task(&dir, None).unwrap_err().to_string();
        assert!(error.contains(expected), "{field}: {error}");
    }
    let mut value = serde_json::to_value(&original).unwrap();
    value.as_object_mut().unwrap().remove("prompt_digest");
    state::write_json(&dir.join("task.json"), &value).unwrap();
    let error = verify_task(&dir, None).unwrap_err().to_string();
    assert!(error.contains("missing field `prompt_digest`"), "{error}");
    task::save(&dir, &original, "substituted assignment").unwrap();
    assert!(
        verify_task(&dir, None)
            .unwrap_err()
            .to_string()
            .contains("does not match the digest recorded")
    );
}

#[test]
fn verify_refuses_redirected_worktree_and_changed_command() {
    let (_temp, repo, loaded, plan) = fixture();
    let original = prepared_record(&repo, &loaded, &plan, PROMPT, Default::default());
    let dir = repo.root.join(".ahu/state/fixture");
    std::fs::create_dir_all(&plan.worktree).unwrap();
    let mut record = original.clone();
    record.worktree = repo.root.join(".worktrees/other");
    std::fs::create_dir_all(&record.worktree).unwrap();
    task::save(&dir, &record, PROMPT).unwrap();
    let error = verify_task(&dir, None).unwrap_err().to_string();
    assert!(error.contains("not the one ahu would create"), "{error}");
    let mut record = original;
    record.launch_command.args.push("--unexpected".into());
    task::save(&dir, &record, PROMPT).unwrap();
    let error = verify_task(&dir, None).unwrap_err().to_string();
    assert!(
        error.contains("recorded launch command") && error.contains("changed identity"),
        "{error}"
    );
}

#[test]
fn planning_rejects_empty_assignment_and_unborn_repository_before_side_effects() {
    let (_temp, repo, _, frozen) = fixture_with_runtime(false);
    let error = plan(&repo, None, frozen.pair.clone(), " \n\t").unwrap_err();
    assert_eq!(error.kind(), crate::util::ErrorKind::Usage);
    assert!(error.to_string().contains("task prompt is empty"));
    let error = plan(&repo, None, frozen.pair, PROMPT).unwrap_err();
    assert_eq!(error.kind(), crate::util::ErrorKind::Prerequisite);
    assert!(error.to_string().contains("no commits yet"));
    assert!(!repo.root.join(".worktrees").exists());
    assert!(!repo.root.join(".ahu").exists());
}

#[test]
fn planning_refuses_missing_context_lock_before_creating_task_state() {
    let (_temp, repo, _, frozen) = fixture();
    git::run_ok(
        &repo.root,
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
    )
    .unwrap();
    let repo = git::discover(&repo.root).unwrap();
    let error = plan(&repo, None, frozen.pair, PROMPT).unwrap_err();
    assert_eq!(error.kind(), crate::util::ErrorKind::Prerequisite);
    assert!(
        error
            .to_string()
            .contains("agent context is not committed and locked")
    );
    assert!(error.to_string().contains(".agents/ahu/lock.md is missing"));
    assert!(!repo.root.join(".worktrees").exists());
    assert!(!frozen.task_dir.exists());
}

#[test]
fn missing_runtime_never_falls_back_to_a_builtin_provider() {
    let (_temp, repo, _loaded, plan) = fixture_with_runtime(false);
    assert!(
        harness::task_command(
            &repo,
            "codex",
            &LaunchRequest {
                model: "gpt-6",
                prompt: PROMPT,
                cwd: &plan.worktree,
                permissions: Default::default()
            },
            None
        )
        .is_err()
    );
    assert!(!plan.worktree.exists());
    assert!(!plan.task_dir.exists());
}

#[test]
fn verification_refuses_record_held_by_a_sibling_checkout() {
    let (_temp, repo, loaded, plan) = fixture();
    std::fs::create_dir_all(&plan.worktree).unwrap();
    let record = prepared_record(&repo, &loaded, &plan, PROMPT, Default::default());
    let sibling = repo.root.join(".worktrees/sibling");
    std::fs::create_dir_all(&sibling).unwrap();
    let misplaced = state::worktree_task_dir(&sibling, &repo.identity(), &plan.task_id);
    task::save(&misplaced, &record, PROMPT).unwrap();
    let error = verify_task(&misplaced, None).unwrap_err().to_string();
    assert!(
        error.contains("record held by a different task's checkout"),
        "{error}"
    );
    assert_eq!(task::load(&misplaced).unwrap().state, TaskState::Starting);
    assert_eq!(task::load_prompt(&misplaced).unwrap(), PROMPT);
    assert!(plan.worktree.is_dir());
}

#[test]
fn verification_resolves_workspace_path_without_executing_or_trusting_record_path() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    const CASE: &str = "AHU_LAUNCH_VERIFY_FIXTURE";
    let Ok(case) = std::env::var(CASE) else {
        let bin = tempfile::tempdir().unwrap();
        symlink(
            crate::selection::resolve_utility("git").unwrap(),
            bin.path().join("git"),
        )
        .unwrap();
        let executable = bin.path().join("codex");
        std::fs::write(&executable, "#!/bin/sh\nexit 99\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        for case in ["external", "missing", "repository", "symlink"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "launch::launch_contract_tests::verification_resolves_workspace_path_without_executing_or_trusting_record_path", "--nocapture"])
                    .env(CASE, case)
                    .env("PATH", bin.path())
                    .current_dir(bin.path())
                    .output().unwrap();
            assert!(
                output.status.success(),
                "{case}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    };
    let (_temp, repo, loaded, plan) = fixture();
    std::fs::create_dir_all(&plan.worktree).unwrap();
    let record = prepared_record(&repo, &loaded, &plan, PROMPT, Default::default());
    task::save(&plan.task_dir, &record, PROMPT).unwrap();
    // Each case runs in a separate test process, so PATH mutation cannot
    // race other tests or accidentally resolve a real installed harness.
    let bin = PathBuf::from(std::env::var_os("PATH").unwrap());
    if case == "missing" {
        std::fs::rename(bin.join("codex"), bin.join("codex-hidden")).unwrap();
        let error = verify_task(&plan.task_dir, None).unwrap_err();
        assert_eq!(error.kind(), crate::util::ErrorKind::Prerequisite);
        assert!(
            error.to_string().contains("not") || error.to_string().contains("unavailable"),
            "{error}"
        );
        std::fs::rename(bin.join("codex-hidden"), bin.join("codex")).unwrap();
    } else if case == "external" {
        let (verified, rebuilt, executable) = verify_task(&plan.task_dir, None).unwrap();
        assert_eq!(verified.task_id, record.task_id);
        assert_eq!(rebuilt, plan.command);
        assert_eq!(
            executable.canonicalize().unwrap(),
            bin.join("codex").canonicalize().unwrap()
        );
        assert_ne!(executable, record.harness_executable);
    } else {
        let local = repo.root.join("codex");
        std::fs::copy(bin.join("codex"), &local).unwrap();
        std::fs::rename(bin.join("codex"), bin.join("codex-hidden")).unwrap();
        // Binaries in the primary checkout and task directory must be refused.
        if case == "symlink" {
            symlink(&local, bin.join("codex")).unwrap();
        } else {
            std::fs::rename(&local, plan.worktree.join("codex")).unwrap();
            symlink(plan.worktree.join("codex"), bin.join("codex")).unwrap();
        }
        let error = verify_task(&plan.task_dir, None).unwrap_err().to_string();
        assert!(
            error.contains("not") || error.contains("unavailable"),
            "{error}"
        );
        std::fs::remove_file(bin.join("codex")).unwrap();
        std::fs::rename(bin.join("codex-hidden"), bin.join("codex")).unwrap();
    }
}

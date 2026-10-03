use super::*;

const PROMPT: &str = "Inspect café\n$(touch never-executed)";

// Construct the frozen boundary directly: no installed harness, user
// configuration, cmux session or additional Git worktree is needed.
fn fixture() -> (tempfile::TempDir, Repo, LoadedConfig, LaunchPlan) {
    let temp = tempfile::tempdir().unwrap();
    git::run_ok(temp.path(), &["init", "-q"]).unwrap();
    let repo = git::discover(temp.path()).unwrap();
    let config = crate::config::ProjectConfig {
        schema_version: 1,
        harness_preferences: vec!["codex".into()],
        model_selection: "project-ranked".into(),
        catalog_version: crate::catalog::CATALOG_VERSION.into(),
        harness_version_pins: Default::default(),
        model_rankings: [("codex".into(), vec!["gpt-6".into()])].into(),
        knowledge: Default::default(),
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
    let command = harness::adapter_for("codex")
        .unwrap()
        .launch_command(&LaunchRequest {
            model: "gpt-6",
            prompt: &delivered,
            cwd: &worktree,
            permissions: Default::default(),
        })
        .unwrap();
    let plan = LaunchPlan {
        mode: LaunchMode::Automatic,
        agent: None,
        pair: ResolvedPair {
            harness: "codex".into(),
            model: "gpt-6".into(),
            basis: "project ranking".into(),
            policy_digest: loaded.digest.clone(),
            catalog_version: loaded.config.catalog_version.clone(),
        },
        enforcement: EnforcementReport {
            harness: "codex".into(),
            harness_version: None,
            model_fixed_for_session: true,
            gaps: vec![DELIVERY_IS_NOT_ENFORCEMENT.into()],
            applied_controls: vec!["model flag".into()],
        },
        snapshot: Default::default(),
        hooks: Default::default(),
        cmux_integration: crate::cmux::integration::inspect_in(
            &repo.root,
            "codex",
            &Default::default(),
        ),
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
    let (_temp, repo, loaded, plan) = fixture();
    let record = prepared_record(&repo, &loaded, &plan, PROMPT, Default::default());
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
    assert!(record.cmux_workspace_id.is_none());
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
    let (_temp, repo, _, frozen) = fixture();
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
    assert!(error.to_string().contains("ahu.lock is missing"));
    assert!(!repo.root.join(".worktrees").exists());
    assert!(!repo.root.join(".ahu").exists());
}

fn commit_planning_context(repo: &Repo) {
    let snapshot = snapshot::collect(&repo.root).unwrap();
    crate::context_lock::refresh(repo, &snapshot).unwrap();
    git::run_ok(&repo.root, &["add", "."]).unwrap();
    git::run_ok(
        &repo.root,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "locked launch fixture",
        ],
    )
    .unwrap();
}

#[test]
fn planning_freezes_registered_identity_and_discloses_repository_inputs() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    const CASE: &str = "AHU_LAUNCH_PLAN_FIXTURE";
    let Ok(case) = std::env::var(CASE) else {
        for case in ["automatic", "named", "missing", "symlink", "dirty-context"] {
            let external = tempfile::tempdir().unwrap();
            let bin = external.path().join("cmux-cli-shims");
            std::fs::create_dir(&bin).unwrap();
            symlink(
                crate::selection::resolve_utility("git").unwrap(),
                bin.join("git"),
            )
            .unwrap();
            if case != "missing" {
                let executable = bin.join("opencode");
                std::fs::write(
                    &executable,
                    "#!/bin/sh\n[ \"$1\" = --version ] || exit 98\nprintf '1.18.31\\n'\n",
                )
                .unwrap();
                std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755))
                    .unwrap();
            }
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "launch::launch_contract_tests::planning_freezes_registered_identity_and_discloses_repository_inputs", "--nocapture"])
                    .env_clear()
                    .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|value| ("LLVM_PROFILE_FILE", value)))
                    .env(CASE, case)
                    .env("PATH", &bin)
                    .env("XDG_CONFIG_HOME", external.path().join("config"))
                    .env("XDG_DATA_HOME", external.path().join("data"))
                    .env("AHU_CMUX_BIN", external.path().join("absent-cmux"))
                    .current_dir(external.path())
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
    let (_temp, repo, mut loaded, mut frozen) = fixture();
    std::fs::create_dir_all(repo.root.join(".agents/ahu/agents")).unwrap();
    loaded.config.harness_preferences = vec!["opencode".into()];
    loaded.config.model_rankings =
        [("opencode".into(), vec!["ollama/glm-5.3:cloud".into()])].into();
    let config_text = crate::config::render(&loaded.config);
    loaded.digest = crate::util::digest_bytes(config_text.as_bytes());
    std::fs::write(&loaded.path, config_text).unwrap();
    std::fs::write(
            repo.root.join(".agents/ahu/agents/reviewer.md"),
            "---\nokf_version: 0.2\ntype: ahu:agent\ntitle: reviewer\ndescription: Review changes\nharness: opencode\nmodel: ollama/glm-5.3:cloud\npermissions: auto\nversion: 1.0.0\n---\nReview the source carefully.\n",
        ).unwrap();
    std::fs::write(
        repo.root.join("opencode.json"),
        r#"{
            "plugin": ["fixture-plugin@1.0.0"],
            "permission": {"bash": "allow"},
            "mcp": {"fixture": {"type": "local", "command": ["fixture-mcp"]}}
        }"#,
    )
    .unwrap();
    std::fs::write(repo.root.join("source.txt"), "committed source").unwrap();
    commit_planning_context(&repo);
    let repo = git::discover(&repo.root).unwrap();
    frozen.pair.policy_digest = loaded.digest.clone();
    frozen.pair.harness = "opencode".into();
    frozen.pair.model = "ollama/glm-5.3:cloud".into();
    let agent = (case == "named").then(|| crate::agent::find(&repo.root, "reviewer").unwrap());
    let redirected = tempfile::tempdir().unwrap();
    if case == "symlink" {
        symlink(redirected.path(), repo.root.join(".worktrees")).unwrap();
    }
    if case == "dirty-context" {
        std::fs::write(repo.root.join("opencode.json"), "{}").unwrap();
    }
    std::fs::write(repo.root.join("source.txt"), "uncommitted source").unwrap();
    let result = plan(&repo, agent.clone(), frozen.pair.clone(), PROMPT);
    if matches!(case.as_str(), "missing" | "symlink" | "dirty-context") {
        let error = result.unwrap_err();
        let message = error.to_string();
        match case.as_str() {
            "missing" => {
                assert_eq!(error.kind(), crate::util::ErrorKind::Prerequisite);
                assert!(
                    message.contains("opencode was not found on PATH"),
                    "{message}"
                );
                assert!(message.contains("not a reason to select a different harness"));
            }
            "symlink" => assert!(message.contains("symlink"), "{message}"),
            _ => {
                assert_eq!(error.kind(), crate::util::ErrorKind::Prerequisite);
                assert!(
                    message.contains("agent context is not committed and locked"),
                    "{message}"
                );
            }
        }
    } else {
        let planned = result.unwrap();
        assert_eq!(planned.base_commit, repo.head);
        assert!(planned.parent_dirty);
        assert_eq!(planned.pair.harness, "opencode");
        assert_eq!(planned.pair.model, "ollama/glm-5.3:cloud");
        assert_eq!(
            planned.agent_label(),
            if agent.is_some() {
                "reviewer@1.0.0"
            } else {
                "auto"
            }
        );
        assert!(planned.branch.starts_with(if agent.is_some() {
            "ahu/reviewer/"
        } else {
            "ahu/auto/"
        }));
        assert_eq!(
            planned.task_dir,
            state::worktree_task_dir(&planned.worktree, &repo.identity(), &planned.task_id)
        );
        let gaps = planned.enforcement.gaps.join("\n");
        for expected in [
            "fixture-plugin@1.0.0",
            "1 MCP server(s)",
            "cmux shim",
            "declares approval settings ahu does not set",
            DELIVERY_IS_NOT_ENFORCEMENT,
        ] {
            assert!(gaps.contains(expected), "missing {expected}: {gaps}");
        }
        let record = prepared_record(&repo, &loaded, &planned, PROMPT, Default::default());
        if let Some(agent) = agent {
            assert_eq!(record.identity.mode, LaunchMode::Named);
            assert_eq!(record.identity.permissions, crate::agent::Permissions::Auto);
            assert_eq!(
                record.identity.instructions_source.as_deref(),
                Some(".agents/ahu/agents/reviewer.md")
            );
            assert_eq!(
                record.identity.source_digest,
                Some(agent.source_digest.clone())
            );
            assert_eq!(
                record.identity.instructions_digest,
                Some(agent.instructions_digest.clone())
            );
            assert_eq!(
                record.identity.identity_digest,
                Some(agent.identity_digest())
            );
            assert!(record.identity.selection_basis.is_none());
        } else {
            assert_eq!(record.identity.mode, LaunchMode::Automatic);
            assert_eq!(
                record.identity.permissions,
                crate::agent::Permissions::Prompt
            );
        }
        let delivered = crate::orchestration::redeliver(&record.delivery, PROMPT).unwrap();
        assert!(delivered.contains(PROMPT));
        assert_eq!(
            delivered.contains("Review the source carefully."),
            case == "named"
        );
        assert_eq!(
            planned.command,
            harness::adapter_for("opencode")
                .unwrap()
                .launch_command(&LaunchRequest {
                    model: &planned.pair.model,
                    prompt: &delivered,
                    cwd: &planned.worktree,
                    permissions: planned.permissions,
                })
                .unwrap()
        );
        assert!(!planned.worktree.exists());
        assert!(!planned.task_dir.exists());
        let preview: serde_json::Value =
            serde_json::from_str(&render_json(&planned, PROMPT).unwrap()).unwrap();
        // Display metadata intentionally defaults to the assignment's opening
        // text; only the command's prompt argument is redacted.
        assert!(!preview["argv"].to_string().contains("never-executed"));
        assert_eq!(preview["summary"], crate::util::sidebar_text(PROMPT, 160));
        assert_eq!(preview["executed"], false);
    }
    assert!(!repo.root.join(".ahu").exists());
    assert!(
        git::run_ok(&repo.root, &["branch", "--list", "ahu/*"])
            .unwrap()
            .trim()
            .is_empty()
    );
    assert_eq!(
        std::fs::read_to_string(repo.root.join("source.txt")).unwrap(),
        "uncommitted source"
    );
    assert_eq!(std::fs::read_dir(redirected.path()).unwrap().count(), 0);
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
fn startup_failure_does_not_claim_post_spawn_termination() {
    for started in [false, true] {
        for cancelled in [false, true] {
            for state in [TaskState::Starting, TaskState::Running, TaskState::Exited] {
                let (_temp, repo, loaded, plan) = fixture();
                std::fs::create_dir_all(&plan.worktree).unwrap();
                let mut record = prepared_record(&repo, &loaded, &plan, PROMPT, Default::default());
                record.state = state;
                task::save(&plan.task_dir, &record, PROMPT).unwrap();
                if cancelled {
                    std::fs::write(plan.task_dir.join("cancel.json"), "{}").unwrap();
                }
                let error =
                    record_startup_error(&plan.task_dir, started, Error::new("synthetic failure"));
                assert_eq!(error.to_string(), "synthetic failure");
                let expected = if started || !state.is_live() {
                    state
                } else if cancelled {
                    TaskState::Cancelled
                } else {
                    TaskState::Failed
                };
                assert_eq!(task::load(&plan.task_dir).unwrap().state, expected);
            }
        }
    }
}

#[test]
fn run_task_preserves_work_on_cancellation_exit_and_spawn_failure() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    const CASE: &str = "AHU_LAUNCH_RUN_FIXTURE";
    let Ok(case) = std::env::var(CASE) else {
        for case in [
            "cancel",
            "success",
            "failure",
            "version-error",
            "auth-error",
            "auth-mismatch",
            "auth-unavailable",
        ] {
            let bin = tempfile::tempdir().unwrap();
            symlink(
                crate::selection::resolve_utility("git").unwrap(),
                bin.path().join("git"),
            )
            .unwrap();
            let script = format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then {} fi\nif [ \"$1\" = app-server ]; then printf '%s\\n' '{{\"id\":1,\"result\":{{}}}}' '{}'; while read -r line; do :; done; exit 0; fi\nprintf started > harness-started\nexit {}\n",
                if case == "version-error" {
                    "exit 1;"
                } else {
                    "echo 'codex-cli 0.160.0'; exit 0;"
                },
                if case == "auth-unavailable" {
                    r#"{"id":2,"result":{"account":{"type":"apiKey"}}}"#
                } else {
                    r#"{"id":2,"result":{"account":{"type":"chatgpt","email":"other@example.invalid"}}}"#
                },
                if case == "success" { 0 } else { 23 }
            );
            let executable = bin.path().join("codex");
            std::fs::write(&executable, script).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "launch::launch_contract_tests::run_task_preserves_work_on_cancellation_exit_and_spawn_failure", "--nocapture"])
                    .env(CASE, case)
                    .env("PATH", bin.path())
                    .env("AHU_CMUX_BIN", bin.path().join("absent-cmux"))
                    .current_dir(bin.path())
                    .stdin(std::process::Stdio::null())
                    .output()
                    .unwrap();
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
    let sentinel = plan.worktree.join("work.txt");
    std::fs::write(&sentinel, "keep me").unwrap();
    let record = prepared_record(&repo, &loaded, &plan, PROMPT, Default::default());
    task::save(&plan.task_dir, &record, PROMPT).unwrap();
    if case == "cancel" {
        std::fs::write(plan.task_dir.join("cancel.json"), "{}").unwrap();
    }
    if case == "auth-error" {
        let auth = repo
            .root
            .join(".ahu/state/repos")
            .join(repo.identity())
            .join("auth-bindings.json");
        state::create_private_dir_all(auth.parent().unwrap()).unwrap();
        state::write_private_file(&auth, br#"{"schema_version":999}"#).unwrap();
    }
    if matches!(case.as_str(), "auth-mismatch" | "auth-unavailable") {
        let auth = repo
            .root
            .join(".ahu/state/repos")
            .join(repo.identity())
            .join("auth-bindings.json");
        state::create_private_dir_all(auth.parent().unwrap()).unwrap();
        state::write_json(
            &auth,
            &serde_json::json!({
                "schema_version": 1, "repo_identity": repo.identity(),
                "bindings": {"codex": {"fingerprint": "0".repeat(64), "identity_kind": "chatgpt"}}
            }),
        )
        .unwrap();
    }
    let result = run_task(&plan.task_dir);
    let expected_state = match case.as_str() {
        "cancel" => {
            assert_eq!(result.unwrap(), HarnessOutcome::Cancelled);
            assert!(!plan.worktree.join("harness-started").exists());
            TaskState::Cancelled
        }
        "version-error" => {
            let error = result.unwrap_err().to_string();
            assert!(
                error.contains("cannot determine installed harness version"),
                "{error}"
            );
            assert!(!plan.worktree.join("harness-started").exists());
            TaskState::Failed
        }
        "auth-error" => {
            assert!(result.unwrap_err().to_string().contains("invalid JSON"));
            assert!(!plan.worktree.join("harness-started").exists());
            TaskState::Failed
        }
        "auth-mismatch" | "auth-unavailable" => {
            let error = result.unwrap_err().to_string();
            assert!(
                error.contains(if case == "auth-mismatch" {
                    "does not match"
                } else {
                    "unavailable"
                }),
                "{error}"
            );
            assert!(!plan.worktree.join("harness-started").exists());
            TaskState::Failed
        }
        "success" | "failure" => {
            let HarnessOutcome::Exited(status) = result.unwrap() else {
                panic!("expected an observed harness exit");
            };
            assert_eq!(status.code(), Some(if case == "success" { 0 } else { 23 }));
            assert_eq!(
                std::fs::read_to_string(plan.worktree.join("harness-started")).unwrap(),
                "started"
            );
            if case == "success" {
                TaskState::Exited
            } else {
                TaskState::Failed
            }
        }
        _ => panic!("unknown fixture case"),
    };
    assert_eq!(task::load(&plan.task_dir).unwrap().state, expected_state);
    assert_eq!(task::load_prompt(&plan.task_dir).unwrap(), PROMPT);
    assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "keep me");
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
        assert!(error.to_string().contains("will not fall back"));
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
            error.contains("not on PATH") && error.contains("will not fall back"),
            "{error}"
        );
        std::fs::remove_file(bin.join("codex")).unwrap();
        std::fs::rename(bin.join("codex-hidden"), bin.join("codex")).unwrap();
    }
}

#[test]
fn group_mapping_recovers_stale_groups_and_restores_promoted_anchors() {
    use std::os::unix::fs::PermissionsExt;
    const CASE: &str = "AHU_LAUNCH_GROUP_FIXTURE";
    let Ok(case) = std::env::var(CASE) else {
        for case in ["current", "stale", "create", "restore", "restore-failed"] {
            let (_temp, repo, _, _) = fixture();
            let rpc = tempfile::tempdir().unwrap();
            let group = cmux::Group {
                id: "group".into(),
                name: repo.display_name(),
                anchor_workspace_id: "promoted".into(),
                member_workspace_ids: vec!["promoted".into()],
                is_collapsed: false,
            };
            let responses = [
                (
                    "workspace.current",
                    serde_json::json!({"workspace_id": if case == "current" { "promoted" } else { "outside" }, "window_id": "window"}),
                ),
                (
                    "workspace.list",
                    serde_json::json!({"workspaces": [{"id": "promoted", "current_directory": repo.root}]}),
                ),
                (
                    "workspace.group.list",
                    serde_json::json!({"groups": if case == "create" { vec![] } else { vec![group.clone()] }}),
                ),
                (
                    "workspace.group.create",
                    serde_json::json!({"group": group}),
                ),
                (
                    "workspace.create",
                    serde_json::json!({"workspace_id": "restored"}),
                ),
                ("workspace.group.set_anchor", serde_json::json!({})),
            ];
            for (method, value) in responses {
                std::fs::write(rpc.path().join(format!("{method}.json")), value.to_string())
                    .unwrap();
            }
            let mut restored = group;
            restored.anchor_workspace_id = "restored".into();
            std::fs::write(
                rpc.path().join("restored.json"),
                serde_json::json!({"groups": [restored]}).to_string(),
            )
            .unwrap();
            let executable = rpc.path().join("cmux-fixture");
            std::fs::write(
                &executable,
                r#"#!/bin/sh
if [ "$1" = ping ]; then exit 0; fi
printf '%s\n' "$2" >> "$AHU_LAUNCH_RPC/calls"
if [ "$2" = workspace.create ] && [ "$AHU_LAUNCH_GROUP_FIXTURE" = restore-failed ]; then
  echo 'synthetic anchor failure' >&2
  exit 1
fi
if [ "$2" = workspace.group.set_anchor ]; then
  /bin/cp "$AHU_LAUNCH_RPC/restored.json" "$AHU_LAUNCH_RPC/workspace.group.list.json"
fi
/bin/cat "$AHU_LAUNCH_RPC/$2.json"
"#,
            )
            .unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "launch::launch_contract_tests::group_mapping_recovers_stale_groups_and_restores_promoted_anchors", "--nocapture"])
                    .env(CASE, case).env("AHU_LAUNCH_REPO", &repo.root)
                    .env("AHU_LAUNCH_RPC", rpc.path()).env("AHU_CMUX_BIN", executable)
                    .env_remove("CMUX_WORKSPACE_ID").env_remove("CMUX_SOCKET_PATH")
                    .current_dir(rpc.path())
                    .output().unwrap();
            assert!(
                output.status.success(),
                "{case}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let calls = std::fs::read_to_string(rpc.path().join("calls")).unwrap();
            assert_eq!(
                calls
                    .lines()
                    .filter(|line| *line == "workspace.group.create")
                    .count(),
                usize::from(case == "create")
            );
            assert_eq!(
                calls
                    .lines()
                    .filter(|line| *line == "workspace.group.set_anchor")
                    .count(),
                usize::from(case == "restore")
            );
        }
        return;
    };
    let repo = git::discover(Path::new(&std::env::var("AHU_LAUNCH_REPO").unwrap())).unwrap();
    let previous = GroupMapping {
        group_id: Some(
            if case.starts_with("restore") {
                "group"
            } else {
                "stale"
            }
            .into(),
        ),
        window_id: Some("window".into()),
        anchor_workspace_id: Some("original".into()),
    };
    state::write_json(&mapping_path(&repo).unwrap(), &previous).unwrap();
    let client = Cmux::discover().unwrap();
    let mut notes = Vec::new();
    let group = ensure_group(&client, &repo, &mut notes).unwrap();
    assert_eq!(group.id, "group");
    let saved: GroupMapping = state::read_json(&mapping_path(&repo).unwrap()).unwrap();
    assert_eq!(saved.group_id.as_deref(), Some("group"));
    assert_eq!(saved.window_id.as_deref(), Some("window"));
    match case.as_str() {
        "current" => {
            assert!(notes.is_empty());
            assert_eq!(saved.anchor_workspace_id.as_deref(), Some("promoted"));
        }
        "restore" => {
            assert_eq!(group.anchor_workspace_id, "restored");
            assert_eq!(saved.anchor_workspace_id.as_deref(), Some("restored"));
            assert!(
                notes
                    .iter()
                    .any(|note| note.contains("created a new anchor"))
            );
        }
        "restore-failed" => {
            assert_eq!(saved.anchor_workspace_id.as_deref(), Some("original"));
            assert!(
                notes
                    .iter()
                    .any(|note| note.contains("synthetic anchor failure")
                        && note.contains("could not restore"))
            );
        }
        _ => assert!(notes.iter().any(|note| note.contains("no longer exists"))),
    }
}

#[test]
fn rollback_reports_retained_checkout_and_preserves_original_error_kind() {
    let (_temp, repo, _, plan) = fixture();
    std::fs::create_dir_all(&plan.worktree).unwrap();
    state::create_private_dir_all(&plan.task_dir).unwrap();
    std::fs::write(plan.task_dir.join("prompt.txt"), PROMPT).unwrap();
    let work = plan.worktree.join("work.txt");
    std::fs::write(&work, "retain unfinished work").unwrap();
    // Git refuses this unregistered checkout. Rollback must report the
    // retained files, never force removal to make cleanup look successful.
    let cause =
        Error::new("synthetic launch failure").with_kind(crate::util::ErrorKind::Prerequisite);
    let error = rollback_worktree(&repo, &plan, cause);
    assert_eq!(error.kind(), crate::util::ErrorKind::Prerequisite);
    let message = error.to_string();
    for expected in [
        "synthetic launch failure",
        "could not be removed",
        "does not force-remove",
        plan.worktree.to_str().unwrap(),
        &plan.branch,
    ] {
        assert!(message.contains(expected), "{message}");
    }
    assert_eq!(
        std::fs::read_to_string(work).unwrap(),
        "retain unfinished work"
    );
    assert!(!plan.task_dir.exists(), "partial task record was retained");
}

#[test]
fn cleanup_removes_owned_state_but_refuses_symlinked_ancestors() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let owned = temp.path().join(".ahu/state/owned");
    state::create_private_dir_all(&owned).unwrap();
    std::fs::write(owned.join("prompt.txt"), PROMPT).unwrap();
    discard_task_dir(&owned);
    assert!(!owned.exists());
    let target = outside.path().join("retained");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("work.txt"), "keep me").unwrap();
    symlink(outside.path(), temp.path().join(".ahu/state/redirect")).unwrap();
    discard_task_dir(&temp.path().join(".ahu/state/redirect/retained"));
    assert_eq!(
        std::fs::read_to_string(target.join("work.txt")).unwrap(),
        "keep me"
    );
}

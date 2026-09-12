mod common;
use common::{TestRepo, git};
use std::process::{Command, Output};

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
        created_at: ahu::task::now_rfc3339(),
        repo_identity: discovered.identity(),
        repo_root: record_source.clone(),
        branch: "ahu/auto/gone0001".to_string(),
        worktree: record_source.clone(),
        base_commit: discovered.head.clone(),
        identity: ahu::task::LaunchIdentity {
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
        cmux_group_id: None,
        cmux_workspace_id: None,
        cmux_window_id: None,
        state: ahu::task::TaskState::Running,
    };
    let dir = repo
        .state_path()
        .join("repos")
        .join(discovered.identity())
        .join("tasks")
        .join(id);
    ahu::task::save(&dir, &record, "secret prompt").unwrap();
    dir
}

fn run(repo: &TestRepo, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ahu"))
        .args(args)
        .current_dir(repo.path())
        .env("AHU_STATE_DIR", repo.state_path())
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
    let diff = run(&repo, &["diff", "abc1"]);
    assert_eq!(diff.status.code(), Some(5));
}

#[test]
fn lookup_rejects_ambiguity_including_unreadable_records_and_accepts_exact_ids() {
    let repo = TestRepo::new();
    record(&repo, "abc1");
    let bad = record(&repo, "abc2");
    std::fs::write(bad.join("task.json"), "broken").unwrap();
    for command in ["task", "diff"] {
        let out = run(&repo, &[command, "abc"]);
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("ambiguous"));
        assert_eq!(run(&repo, &[command, "abc2"]).status.code(), Some(5));
        assert_eq!(run(&repo, &[command, "missing"]).status.code(), Some(2));
        assert_eq!(run(&repo, &[command]).status.code(), Some(2));
        assert_eq!(
            run(&repo, &[command, "abc1", "--output", "yaml"])
                .status
                .code(),
            Some(2)
        );
    }
    record(&repo, "abc12");
    assert!(run(&repo, &["task", "abc1"]).status.success());
}

#[test]
fn diff_covers_base_to_working_tree_without_executing_helpers_or_staging() {
    let repo = TestRepo::new();
    record(&repo, "abc1");
    repo.write("committed.txt", "committed change\n");
    repo.commit("task change");
    repo.write("staged.txt", "staged change\n");
    git(repo.path(), &["add", "staged.txt"]);
    repo.write("README.md", "unstaged change\n");
    repo.write("untracked.txt", "untracked change\n");
    // An external helper would fail the test if invoked.
    git(
        repo.path(),
        &["config", "diff.external", "/nonexistent-diff-helper"],
    );
    git(
        repo.path(),
        &["config", "diff.fixture.textconv", "/nonexistent-textconv"],
    );
    repo.write(".gitattributes", "*.txt diff=fixture\n");
    let before = git(repo.path(), &["status", "--porcelain"]);
    let output = run(&repo, &["diff", "abc1"]);
    assert!(output.status.success(), "{:?}", output);
    let patch = String::from_utf8(output.stdout).unwrap();
    for expected in ["+committed change", "+staged change", "+unstaged change"] {
        assert!(patch.contains(expected), "{patch}");
    }
    assert!(!patch.contains("untracked change"));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Untracked (not included in diff): untracked.txt")
    );
    assert_eq!(git(repo.path(), &["status", "--porcelain"]), before);
}

#[test]
fn diff_refuses_foreign_checkouts_and_option_like_bases() {
    let repo = TestRepo::new();
    let foreign = TestRepo::new();
    let path = record(&repo, "abc1").join("task.json");
    let original: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for (key, value) in [
        ("worktree", foreign.path().to_str().unwrap()),
        ("base_commit", "--output=unexpected"),
    ] {
        let mut record = original.clone();
        record[key] = value.into();
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert_eq!(run(&repo, &["diff", "abc1"]).status.code(), Some(5));
    }
}

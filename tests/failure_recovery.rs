//! Failure recovery and the interactive flow's safety properties.
//!
//! A launch that cannot finish must leave the repository as it found it, and a
//! launch that has started must never be silently torn down.

mod common;

use std::io::Cursor;

use ahu::launcher::Console;
use ahu::{commands, config, git, launch, selection, state, task};
use common::TestRepo;

/// Drive a command with scripted answers and capture what it printed.
fn scripted(
    input: &str,
    f: impl FnOnce(&mut Console<'_>) -> ahu::util::Result<i32>,
) -> (i32, String) {
    let mut reader = Cursor::new(input.as_bytes().to_vec());
    let mut written: Vec<u8> = Vec::new();
    let code = {
        let mut console = Console {
            input: &mut reader,
            output: &mut written,
            interactive: true,
        };
        f(&mut console)
    };
    let text = String::from_utf8_lossy(&written).to_string();
    match code {
        Ok(code) => (code, text),
        Err(e) => (-1, format!("{text}\nERROR: {e}")),
    }
}

/// Serialises the tests that point ahu's state directory at their own temporary
/// directory. `AHU_STATE_DIR` is process-wide, so only one test may own it at a
/// time; the guard is held for the whole closure.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn with_state<T>(repo: &TestRepo, f: impl FnOnce() -> T) -> T {
    let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: every reader and writer of these variables in this binary goes
    // through `with_state`, so the guard makes the mutation exclusive.
    unsafe {
        std::env::set_var("AHU_STATE_DIR", repo.state_path());
        std::env::set_var("AHU_CMUX_BIN", repo.state_path().join("no-such-cmux"));
    }
    let result = f();
    drop(guard);
    result
}

#[test]
fn a_launch_that_cannot_reach_cmux_leaves_no_worktree_branch_or_record() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");

    let discovered = git::discover(repo.path()).unwrap();
    let loaded = config::load(repo.path()).unwrap().unwrap();
    let agent = ahu::agent::find(repo.path(), "chris").unwrap();
    let pair = selection::ResolvedPair {
        harness: agent.manifest.harness.clone(),
        model: agent.manifest.model.clone(),
        basis: "named agent".to_string(),
        policy_digest: loaded.digest.clone(),
        catalog_version: loaded.config.catalog_version.clone(),
    };
    // `with_state` keeps both the plan's paths and cmux discovery inside this
    // test's temporary directory, so it can never touch real state or a real
    // cmux session.
    let (plan, error) = with_state(&repo, || {
        let plan = launch::plan(&discovered, Some(agent), pair, "do the thing").unwrap();
        let error = launch::execute(&discovered, &loaded, &plan, "do the thing", false)
            .unwrap_err()
            .to_string();
        (plan, error)
    });
    assert!(
        plan.worktree
            .starts_with(discovered.root.join(".worktrees"))
    );
    assert!(error.contains("cmux"), "{error}");

    assert!(
        !plan.worktree.exists(),
        "a failed launch left a worktree behind"
    );
    assert!(
        !plan.task_dir.exists(),
        "a failed launch left a task record behind"
    );
    assert!(
        !git::branch_exists(&discovered, &plan.branch).unwrap(),
        "a failed launch left branch {} behind",
        plan.branch
    );
    // The invoking checkout is untouched.
    assert_eq!(
        common::git(repo.path(), &["rev-parse", "--abbrev-ref", "HEAD"]),
        "main"
    );
}

#[test]
fn an_empty_prompt_is_refused_before_anything_is_created() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    let discovered = git::discover(repo.path()).unwrap();
    let loaded = config::load(repo.path()).unwrap().unwrap();
    let pair = selection::resolve_automatic(&loaded).unwrap();
    let error = launch::plan(&discovered, None, pair, "   \n  \n")
        .unwrap_err()
        .to_string();
    assert!(error.contains("empty"), "{error}");
}

#[test]
fn a_repository_without_commits_is_refused_with_an_actionable_message() {
    let dir = tempfile::TempDir::new().unwrap();
    common::git(dir.path(), &["init", "-q", "-b", "main"]);
    let discovered = git::discover(dir.path()).unwrap();
    let loaded_dir = TestRepo::new();
    loaded_dir.init_config();
    let loaded = config::load(loaded_dir.path()).unwrap().unwrap();
    let pair = selection::resolve_automatic(&loaded).unwrap();
    let error = launch::plan(&discovered, None, pair, "prompt")
        .unwrap_err()
        .to_string();
    assert!(error.contains("no commits yet"), "{error}");
}

#[test]
fn outside_a_repository_ahu_explains_rather_than_creating_anything() {
    let dir = tempfile::TempDir::new().unwrap();
    let error = git::discover(dir.path()).unwrap_err().to_string();
    assert!(error.contains("not inside a Git repository"), "{error}");
}

#[test]
fn the_launch_lock_serialises_concurrent_launches_for_one_repository() {
    let repo = TestRepo::new();
    with_state(&repo, || {
        let first = state::LaunchLock::acquire("repo-under-test").unwrap();
        // A second attempt inside the same window must not proceed.
        let start = std::time::Instant::now();
        let second = state::LaunchLock::acquire("repo-under-test");
        assert!(second.is_err(), "two launches acquired the same lock");
        assert!(start.elapsed() >= std::time::Duration::from_secs(1));
        drop(first);
        // Once released, the next launch proceeds.
        assert!(state::LaunchLock::acquire("repo-under-test").is_ok());
    });
}

#[test]
fn task_ids_are_unique_across_rapid_repeated_launches() {
    let ids: std::collections::BTreeSet<String> = (0..500).map(|_| task::new_task_id()).collect();
    assert_eq!(ids.len(), 500, "task ids collided");
}

#[test]
fn run_task_preserves_the_record_when_its_worktree_is_gone() {
    let temp = tempfile::TempDir::new().unwrap();
    let task_dir = temp.path().join("task");
    let missing = temp.path().join("gone");
    // Build a record pointing at a worktree that does not exist.
    std::fs::create_dir_all(&missing).unwrap();
    let record_source = missing.clone();
    std::fs::create_dir_all(&task_dir).unwrap();
    let adapter = ahu::harness::adapter_for("claude-code").unwrap();
    let command = adapter
        .launch_command(&ahu::harness::LaunchRequest {
            model: "claude-opus-5",
            native_agent: None,
            prompt: "p",
            cwd: &record_source,
        })
        .unwrap();
    let enforcement = adapter.enforcement("claude-opus-5");
    let record = ahu::task::TaskRecord {
        schema_version: ahu::task::TASK_SCHEMA_VERSION,
        task_id: "gone0001".to_string(),
        title: "t".to_string(),
        created_at: ahu::task::now_rfc3339(),
        repo_identity: "r".to_string(),
        repo_root: record_source.clone(),
        branch: "ahu/auto/gone0001".to_string(),
        worktree: record_source.clone(),
        base_commit: Some("0".repeat(40)),
        identity: ahu::task::LaunchIdentity {
            mode: ahu::task::LaunchMode::Automatic,
            agent: "auto".to_string(),
            agent_version: None,
            harness: "claude-code".to_string(),
            model: "claude-opus-5".to_string(),
            instructions_source: None,
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
        prompt_digest: String::new(),
        harness_executable: std::path::PathBuf::from("claude"),
        materialize: Default::default(),
        launch_command: command,
        reliability_warning: None,
        enforcement,
        cmux_group_id: None,
        cmux_workspace_id: None,
        cmux_window_id: None,
        state: ahu::task::TaskState::Starting,
    };
    ahu::task::save(&task_dir, &record, "p").unwrap();
    std::fs::remove_dir_all(&missing).unwrap();

    let error = launch::run_task(&task_dir).unwrap_err().to_string();
    assert!(error.contains("missing"), "{error}");
    assert!(
        task_dir.join("task.json").exists(),
        "the task record must be preserved so the user can recover"
    );
}

// --- interactive flow ---

#[test]
fn pasting_a_multiline_prompt_does_not_submit_it() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    let discovered = git::discover(repo.path()).unwrap();

    // Selector, then a multiline paste, then the sentinel, then decline submit.
    let script = "@chris\nline one\nline two\n\nline four\n.\nn\n";
    let (code, text) = with_state(&repo, || {
        scripted(script, |console| {
            commands::interactive(console, &discovered, false)
        })
    });

    assert_eq!(code, 1, "declining must not launch: {text}");
    assert!(text.contains("About to submit"), "{text}");
    assert!(text.contains("chris@1.0.0"), "{text}");
    assert!(text.contains("claude-opus-5"), "{text}");
    assert!(text.contains("4 line(s)"), "{text}");
    assert!(
        text.contains("No worktree, branch, or session was created"),
        "{text}"
    );

    // Nothing at all was created.
    let identity = discovered.identity();
    assert!(task::list(&identity).unwrap().is_empty());
    let branches = common::git(repo.path(), &["branch", "--list", "ahu/*"]);
    assert!(branches.is_empty(), "{branches}");
}

#[test]
fn the_resolved_harness_and_model_are_shown_before_the_prompt_is_entered() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.commit("fixture");
    let discovered = git::discover(repo.path()).unwrap();

    // No selector: automatic selection.
    let (_, text) = with_state(&repo, || {
        scripted("\n.cancel\n", |console| {
            commands::interactive(console, &discovered, false)
        })
    });
    let resolved_at = text.find("Resolved for this task").expect("preview shown");
    let composer_at = text.find("Task prompt.").expect("composer shown");
    assert!(
        resolved_at < composer_at,
        "the provider must be shown before the prompt is typed"
    );
    assert!(text.contains("auto (no named agent)"), "{text}");
    assert!(text.contains("claude-opus-5"), "{text}");
}

#[test]
fn a_first_load_hygiene_review_runs_before_submission_and_deletes_nothing() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.write(".claude/skills/review/SKILL.md", "a skill that may load\n");
    repo.commit("fixture");
    let discovered = git::discover(repo.path()).unwrap();

    let (_, text) = with_state(&repo, || {
        scripted("@chris\ndo the thing\n.\nn\n", |console| {
            commands::interactive(console, &discovered, false)
        })
    });
    assert!(text.contains("Context hygiene review"), "{text}");
    assert!(text.contains("first load"), "{text}");
    assert!(text.contains(".claude/skills/review/SKILL.md"), "{text}");
    assert!(text.contains("Nothing has been changed"), "{text}");
    // The skill is still there.
    assert!(repo.path().join(".claude/skills/review/SKILL.md").exists());
}

#[test]
fn a_noninteractive_first_run_asks_for_setup_and_writes_nothing() {
    let repo = TestRepo::new();
    let discovered = git::discover(repo.path()).unwrap();
    let mut reader = Cursor::new(Vec::new());
    let mut written: Vec<u8> = Vec::new();
    let result = {
        let mut console = Console {
            input: &mut reader,
            output: &mut written,
            interactive: false,
        };
        with_state(&repo, || {
            commands::interactive(&mut console, &discovered, false)
        })
    };
    let error = result.unwrap_err().to_string();
    assert!(error.contains("not run interactively"), "{error}");
    assert!(error.contains("Nothing was changed"), "{error}");
    assert!(!repo.path().join(".agents/ahu/config.toml").exists());
}

#[test]
fn cancelled_setup_writes_nothing() {
    let repo = TestRepo::new();
    let discovered = git::discover(repo.path()).unwrap();
    // Choose Claude Code, accept catalog model order, default interval, then say no.
    let (code, text) = with_state(&repo, || {
        scripted("1\n\n\nn\n", |console| commands::init(console, &discovered))
    });
    assert_eq!(code, 1, "{text}");
    assert!(text.contains("Nothing was written"), "{text}");
    assert!(!repo.path().join(".agents/ahu/config.toml").exists());
}

#[test]
fn setup_saves_only_the_config_file() {
    let repo = TestRepo::new();
    let discovered = git::discover(repo.path()).unwrap();
    let (code, text) = with_state(&repo, || {
        scripted("1\n\n\ny\n", |console| commands::init(console, &discovered))
    });
    assert_eq!(code, 0, "{text}");
    let written = repo.read(".agents/ahu/config.toml");
    assert!(
        written.contains("harness_preferences = [\"claude-code\"]"),
        "{written}"
    );
    assert!(written.contains("claude-opus-5"), "{written}");

    // Only the config file was added, and nothing was staged.
    let status = common::git(repo.path(), &["status", "--porcelain"]);
    assert!(status.contains(".agents/"), "{status}");
    let staged = common::git(repo.path(), &["diff", "--cached", "--name-only"]);
    assert!(staged.is_empty(), "{staged}");

    // It is usable immediately, uncommitted.
    let loaded = config::load(repo.path()).unwrap().unwrap();
    assert_eq!(loaded.config.harness_preferences, vec!["claude-code"]);

    // Re-running reports the existing configuration rather than replacing it.
    let (code, text) = with_state(&repo, || {
        scripted("", |console| commands::init(console, &discovered))
    });
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("already initialized"), "{text}");
    assert_eq!(repo.read(".agents/ahu/config.toml"), written);
}

#[test]
fn onboarding_is_additive_idempotent_and_reversible() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.write(
        ".claude/agents/sam.md",
        "---\nname: sam\ndescription: a fixture agent\nmodel: claude-sonnet-5\ntools: Read\n---\n\nYou are sam.\n",
    );
    let native_before = repo.read(".claude/agents/sam.md");
    repo.commit("native agent");
    let discovered = git::discover(repo.path()).unwrap();

    // Preview writes nothing.
    let (code, text) = with_state(&repo, || {
        scripted("", |console| {
            commands::onboard_cmd(console, &discovered, None, None, None, "0.1.0")
        })
    });
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("Nothing has been written"), "{text}");
    assert!(text.contains("can be registered"), "{text}");
    assert!(!repo.path().join(".agents/ahu/agents/sam.toml").exists());

    // Registering adds exactly one file and leaves the native definition alone.
    let (code, text) = with_state(&repo, || {
        scripted("y\n", |console| {
            commands::onboard_cmd(console, &discovered, Some("sam"), None, None, "0.1.0")
        })
    });
    assert_eq!(code, 0, "{text}");
    assert_eq!(repo.read(".claude/agents/sam.md"), native_before);
    let resolved = ahu::agent::find(repo.path(), "sam").unwrap();
    assert_eq!(resolved.manifest.model, "claude-sonnet-5");
    assert_eq!(
        resolved.manifest.description, "a fixture agent",
        "the native description is carried into the manifest"
    );

    // Repeating produces no change.
    let (code, text) = with_state(&repo, || {
        scripted("", |console| {
            commands::onboard_cmd(console, &discovered, Some("sam"), None, None, "0.1.0")
        })
    });
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("already registered"), "{text}");

    // Removing takes only the manifest.
    let (code, _) = with_state(&repo, || {
        scripted("", |console| {
            commands::onboard_cmd(console, &discovered, None, Some("sam"), None, "0.1.0")
        })
    });
    assert_eq!(code, 0);
    assert!(!repo.path().join(".agents/ahu/agents/sam.toml").exists());
    assert_eq!(repo.read(".claude/agents/sam.md"), native_before);
}

#[test]
fn the_inventory_separates_available_from_loaded_and_admits_its_gaps() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.write("CLAUDE.md", "repository guidance\n");
    repo.write(".claude/skills/review/SKILL.md", "skill\n");
    repo.commit("fixture");
    let discovered = git::discover(repo.path()).unwrap();

    let (code, text) = with_state(&repo, || {
        scripted("", |console| {
            commands::inventory_cmd(console, &discovered, Some("chris"))
        })
    });
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("[loaded] chris@1.0.0"), "{text}");
    assert!(text.contains("[available] CLAUDE.md"), "{text}");
    assert!(
        text.contains("[available] .claude/skills/review/SKILL.md"),
        "{text}"
    );
    assert!(text.contains("[opaque]"), "{text}");
    assert!(text.contains("What ahu cannot see"), "{text}");
    assert!(text.contains("is not complete"), "{text}");
}

#[test]
fn drift_is_reported_when_a_version_label_covers_changed_inputs() {
    use ahu::drift;
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    let discovered = git::discover(repo.path()).unwrap();
    let loaded = config::load(repo.path()).unwrap().unwrap();
    let agent = ahu::agent::find(repo.path(), "chris").unwrap();
    let plan = launch::plan(
        &discovered,
        Some(agent.clone()),
        selection::ResolvedPair {
            harness: "claude-code".to_string(),
            model: "claude-opus-5".to_string(),
            basis: "named".to_string(),
            policy_digest: loaded.digest.clone(),
            catalog_version: loaded.config.catalog_version.clone(),
        },
        "prompt",
    )
    .unwrap();

    // Pretend a previous task of the same label ran with different inputs.
    let mut previous = ahu::task::TaskRecord {
        schema_version: ahu::task::TASK_SCHEMA_VERSION,
        task_id: "prev0001".to_string(),
        title: "earlier".to_string(),
        created_at: "2026-09-01T00:00:00Z".to_string(),
        repo_identity: discovered.identity(),
        repo_root: discovered.root.clone(),
        branch: "ahu/chris/prev0001".to_string(),
        worktree: discovered.root.clone(),
        base_commit: discovered.head.clone(),
        identity: ahu::task::LaunchIdentity {
            mode: ahu::task::LaunchMode::Named,
            agent: "chris".to_string(),
            agent_version: Some("1.0.0".to_string()),
            harness: "claude-code".to_string(),
            model: "claude-opus-5".to_string(),
            instructions_source: Some(".claude/agents/chris.md".to_string()),
            instructions_digest: Some("1".repeat(64)),
            identity_digest: Some("1".repeat(64)),
            selection_basis: None,
        },
        policy_digest: loaded.digest.clone(),
        catalog_version: loaded.config.catalog_version.clone(),
        config_snapshot: Default::default(),
        config_snapshot_digest: "2".repeat(64),
        hooks: Default::default(),
        hooks_digest: String::new(),
        prompt_digest: String::new(),
        harness_executable: std::path::PathBuf::from("claude"),
        materialize: Default::default(),
        launch_command: plan.command.clone(),
        reliability_warning: None,
        enforcement: plan.enforcement.clone(),
        cmux_group_id: None,
        cmux_workspace_id: None,
        cmux_window_id: None,
        state: ahu::task::TaskState::Exited,
    };
    previous.state = ahu::task::TaskState::Exited;

    let found = drift::detect(
        "chris@1.0.0",
        Some(&agent.identity_digest()),
        &plan.snapshot.digest(),
        &loaded.digest,
        &plan.hooks.digest(),
        &[(std::path::PathBuf::from("/tmp/prev"), previous)],
    )
    .expect("drift detected");
    let rendered = drift::render(&found);
    assert!(
        rendered.contains("instructions or manifest changed"),
        "{rendered}"
    );
    assert!(rendered.contains("bump the agent's version"), "{rendered}");
    assert!(rendered.contains("harmless patch"), "{rendered}");
}

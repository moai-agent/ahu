//! Safe prompt transport.
//!
//! A pasted prompt may contain anything a shell would interpret. It must reach
//! the harness byte for byte, and it must never be evaluated on the way there.
//! ahu achieves this by keeping the prompt out of every shell string: cmux's
//! startup command carries only ahu's own quoted paths, and the harness is
//! started with an argument vector.

mod common;

use std::path::{Path, PathBuf};

use ahu::cmux;
use ahu::harness::{self, LaunchRequest};
use ahu::util::shell_single_quote;
use common::{HOSTILE_PROMPT, TestRepo, fake_harness};

#[test]
fn the_prompt_is_a_single_argument_reproduced_byte_for_byte() {
    let adapter = harness::adapter_for("claude-code").unwrap();
    let command = adapter
        .launch_command(&LaunchRequest {
            model: "claude-opus-5",
            native_agent: Some("chris"),
            prompt: HOSTILE_PROMPT,
            cwd: Path::new("/tmp"),
        })
        .unwrap();
    assert_eq!(command.program, "claude");
    assert_eq!(
        command.args,
        vec![
            "--model".to_string(),
            "claude-opus-5".to_string(),
            "--agent".to_string(),
            "chris".to_string(),
            "--".to_string(),
            HOSTILE_PROMPT.to_string(),
        ]
    );
    // Exactly one argument holds the prompt, and it is unmodified.
    assert_eq!(
        command
            .args
            .iter()
            .filter(|a| a.contains("$(touch"))
            .count(),
        1
    );
}

#[test]
fn a_prompt_that_looks_like_an_option_is_still_a_prompt() {
    let adapter = harness::adapter_for("claude-code").unwrap();
    let command = adapter
        .launch_command(&LaunchRequest {
            model: "claude-opus-5",
            native_agent: None,
            prompt: "--dangerously-skip-permissions",
            cwd: Path::new("/tmp"),
        })
        .unwrap();
    let separator = command.args.iter().position(|a| a == "--").unwrap();
    assert_eq!(
        command.args[separator + 1],
        "--dangerously-skip-permissions"
    );
    // The adapter never widens permissions on its own.
    assert!(
        !command.args[..separator]
            .iter()
            .any(|a| a.contains("permission"))
    );
    assert!(
        !command.args[..separator]
            .iter()
            .any(|a| a.contains("dangerously"))
    );
}

#[test]
fn shell_quoting_survives_every_metacharacter() {
    for value in [
        "plain",
        "with space",
        "with'quote",
        "$(touch pwned)",
        "`backtick`",
        "a\"b\\c",
        "semi;colon && pipe |",
        "new\nline",
    ] {
        let quoted = shell_single_quote(value);
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("printf '%s' {quoted}"))
            .output()
            .expect("sh runs");
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            value,
            "for {value:?}"
        );
    }
}

#[test]
fn the_cmux_startup_command_carries_no_prompt_and_survives_a_hostile_path() {
    let temp = tempfile::TempDir::new().unwrap();
    // A task directory whose name would break naive quoting.
    let nasty = temp.path().join("task dir with 'quote' and $VAR");
    std::fs::create_dir_all(&nasty).unwrap();
    let recorder = temp.path().join("argv.txt");
    let exe = write_argv_recorder(temp.path(), &recorder);

    let command = cmux::startup_command(&exe, &nasty);
    assert!(!command.contains("$(touch"));
    assert!(!command.contains(HOSTILE_PROMPT));

    // cmux types this into an interactive shell, so run it through one.
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .status()
        .expect("sh runs");
    assert!(status.success(), "startup command failed: {command}");

    let recorded: Vec<String> = std::fs::read_to_string(&recorder)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        recorded,
        vec![
            "run-task".to_string(),
            "--task-dir".to_string(),
            nasty.to_string_lossy().to_string(),
        ]
    );
}

/// The whole path: a prepared task record, started the way cmux starts it,
/// reaching a stand-in harness with the prompt intact and nothing executed.
#[test]
fn run_task_delivers_a_hostile_prompt_literally_and_executes_nothing() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");

    let temp = tempfile::TempDir::new().unwrap();
    let worktree = temp.path().join("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    let recorder = temp.path().join("argv.txt");
    let bin = fake_harness(temp.path(), &recorder);
    let canary = temp.path().join("ahu-pwned");
    let prompt = format!(
        "{HOSTILE_PROMPT}\nalso $(touch {canary}) and `touch {canary}`",
        canary = canary.display()
    );

    let task_dir = temp.path().join("task");
    write_task_record(&task_dir, &worktree, &prompt);

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
        .args(["run-task", "--task-dir", &task_dir.to_string_lossy()])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("AHU_STATE_DIR", repo.state_path())
        // Point cmux discovery at nothing so the test never touches a real session.
        .env("AHU_CMUX_BIN", temp.path().join("no-such-cmux"))
        .output()
        .expect("ahu runs");
    assert!(
        output.status.success(),
        "ahu run-task failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let recorded = std::fs::read_to_string(&recorder).expect("the harness ran");
    let args: Vec<&str> = recorded.lines().collect();
    assert_eq!(args[0], "--model");
    assert_eq!(args[1], "claude-opus-5");
    assert_eq!(args[2], "--agent");
    assert_eq!(args[3], "chris");
    assert_eq!(args[4], "--");
    // The fake harness records one argument per line, so compare line by line.
    assert_eq!(args[5..].join("\n"), prompt);
    assert!(
        !canary.exists(),
        "command substitution inside the prompt was executed"
    );

    // The reliability warning is shown to the person using the session.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(harness::RELIABILITY_WARNING), "{stderr}");
}

#[test]
fn run_task_refuses_to_start_a_session_under_an_edited_identity() {
    let repo = TestRepo::new();
    let temp = tempfile::TempDir::new().unwrap();
    let worktree = temp.path().join("worktree");
    std::fs::create_dir_all(&worktree).unwrap();
    let recorder = temp.path().join("argv.txt");
    let bin = fake_harness(temp.path(), &recorder);
    let task_dir = temp.path().join("task");
    write_task_record(&task_dir, &worktree, "do the thing");

    // Tamper with the frozen command, leaving the identity fields alone.
    let raw = std::fs::read_to_string(task_dir.join("task.json")).unwrap();
    std::fs::write(
        task_dir.join("task.json"),
        raw.replace(
            "\"claude-opus-5\",\n      \"--agent\"",
            "\"claude-haiku-4-5\",\n      \"--agent\"",
        ),
    )
    .unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
        .args(["run-task", "--task-dir", &task_dir.to_string_lossy()])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("AHU_STATE_DIR", repo.state_path())
        .env("AHU_CMUX_BIN", temp.path().join("no-such-cmux"))
        .output()
        .expect("ahu runs");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("changed identity"), "{stderr}");
    assert!(!recorder.exists(), "the harness must not have been started");
}

fn write_argv_recorder(dir: &Path, record: &Path) -> PathBuf {
    let script = dir.join("fake-ahu");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             : > '{record}'\n\
             for arg in \"$@\"; do printf '%s\\n' \"$arg\" >> '{record}'; done\n",
            record = record.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    script
}

/// Build the same record `launch::execute` writes, without needing cmux.
fn write_task_record(task_dir: &Path, worktree: &Path, prompt: &str) {
    use ahu::harness::LaunchRequest;
    use ahu::task::{LaunchIdentity, LaunchMode, TaskRecord, TaskState};

    let adapter = ahu::harness::adapter_for("claude-code").unwrap();
    let command = adapter
        .launch_command(&LaunchRequest {
            model: "claude-opus-5",
            native_agent: Some("chris"),
            prompt,
            cwd: worktree,
        })
        .unwrap();
    let enforcement = adapter.enforcement("claude-opus-5");
    let record = TaskRecord {
        schema_version: ahu::task::TASK_SCHEMA_VERSION,
        task_id: "testtask0001".to_string(),
        title: "fixture".to_string(),
        created_at: ahu::task::now_rfc3339(),
        repo_identity: "testrepo".to_string(),
        repo_root: worktree.to_path_buf(),
        branch: "ahu/chris/testtask0001".to_string(),
        worktree: worktree.to_path_buf(),
        base_commit: Some("0".repeat(40)),
        identity: LaunchIdentity {
            mode: LaunchMode::Named,
            agent: "chris".to_string(),
            agent_version: Some("1.0.0".to_string()),
            harness: "claude-code".to_string(),
            model: "claude-opus-5".to_string(),
            instructions_source: Some(".claude/agents/chris.md".to_string()),
            instructions_digest: Some("0".repeat(64)),
            identity_digest: Some("0".repeat(64)),
            selection_basis: None,
        },
        policy_digest: "0".repeat(64),
        catalog_version: ahu::catalog::CATALOG_VERSION.to_string(),
        config_snapshot: Default::default(),
        config_snapshot_digest: "0".repeat(64),
        hooks: Default::default(),
        hooks_digest: String::new(),
        materialize: Default::default(),
        launch_command: command,
        reliability_warning: enforcement
            .needs_reliability_warning()
            .then(|| harness::RELIABILITY_WARNING.to_string()),
        enforcement,
        cmux_group_id: None,
        cmux_workspace_id: None,
        cmux_window_id: None,
        state: TaskState::Starting,
    };
    ahu::task::save(task_dir, &record, prompt).unwrap();
}

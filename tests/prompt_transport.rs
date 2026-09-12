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
    // `run_task` re-derives the working directory from the repository identity
    // and task id, so the fixture must use the path ahu would actually create.
    // `run_task` re-derives the worktree from the record's repo_root and task id.
    let worktree = std::fs::canonicalize(repo.path())
        .unwrap()
        .join(".worktrees/testtask0001");
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
    // `run_task` re-derives the worktree from the record's repo_root and task id.
    let worktree = std::fs::canonicalize(repo.path())
        .unwrap()
        .join(".worktrees/testtask0001");
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
        repo_root: worktree
            .parent()
            .and_then(|p| p.parent())
            .unwrap()
            .to_path_buf(),
        branch: "ahu/chris/testtask0001".to_string(),
        worktree: worktree.to_path_buf(),
        base_commit: Some("0".repeat(40)),
        identity: LaunchIdentity {
            mode: LaunchMode::Named,
            agent: "chris".to_string(),
            agent_version: Some("1.0.0".to_string()),
            native_agent: Some("chris".to_string()),
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
        prompt_digest: ahu::util::digest_bytes(prompt.as_bytes()),
        harness_executable: std::path::PathBuf::from("claude"),
        materialize: Default::default(),
        launch_command: command.redacted(),
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

/// Every adapter must keep the prompt a single literal argv element and must
/// not widen the harness's own permission or sandbox defaults.
#[test]
fn every_adapter_delivers_the_prompt_literally_and_widens_no_permissions() {
    for (harness, model, expect_agent_flag) in [
        ("claude-code", "claude-opus-5", true),
        ("codex", "gpt-6-astra", false),
        ("antigravity", "gemini-3.1-pro-high", true),
    ] {
        let adapter = harness::adapter_for(harness).unwrap();
        let command = adapter
            .launch_command(&LaunchRequest {
                model,
                native_agent: Some("chris"),
                prompt: HOSTILE_PROMPT,
                cwd: Path::new("/tmp"),
            })
            .unwrap_or_else(|e| panic!("{harness}: {e}"));

        // The prompt survives byte for byte, in exactly one element.
        let index = command
            .prompt_arg
            .expect("{harness} records the prompt slot");
        assert_eq!(command.args[index], HOSTILE_PROMPT, "{harness}");
        assert_eq!(
            command
                .args
                .iter()
                .filter(|a| a.contains("$(touch"))
                .count(),
            1,
            "{harness} must hold the prompt exactly once"
        );
        // The exact model is pinned.
        assert!(command.args.iter().any(|a| a == model), "{harness}");
        // Agent selection is requested where the harness has the concept.
        assert_eq!(
            command.args.iter().any(|a| a == "--agent"),
            expect_agent_flag,
            "{harness} agent flag"
        );
        // No adapter may widen permissions, sandboxing, or approvals.
        for forbidden in [
            "--dangerously-skip-permissions",
            "--dangerously-bypass-approvals-and-sandbox",
            "--dangerously-bypass-hook-trust",
            "--permission-mode",
            "--approve-for-me",
            "--sandbox",
            "--ask-for-approval",
            "--yolo",
            "--add-dir",
        ] {
            assert!(
                !command.args[..index].iter().any(|a| a == forbidden),
                "{harness} must not pass {forbidden}"
            );
        }
        // Redaction keeps the prompt out of the stored record.
        assert_ne!(command.redacted().args[index], HOSTILE_PROMPT, "{harness}");
    }
}

/// Only Claude Code can actually enforce a named agent's system prompt. The
/// other two say so instead of implying a guarantee they cannot keep.
#[test]
fn adapters_report_their_real_enforcement_limits() {
    for (harness, model) in [
        ("claude-code", "claude-opus-5"),
        ("codex", "gpt-6-astra"),
        ("antigravity", "gemini-3.1-pro-high"),
    ] {
        let report = harness::adapter_for(harness).unwrap().enforcement(model);
        assert_eq!(report.harness, harness);
        assert!(
            !report.model_fixed_for_session,
            "{harness}: none of the three can hold a model for a whole session"
        );
        assert!(report.needs_reliability_warning(), "{harness}");
        assert!(!report.gaps.is_empty(), "{harness} must name its gaps");
        assert!(
            !report.applied_controls.is_empty(),
            "{harness} must name what it does control"
        );
    }

    // The two harnesses without working per-agent selection must say so.
    for harness in ["codex", "antigravity"] {
        let report = harness::adapter_for(harness).unwrap().enforcement("x");
        assert!(
            report
                .gaps
                .iter()
                .any(|g| g.contains("instructions in the prompt")),
            "{harness} must tell the user where the instructions have to go"
        );
    }
}

/// The launch and the integrity check must never disagree about whether the
/// harness was asked for a named agent.
///
/// Codex has no per-agent selection, so a Codex-backed agent is launched
/// without one. If the plan and `run_task` derived that independently they
/// could differ, and the session would be refused for a mismatch that was
/// ahu's own doing.
#[test]
fn the_native_agent_decision_is_recorded_not_re_derived() {
    use ahu::agent::SourceFormat;

    assert!(SourceFormat::ClaudeAgent.selects_native_agent());
    assert!(SourceFormat::AntigravityAgent.selects_native_agent());
    assert!(
        !SourceFormat::CodexAgent.selects_native_agent(),
        "Codex has no --agent flag"
    );
    assert!(!SourceFormat::Markdown.selects_native_agent());

    // A rebuild that assumes "named launch means --agent" produces a different
    // command for a harness that has no such flag.
    let adapter = harness::adapter_for("codex").unwrap();
    let without = adapter
        .launch_command(&LaunchRequest {
            model: "gpt-6-astra",
            native_agent: None,
            prompt: "p",
            cwd: Path::new("/tmp"),
        })
        .unwrap();
    assert!(!without.args.iter().any(|a| a == "--agent"));

    // And for Antigravity, a named agent must be requested by name.
    let antigravity = harness::adapter_for("antigravity").unwrap();
    let with = antigravity
        .launch_command(&LaunchRequest {
            model: "gemini-3.1-pro-high",
            native_agent: Some("vela"),
            prompt: "p",
            cwd: Path::new("/tmp"),
        })
        .unwrap();
    let index = with.args.iter().position(|a| a == "--agent").unwrap();
    assert_eq!(with.args[index + 1], "vela");
}

/// A wrapper between ahu and the harness can add flags ahu refuses to pass, so
/// ahu must disclose it rather than claim the harness's defaults are intact.
#[test]
fn a_wrapper_on_the_path_is_disclosed_as_an_enforcement_gap() {
    let shim =
        Path::new("/var/folders/xx/T/cmux-cli-shims/00000000-0000-0000-0000-000000000000/codex");
    let note = harness::wrapper_interposed(shim).expect("a cmux shim must be detected");
    assert!(note.contains("cmux shim"), "{note}");
    assert!(note.contains("cannot inspect"), "{note}");

    // A real binary is not flagged.
    assert!(harness::wrapper_interposed(Path::new("/usr/local/bin/codex")).is_none());

    // No adapter may claim a wrapper leaves the harness's defaults untouched.
    for harness_id in ["claude-code", "codex", "antigravity"] {
        let report = harness::adapter_for(harness_id).unwrap().enforcement("m");
        for control in &report.applied_controls {
            assert!(
                !control.contains("are unchanged"),
                "{harness_id} must not claim defaults are unchanged: {control}"
            );
        }
    }
}

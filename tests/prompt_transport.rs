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
            prompt: HOSTILE_PROMPT,
            cwd: Path::new("/tmp"),
            permissions: Default::default(),
        })
        .unwrap();
    assert_eq!(command.program, "claude");
    assert_eq!(
        command.args,
        vec![
            "--model".to_string(),
            "claude-opus-5".to_string(),
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
            prompt: "--dangerously-skip-permissions",
            cwd: Path::new("/tmp"),
            permissions: Default::default(),
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
fn file_inline_and_stdin_prompts_reach_the_argv_boundary_byte_for_byte() {
    use ahu::cli::{Command, parse_with_stdin};
    use std::io::Cursor;

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("assignment.txt");
    let prompt = format!("\n{HOSTILE_PROMPT}\r\nUnicode: λ 🗿\n\n");
    std::fs::write(&path, &prompt).unwrap();
    let cases = [
        (
            vec![
                "launch",
                "@reviewer",
                "--prompt-file",
                path.to_str().unwrap(),
            ],
            "unused stdin",
        ),
        (
            vec!["launch", "@reviewer", "--prompt", prompt.as_str()],
            "unused stdin",
        ),
        (vec!["launch", "@reviewer"], prompt.as_str()),
    ];
    for (args, stdin) in cases {
        let Command::Launch { prompt: source, .. } = parse_with_stdin(args, true).unwrap() else {
            panic!("expected launch");
        };
        let text = source
            .read(&mut Cursor::new(stdin.as_bytes()), false)
            .unwrap();
        assert_eq!(text.as_bytes(), prompt.as_bytes());
        let (delivered, _) =
            ahu::orchestration::deliver(Some("Fixture instructions."), &text).unwrap();
        for (harness, model) in [
            ("claude-code", "claude-opus-5"),
            ("codex", "gpt-6-astra"),
            ("antigravity", "gemini-3.1-pro-high"),
        ] {
            let command = harness::adapter_for(harness)
                .unwrap()
                .launch_command(&LaunchRequest {
                    model,
                    prompt: &delivered,
                    cwd: temp.path(),
                    permissions: Default::default(),
                })
                .unwrap();
            let index = command.prompt_arg.unwrap();
            assert_eq!(command.args[index], delivered);
            assert!(command.args[index].ends_with(&prompt));
            assert_eq!(
                command
                    .args
                    .iter()
                    .filter(|arg| arg.contains(&prompt))
                    .count(),
                1
            );
            assert!(!command.redacted().args[index].contains(&prompt));
        }
    }
}

#[test]
fn prompt_sources_reject_empty_or_invalid_text_without_interpreting_it() {
    use ahu::cli::PromptSource;
    use std::io::Cursor;

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("empty.txt");
    std::fs::write(&path, " \r\n\t").unwrap();
    for source in [
        PromptSource::File(path.clone()),
        PromptSource::Inline(" \n".into()),
        PromptSource::Stdin,
    ] {
        assert!(source.read(&mut Cursor::new(b" \n"), false).is_err());
    }
    std::fs::write(&path, [0xff]).unwrap();
    assert!(
        PromptSource::File(path)
            .read(&mut Cursor::new(b""), false)
            .is_err()
    );
    assert!(
        PromptSource::Stdin
            .read(&mut Cursor::new([0xff]), false)
            .is_err()
    );
    assert!(
        PromptSource::Stdin
            .read(&mut Cursor::new(b"valid"), true)
            .is_err()
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
    // `run_task` re-derives the worktree from the repository it verifies.
    let worktree = ahu::git::discover(repo.path())
        .unwrap()
        .root
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
    write_task_record(&task_dir, &worktree, &prompt, &bin.join("claude"));

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
    // ahu pins the model with a real flag, and passes nothing else before `--`.
    assert_eq!(args[0], "--model");
    assert_eq!(args[1], "claude-opus-5");
    assert_eq!(args[2], "--");
    let separator = 2;
    // Everything ahu supplies is inside the single prompt argument now. The fake
    // harness records one argument per line, so compare the whole tail.
    let delivered = args[separator + 1..].join("\n");
    assert!(
        delivered.contains(ahu::orchestration::INSTRUCTIONS.trim()),
        "{delivered}"
    );
    assert!(
        delivered.contains("You are chris."),
        "the agent's instructions travel in the prompt"
    );
    assert!(
        !delivered.contains("tools: Read, Edit"),
        "frontmatter is metadata ahu reads, not instructions it delivers: {delivered}"
    );
    assert!(
        delivered.ends_with(&prompt),
        "the task prompt is last and unmodified"
    );
    // ahu's sections are fenced and the task prompt is outside the fence.
    let record = ahu::task::load(&task_dir).unwrap();
    let close = ahu::orchestration::close_tag("agent", &record.delivery.nonce);
    assert!(delivered.find(&close).unwrap() < delivered.find(&prompt).unwrap());
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
    // `run_task` re-derives the worktree from the repository it verifies.
    let worktree = ahu::git::discover(repo.path())
        .unwrap()
        .root
        .join(".worktrees/testtask0001");
    std::fs::create_dir_all(&worktree).unwrap();
    let recorder = temp.path().join("argv.txt");
    let bin = fake_harness(temp.path(), &recorder);
    let task_dir = temp.path().join("task");
    write_task_record(&task_dir, &worktree, "do the thing", &bin.join("claude"));

    // Tamper with the frozen command, leaving the identity fields alone.
    // Only the frozen argv, leaving every identity field alone: the point is
    // that the recorded command is checked against what the identity produces,
    // not that the two copies of the model string agree with each other.
    let raw = std::fs::read_to_string(task_dir.join("task.json")).unwrap();
    let tampered = raw.replace(
        "\"claude-opus-5\",\n      \"--\"",
        "\"claude-haiku-4-5\",\n      \"--\"",
    );
    assert_ne!(tampered, raw, "the fixture must actually be changed");
    std::fs::write(task_dir.join("task.json"), tampered).unwrap();

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
fn write_task_record(task_dir: &Path, worktree: &Path, prompt: &str, harness_path: &Path) {
    // `run_task` recomputes the repository identity from the Git common
    // directory and refuses a record that names a different one, so the fixture
    // has to use the real values.
    let repo_root = worktree.parent().and_then(|p| p.parent()).unwrap();
    let discovered = ahu::git::discover(repo_root).expect("fixture repo");
    let repo_identity = discovered.identity();
    use ahu::harness::LaunchRequest;
    use ahu::task::{LaunchIdentity, LaunchMode, TaskRecord, TaskState};

    let adapter = ahu::harness::adapter_for("claude-code").unwrap();
    // The fixture composes the delivered prompt exactly as `launch::plan` does,
    // because `run_task` rebuilds it from the frozen delivery and compares.
    let (delivered, delivery) =
        ahu::orchestration::deliver(Some("You are chris.\n"), prompt).unwrap();
    let command = adapter
        .launch_command(&LaunchRequest {
            model: "claude-opus-5",
            prompt: &delivered,
            cwd: worktree,
            permissions: Default::default(),
        })
        .unwrap();
    let enforcement = adapter
        .enforcement("claude-opus-5", Default::default())
        .unwrap();
    let record = TaskRecord {
        schema_version: ahu::task::TASK_SCHEMA_VERSION,
        task_id: "testtask0001".to_string(),
        title: "fixture".to_string(),
        created_at: ahu::task::now_rfc3339(),
        repo_identity: repo_identity.clone(),
        repo_root: discovered.root.clone(),
        branch: "ahu/chris/testtask0001".to_string(),
        worktree: worktree.to_path_buf(),
        base_commit: Some("0".repeat(40)),
        identity: LaunchIdentity {
            mode: LaunchMode::Named,
            agent: "chris".to_string(),
            agent_version: Some("1.0.0".to_string()),
            permissions: Default::default(),
            harness: "claude-code".to_string(),
            model: "claude-opus-5".to_string(),
            instructions_source: Some(".claude/agents/chris.md".to_string()),
            source_digest: Some("0".repeat(64)),
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
        delivery,
        prompt_digest: ahu::util::digest_bytes(prompt.as_bytes()),
        harness_executable: harness_path.to_path_buf(),
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
    for (harness, model) in [
        ("claude-code", "claude-opus-5"),
        ("codex", "gpt-6-astra"),
        ("antigravity", "gemini-3.1-pro-high"),
    ] {
        let adapter = harness::adapter_for(harness).unwrap();
        let command = adapter
            .launch_command(&LaunchRequest {
                model,
                prompt: HOSTILE_PROMPT,
                cwd: Path::new("/tmp"),
                permissions: Default::default(),
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
        // No adapter selects an identity by name any more: a name is not bound
        // to the file ahu read, digested, and attributed the instructions to.
        assert!(
            !command.args.iter().any(|a| a == "--agent"),
            "{harness} must not select an agent by name"
        );
        // No adapter may widen permissions, sandboxing, or approvals.
        for forbidden in [
            "--agent",
            "--append-system-prompt",
            "--disallowedTools",
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
        let report = harness::adapter_for(harness)
            .unwrap()
            .enforcement(model, Default::default())
            .unwrap();
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

    // No adapter may claim to select or enforce an agent identity.
    for harness in ["claude-code", "codex", "antigravity"] {
        let report = harness::adapter_for(harness)
            .unwrap()
            .enforcement("x", Default::default())
            .unwrap();
        for control in &report.applied_controls {
            assert!(
                !control.contains("--agent"),
                "{harness} must not list agent selection as an applied control: {control}"
            );
        }
    }
}

/// The one sentence a reader of a launch preview most needs, and it is a gap.
///
/// It has to be in `gaps` rather than `applied_controls`, and it has to appear
/// for every harness, because the whole point of the uniform-delivery change is
/// that no harness is different here.
#[test]
fn every_launch_reports_prompt_delivery_as_a_gap_not_a_control() {
    use common::TestRepo;

    for (harness, model) in [
        ("claude-code", "claude-opus-5"),
        ("codex", "gpt-6-astra"),
        ("antigravity", "gemini-3.1-pro-high"),
    ] {
        let repo = TestRepo::new();
        repo.write(
            ".agents/ahu/config.toml",
            &format!(
                "schema_version = 1\n\
                 harness_preferences = [{harness:?}]\n\
                 model_selection = \"project-ranked\"\n\
                 catalog_version = {:?}\n\
                 \n[model_rankings]\n\
                 {harness:?} = [{model:?}]\n\
                 \n[context_hygiene]\n\
                 review_on_first_load = false\n\
                 review_interval_days = 7\n",
                ahu::catalog::CATALOG_VERSION
            ),
        );
        repo.add_agent_on("vela", "1.0.0", harness, model);
        repo.commit("fixture");

        let discovered = ahu::git::discover(repo.path()).unwrap();
        let loaded = ahu::config::load(repo.path()).unwrap().unwrap();
        let agent = ahu::agent::find(repo.path(), "vela").unwrap();
        let pair = ahu::selection::ResolvedPair {
            harness: harness.to_string(),
            model: model.to_string(),
            basis: "named agent".to_string(),
            policy_digest: loaded.digest.clone(),
            catalog_version: loaded.config.catalog_version.clone(),
        };
        let Ok(plan) = ahu::launch::plan(&discovered, Some(agent), pair, "review it") else {
            // The harness is not installed on this machine; `plan` resolves the
            // executable. Nothing to assert, and nothing to skip silently: the
            // claude-code case always runs, because the fixtures install a fake.
            continue;
        };

        assert!(
            plan.enforcement
                .gaps
                .contains(&ahu::launch::DELIVERY_IS_NOT_ENFORCEMENT.to_string()),
            "{harness} must report prompt delivery as a gap: {:?}",
            plan.enforcement.gaps
        );
        for control in &plan.enforcement.applied_controls {
            assert!(
                !control.contains("not an enforced system prompt"),
                "{harness} must not dress the gap up as a control: {control}"
            );
        }
        let preview = ahu::commands::render_preview(&discovered, &plan, "review it", None);
        assert!(
            preview.contains("not an enforced system prompt"),
            "the preview must say it plainly: {preview}"
        );
        assert!(
            preview.contains("no harness enforces this agent's identity"),
            "{preview}"
        );
        // And it must still say where the instructions came from.
        assert!(
            preview.contains(".agents/ahu/instructions/vela.md"),
            "the attribution must survive: {preview}"
        );
    }
}

/// A source format decides how a file is parsed, and nothing else.
///
/// It used to also decide whether ahu asked the harness for the agent by name.
/// That was the unsound part: `--agent <name>` resolves through the harness's own
/// agent search, which is not bound to the file ahu read and digested, so ahu
/// asserted a binding it could not check. No adapter takes an agent name now, so
/// there is nothing left for a format to select.
#[test]
fn a_source_format_only_decides_how_its_file_is_parsed() {
    use ahu::agent::SourceFormat;

    assert!(SourceFormat::ClaudeAgent.has_frontmatter());
    assert!(SourceFormat::AntigravityAgent.has_frontmatter());
    assert!(!SourceFormat::Markdown.has_frontmatter());

    // The parse is what ahu delivers: frontmatter is metadata, the body is the
    // instruction text.
    let repo = TestRepo::new();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.write(
        ".agents/ahu/instructions/plain.md",
        "---\nnot: frontmatter\n---\nplain body\n",
    );
    repo.write(
        ".agents/ahu/agents/plain.toml",
        "schema_version = 1\nname = \"plain\"\nversion = \"1.0.0\"\n\
         harness = \"claude-code\"\nmodel = \"claude-opus-5\"\n\
         \n[source]\nformat = \"markdown\"\npath = \".agents/ahu/instructions/plain.md\"\n",
    );
    repo.commit("fixture");

    let claude_agent = ahu::agent::find(repo.path(), "chris").unwrap();
    assert_eq!(claude_agent.instructions.trim(), "You are chris.");
    assert!(!claude_agent.instructions.contains("tools: Read, Edit"));
    assert_eq!(
        claude_agent
            .native_settings
            .get("tools")
            .map(String::as_str),
        Some("Read, Edit"),
        "frontmatter is still read, just not delivered"
    );

    // Plain Markdown is used as-is, frontmatter-looking text and all.
    let markdown = ahu::agent::find(repo.path(), "plain").unwrap();
    assert!(markdown.instructions.contains("not: frontmatter"));

    // And no adapter has anywhere to put a name even if one were derived.
    for harness_id in ["claude-code", "codex", "antigravity"] {
        let command = harness::adapter_for(harness_id)
            .unwrap()
            .launch_command(&LaunchRequest {
                model: match harness_id {
                    "codex" => "gpt-6-astra",
                    "antigravity" => "gemini-3.1-pro-high",
                    _ => "claude-opus-5",
                },
                prompt: "p",
                cwd: Path::new("/tmp"),
                permissions: Default::default(),
            })
            .unwrap();
        assert!(!command.args.iter().any(|a| a == "--agent"), "{harness_id}");
    }
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
        let report = harness::adapter_for(harness_id)
            .unwrap()
            .enforcement("m", Default::default())
            .unwrap();
        for control in &report.applied_controls {
            assert!(
                !control.contains("are unchanged"),
                "{harness_id} must not claim defaults are unchanged: {control}"
            );
        }
    }
}

/// Widening a harness's approval boundary is opt-in, per agent, and mapped onto
/// each harness's own native flag. Absent configuration passes nothing at all.
#[test]
fn approval_widening_is_opt_in_and_harness_native() {
    use ahu::agent::Permissions;

    // Default: ahu adds no permission flag anywhere.
    for harness_id in ["claude-code", "codex", "antigravity"] {
        let command = harness::adapter_for(harness_id)
            .unwrap()
            .launch_command(&LaunchRequest {
                model: match harness_id {
                    "codex" => "gpt-6-astra",
                    "antigravity" => "gemini-3.1-pro-high",
                    _ => "claude-opus-5",
                },
                prompt: "p",
                cwd: Path::new("/tmp"),
                permissions: Permissions::default(),
            })
            .unwrap();
        for flag in [
            "--permission-mode",
            "--dangerously-skip-permissions",
            "--approve-for-me",
            "--ask-for-approval",
            "--sandbox",
            "--mode",
        ] {
            assert!(
                !command.args.iter().any(|a| a == flag),
                "{harness_id} must pass no permission flag by default, found {flag}"
            );
        }
    }

    // Opt-in maps to the flag each harness actually documents.
    for (harness_id, model, mode, expected) in [
        (
            "claude-code",
            "claude-opus-5",
            Permissions::Auto,
            vec!["--permission-mode", "auto"],
        ),
        (
            "claude-code",
            "claude-opus-5",
            Permissions::AcceptEdits,
            vec!["--permission-mode", "acceptEdits"],
        ),
        (
            "antigravity",
            "gemini-3.1-pro-high",
            Permissions::Auto,
            vec!["--dangerously-skip-permissions"],
        ),
        (
            "antigravity",
            "gemini-3.1-pro-high",
            Permissions::AcceptEdits,
            vec!["--mode", "accept-edits"],
        ),
        (
            "codex",
            "gpt-6-astra",
            Permissions::Auto,
            vec!["--ask-for-approval", "never"],
        ),
        (
            "codex",
            "gpt-6-astra",
            Permissions::AcceptEdits,
            vec!["--approve-for-me"],
        ),
    ] {
        let command = harness::adapter_for(harness_id)
            .unwrap()
            .launch_command(&LaunchRequest {
                model,
                prompt: HOSTILE_PROMPT,
                cwd: Path::new("/tmp"),
                permissions: mode,
            })
            .unwrap();
        for flag in &expected {
            assert!(
                command.args.iter().any(|a| a == flag),
                "{harness_id}/{} should pass {flag}: {:?}",
                mode.as_str(),
                command.args
            );
        }
        // The prompt is still exactly one literal element after all of that.
        let index = command.prompt_arg.unwrap();
        assert_eq!(command.args[index], HOSTILE_PROMPT, "{harness_id}");
    }

    // Every mode explains itself, and only the widening ones say so.
    assert!(!Permissions::Prompt.widens_defaults());
    assert!(Permissions::AcceptEdits.widens_defaults());
    assert!(Permissions::Auto.widens_defaults());
    assert!(Permissions::Auto.disclosure().contains("unattended"));
}

/// The Enforcement block must never deny passing a flag the launch passes.
///
/// Regression test for a contradiction in the submission preview: `enforcement`
/// returned a fixed `applied_controls` list asserting "ahu passes no
/// --permission-mode, ..." while `launch_command` pushed exactly that flag for
/// an agent whose manifest declared `permissions = accept-edits` or `auto`. The
/// Approvals block disclosed the widening three lines above, so the preview
/// simultaneously stated and denied the same fact — and every reviewer agent in
/// this repository is declared `permissions = "auto"`, so it was the common
/// case, not a corner.
///
/// The check is structural rather than a string match on today's wording: for
/// every adapter and every permission level, parse each "passes no A, B, or C"
/// clause out of the controls and assert none of those flags appears in the
/// argument vector the same adapter just built.
#[test]
fn no_enforcement_control_denies_a_flag_the_launch_actually_passes() {
    use ahu::agent::Permissions;

    for (harness, model) in [
        ("claude-code", "claude-opus-5"),
        ("codex", "gpt-6-astra"),
        ("antigravity", "gemini-3.1-pro-high"),
    ] {
        for permissions in [
            Permissions::Prompt,
            Permissions::AcceptEdits,
            Permissions::Auto,
        ] {
            let adapter = harness::adapter_for(harness).unwrap();
            let command = adapter
                .launch_command(&LaunchRequest {
                    model,
                    prompt: "do the thing",
                    cwd: Path::new("/tmp/ahu-fixture-worktree"),
                    permissions,
                })
                .unwrap();
            let report = adapter.enforcement(model, permissions).unwrap();

            for control in &report.applied_controls {
                for denied in denied_flags(control) {
                    assert!(
                        !command.args.contains(&denied),
                        "{harness} with permissions = {}: the Enforcement block says \
                         \"passes no {denied}\" but the launch command is {:?}\n  control: {control}",
                        permissions.as_str(),
                        command.args,
                    );
                }
            }
        }
    }
}

/// Pull the flags out of every "passes no A, B, or C" clause in a control line.
fn denied_flags(control: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = control;
    while let Some(at) = rest.find("passes no ") {
        let clause = &rest[at + "passes no ".len()..];
        // A clause runs to the end of the sentence.
        let clause = clause.split(';').next().unwrap_or(clause);
        for token in clause.split([',', ' ']) {
            let token = token.trim().trim_end_matches(['.', ';']);
            if token.starts_with("--") {
                found.push(token.to_string());
            }
        }
        rest = &rest[at + "passes no ".len()..];
    }
    found
}

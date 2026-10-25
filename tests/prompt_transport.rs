//! Safe prompt transport.
//!
//! A pasted prompt may contain anything a shell would interpret. It must reach
//! the harness byte for byte, and it must never be evaluated on the way there.
//! ahu achieves this by keeping the prompt out of every shell string: cmux's
//! startup command carries only ahu's own quoted paths, and the harness is
//! started with an argument vector.

mod common;

use std::path::{Path, PathBuf};

use ahu::harness::{self, LaunchRequest};
use ahu::interactive;
use ahu::util::shell_single_quote;
use common::{HOSTILE_PROMPT, TestRepo, fake_harness};

#[test]
fn the_prompt_is_a_single_argument_reproduced_byte_for_byte() {
    if !common::in_harness_fixture("the_prompt_is_a_single_argument_reproduced_byte_for_byte") {
        return;
    }
    let plan = common::runtime_plan(
        "sixth",
        &LaunchRequest {
            model: "sixth/exact",
            prompt: HOSTILE_PROMPT,
            cwd: Path::new("/tmp"),
            permissions: Default::default(),
        },
    )
    .unwrap();
    let index = plan.command.prompt_arg.unwrap();
    assert_eq!(
        ahu::orchestration::fence_body(&plan.command.args[index], "request", &plan.delivery.nonce),
        Some(HOSTILE_PROMPT)
    );
    assert_eq!(
        plan.command
            .args
            .iter()
            .filter(|a| a.contains(HOSTILE_PROMPT))
            .count(),
        1
    );
    assert_eq!(
        &plan.command.args[..index],
        ["--model", "sixth/exact", "--"]
    );
}

#[test]
fn a_prompt_that_looks_like_an_option_is_still_a_prompt() {
    if !common::in_harness_fixture("a_prompt_that_looks_like_an_option_is_still_a_prompt") {
        return;
    }
    let prompt = "--dangerously-skip-permissions";
    let plan = common::runtime_plan(
        "sixth",
        &LaunchRequest {
            model: "sixth/exact",
            prompt,
            cwd: Path::new("/tmp"),
            permissions: Default::default(),
        },
    )
    .unwrap();
    let index = plan.command.prompt_arg.unwrap();
    assert_eq!(plan.command.args[index - 1], "--");
    assert_eq!(
        ahu::orchestration::fence_body(&plan.command.args[index], "request", &plan.delivery.nonce),
        Some(prompt)
    );
    assert!(
        !plan.command.args[..index]
            .iter()
            .any(|arg| arg.contains("permission") || arg.contains("dangerously"))
    );
}

#[test]
fn file_inline_and_stdin_prompts_reach_the_argv_boundary_byte_for_byte() {
    if !common::in_harness_fixture(
        "file_inline_and_stdin_prompts_reach_the_argv_boundary_byte_for_byte",
    ) {
        return;
    }
    use ahu::cli::{Command, parse_with_stdin};
    use std::io::Cursor;

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("assignment.txt");
    let prompt = format!("\n{HOSTILE_PROMPT}\r\nUnicode: λ 🗿\n\n");
    std::fs::write(&path, &prompt).unwrap();
    let cases = [
        (
            vec!["@reviewer", "--prompt-file", path.to_str().unwrap()],
            "unused stdin",
        ),
        (
            vec!["@reviewer", "--prompt", prompt.as_str()],
            "unused stdin",
        ),
        (vec!["@reviewer"], prompt.as_str()),
    ];
    for (args, stdin) in cases {
        let Command::Launch { prompt: source, .. } = parse_with_stdin(args, true).unwrap() else {
            panic!("expected launch");
        };
        let text = source
            .read(&mut Cursor::new(stdin.as_bytes()), false)
            .unwrap();
        assert_eq!(text.as_bytes(), prompt.as_bytes());
        let (delivered, delivery) =
            ahu::orchestration::deliver(Some("Fixture instructions."), &text).unwrap();
        for (harness, model) in [
            ("claude-code", "claude-opus-5"),
            ("codex", "gpt-6-astra"),
            ("antigravity", "gemini-3.1-pro-high"),
            ("opencode", "ollama/glm-5.3:cloud"),
        ] {
            let plan = common::runtime_plan(
                harness,
                &LaunchRequest {
                    model,
                    prompt: &delivered,
                    cwd: temp.path(),
                    permissions: Default::default(),
                },
            )
            .unwrap();
            let command = &plan.command;
            let index = command.prompt_arg.unwrap();
            let inner = ahu::orchestration::fence_body(
                &command.args[index],
                "request",
                &plan.delivery.nonce,
            )
            .unwrap();
            assert_eq!(inner, delivered);
            assert_eq!(
                ahu::orchestration::fence_body(inner, "request", &delivery.nonce),
                Some(prompt.as_str())
            );
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

    let command = interactive::startup_command(&exe, &nasty);
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
    let state_capture = temp.path().join("session-state.txt");
    let script = bin.join("claude");
    let original = std::fs::read_to_string(&script).unwrap();
    std::fs::write(
        &script,
        original.replace(
            "exit 0",
            &format!(
                "printf '%s' \"$AHU_STATE_DIR\" > {}\nexit 0",
                shell_single_quote(&state_capture.to_string_lossy())
            ),
        ),
    )
    .unwrap();
    let canary = temp.path().join("ahu-pwned");
    let prompt = format!(
        "{HOSTILE_PROMPT}\nalso $(touch {canary}) and `touch {canary}`",
        canary = canary.display()
    );

    let task_dir = temp.path().join("task");
    write_task_record(&task_dir, &worktree, &prompt, &bin.join("claude"));

    let output = common::ahu()
        .args(["run-task", "--task-dir", &task_dir.to_string_lossy()])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        // Point cmux discovery at nothing so the test never touches a real session.
        .output()
        .expect("ahu runs");
    assert!(
        output.status.success(),
        "ahu run-task failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(std::fs::read_to_string(&state_capture).unwrap(), "");
    assert!(worktree.join(".ahu/.gitignore").is_file());
    assert!(!worktree.join(".worktrees").exists());
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
    let record = ahu::task::load(&task_dir).unwrap();
    assert_eq!(
        ahu::orchestration::fence_body(&delivered, "request", &record.delivery.nonce),
        Some(prompt.as_str())
    );
    // The request follows the agent's instructions in its own fence.
    let close = ahu::orchestration::close_tag("agent", &record.delivery.nonce);
    assert!(delivered.find(&close).unwrap() < delivered.find(&prompt).unwrap());
    assert!(
        !canary.exists(),
        "command substitution inside the prompt was executed"
    );

    // Legacy reliability warnings are not replayed in the session.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains(harness::RELIABILITY_WARNING), "{stderr}");
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

    let output = common::ahu()
        .args(["run-task", "--task-dir", &task_dir.to_string_lossy()])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
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

    // The fixture composes the delivered prompt exactly as `launch::plan` does,
    // because `run_task` rebuilds it from the frozen delivery and compares.
    let (delivered, delivery) = ahu::orchestration::deliver_composed(
        Some("You are chris.\n"),
        prompt,
        ahu::orchestration::Composition {
            mode: ahu::orchestration::Mode::Interactive,
            metadata: Some(ahu::orchestration::Metadata {
                task_id: "testtask0001".into(),
                agent: "chris@1.0.0".into(),
                harness: "claude-code".into(),
                model: "claude-opus-5".into(),
                permissions: Default::default(),
            }),
            state: None,
        },
    )
    .unwrap();
    let command = common::record_command(&LaunchRequest {
        model: "claude-opus-5",
        prompt: &delivered,
        cwd: worktree,
        permissions: Default::default(),
    });
    let enforcement = common::record_enforcement("claude-code");
    let record = TaskRecord {
        schema_version: ahu::task::TASK_SCHEMA_VERSION,
        task_id: "testtask0001".to_string(),
        title: "fixture".to_string(),
        summary: String::new(),
        created_at: ahu::task::now_rfc3339(),
        repo_identity: repo_identity.clone(),
        repo_root: discovered.root.clone(),
        branch: "ahu/chris/testtask0001".to_string(),
        worktree: worktree.to_path_buf(),
        base_commit: Some("0".repeat(40)),
        identity: LaunchIdentity {
            release_id: None,
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
        config_snapshot: Default::default(),
        config_snapshot_digest: "0".repeat(64),
        hooks: Default::default(),
        hooks_digest: String::new(),
        delivery,
        prompt_digest: ahu::util::digest_bytes(prompt.as_bytes()),
        harness_executable: harness_path.to_path_buf(),
        materialize: Default::default(),
        launch_command: command.redacted(),
        reliability_warning: Some(harness::RELIABILITY_WARNING.to_string()),
        enforcement,
        session_host: None,
        session_ref: None,
        state: TaskState::Starting,
    };
    ahu::task::save(task_dir, &record, prompt).unwrap();
}

/// Every adapter must keep the prompt a single literal argv element and must
/// not widen the harness's own permission or sandbox defaults.
#[test]
fn every_adapter_delivers_the_prompt_literally_and_widens_no_permissions() {
    if !common::in_harness_fixture(
        "every_adapter_delivers_the_prompt_literally_and_widens_no_permissions",
    ) {
        return;
    }
    for harness in ["sixth", "codex", "claude-code"] {
        let plan = common::runtime_plan(
            harness,
            &LaunchRequest {
                model: "synthetic/exact",
                prompt: HOSTILE_PROMPT,
                cwd: Path::new("/tmp"),
                permissions: Default::default(),
            },
        )
        .unwrap();
        let command = &plan.command;
        let index = command.prompt_arg.unwrap();
        assert_eq!(
            ahu::orchestration::fence_body(&command.args[index], "request", &plan.delivery.nonce),
            Some(HOSTILE_PROMPT)
        );
        assert_eq!(
            command
                .args
                .iter()
                .filter(|arg| arg.contains(HOSTILE_PROMPT))
                .count(),
            1
        );
        assert_eq!(&command.args[..index], ["--model", "synthetic/exact", "--"]);
        assert!(!command.redacted().args[index].contains(HOSTILE_PROMPT));
    }
}

/// No adapter can actually enforce a named agent's system prompt. Each says so
/// instead of implying a guarantee it cannot keep.
#[test]
fn adapters_report_their_real_enforcement_limits() {
    if !common::in_harness_fixture("adapters_report_their_real_enforcement_limits") {
        return;
    }
    for harness in ["sixth", "codex", "claude-code"] {
        let plan = common::runtime_plan(
            harness,
            &LaunchRequest {
                model: "synthetic/exact",
                prompt: "review",
                cwd: Path::new("/tmp"),
                permissions: Default::default(),
            },
        )
        .unwrap();
        assert_eq!(plan.enforcement.harness, harness);
        assert!(!plan.enforcement.model_fixed_for_session);
        assert!(!plan.enforcement.gaps.is_empty());
        assert!(!plan.enforcement.applied_controls.is_empty());
        assert!(
            plan.enforcement
                .applied_controls
                .iter()
                .all(|c| !c.contains("--agent"))
        );
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

    if !common::in_harness_fixture("every_launch_reports_prompt_delivery_as_a_gap_not_a_control") {
        return;
    }

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
                 \n[model_rankings]\n\
                 {harness:?} = [{model:?}]\n\
",
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
            runtime_binding_digest: ahu::harness_runtime::load(&discovered, harness)
                .unwrap()
                .binding_digest(),
        };
        // Every harness resolves, because the fixture PATH installs a fake for
        // each one. Skipping a harness here would have made "no harness is
        // different" a claim about whichever harness happened to be installed.
        let plan = ahu::launch::plan(&discovered, Some(agent), pair, "review it")
            .unwrap_or_else(|e| panic!("the fixture PATH must resolve {harness}: {e}"));

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
            preview.contains("current terminal and installed session-host plugins are optional"),
            "agent assignment must not require cmux: {preview}"
        );
        assert!(
            !preview.contains("separate cmux sessions"),
            "agent assignment must not imply cmux is required: {preview}"
        );
        assert!(!preview.contains(harness::RELIABILITY_WARNING), "{preview}");
        assert!(
            !preview.contains("no harness enforces this agent's identity"),
            "{preview}"
        );
        assert!(preview.contains(".agents/ahu/lock.md"), "{preview}");
        // And it must still say where the instructions came from.
        assert!(
            preview.contains(".agents/ahu/agents/vela.md"),
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

    // A codex definition holds no frontmatter: whatever the file holds is the
    // verbatim instruction text, frontmatter-looking text and all.
    assert!(SourceFormat::ClaudeAgent.has_frontmatter());
    assert!(SourceFormat::AntigravityAgent.has_frontmatter());
    assert!(!SourceFormat::CodexAgent.has_frontmatter());
    assert!(SourceFormat::OpenCodeAgent.has_frontmatter());

    // The parse is what ahu delivers: frontmatter is metadata, the body is the
    // instruction text.
    let repo = TestRepo::new();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.write(
        ".codex/agents/plain.toml",
        "---\nnot: frontmatter\n---\nplain body\n",
    );
    repo.write(
        ".agents/ahu/agents/plain.md",
        "---\nokf_version: 0.2\ntype: ahu:agent\ntitle: plain\ndescription: fixture agent\n\
         status: stable\ntags: [agents]\nharness: codex\nmodel: gpt-6-astra\n\
         permissions: prompt\nversion: 1.0.0\nsource_format: codex-agent\n\
         source_path: .codex/agents/plain.toml\n\n---\n\nInstructions live in the native \
         definition at `.codex/agents/plain.toml`, referenced in place and never edited.\n",
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

    // The codex-agent parse is verbatim: nothing is stripped, so the
    // frontmatter-looking text is delivered as instruction text.
    let plain = ahu::agent::find(repo.path(), "plain").unwrap();
    assert!(plain.instructions.contains("not: frontmatter"));

    // And no adapter has anywhere to put a name even if one were derived.
    for harness_id in ["claude-code", "codex", "antigravity"] {
        let command = common::runtime_plan(
            harness_id,
            &LaunchRequest {
                model: match harness_id {
                    "codex" => "gpt-6-astra",
                    "antigravity" => "gemini-3.1-pro-high",
                    _ => "claude-opus-5",
                },
                prompt: "p",
                cwd: Path::new("/tmp"),
                permissions: Default::default(),
            },
        )
        .unwrap()
        .command;
        assert!(!command.args.iter().any(|a| a == "--agent"), "{harness_id}");
    }
}

/// Widening a harness's approval boundary is opt-in, per agent, and mapped onto
/// each harness's own native flag. Absent configuration passes nothing at all.
#[test]
fn approval_widening_is_opt_in_and_harness_native() {
    if !common::in_harness_fixture("approval_widening_is_opt_in_and_harness_native") {
        return;
    }
    use ahu::agent::Permissions;
    for (mode, flags) in [
        (Permissions::Prompt, vec![]),
        (Permissions::AcceptEdits, vec!["--accept-edits"]),
        (Permissions::Auto, vec!["--auto"]),
    ] {
        let plan = common::runtime_plan(
            "sixth",
            &LaunchRequest {
                model: "sixth/exact",
                prompt: HOSTILE_PROMPT,
                cwd: Path::new("/tmp"),
                permissions: mode,
            },
        )
        .unwrap();
        assert_eq!(plan.permissions, mode);
        let index = plan.command.prompt_arg.unwrap();
        let mut expected = vec!["--model", "sixth/exact"];
        expected.extend(flags);
        expected.push("--");
        assert_eq!(plan.command.args[..index], expected);
        assert_eq!(
            ahu::orchestration::fence_body(
                &plan.command.args[index],
                "request",
                &plan.delivery.nonce
            ),
            Some(HOSTILE_PROMPT)
        );
    }
    assert!(!Permissions::Prompt.widens_defaults());
    assert!(Permissions::Auto.widens_defaults());
    assert!(Permissions::AcceptEdits.widens_defaults());
}

#[test]
fn native_mcp_approval_flags_are_owned_by_installed_plugins() {
    if !common::in_harness_fixture("native_mcp_approval_flags_are_owned_by_installed_plugins") {
        return;
    }
    let plan = common::runtime_plan(
        "sixth",
        &LaunchRequest {
            model: "sixth/exact",
            prompt: "review",
            cwd: Path::new("/tmp"),
            permissions: ahu::agent::Permissions::Auto,
        },
    )
    .unwrap();
    assert!(plan.command.args.contains(&"--auto".into()));
    assert!(
        plan.command
            .args
            .iter()
            .all(|arg| !arg.starts_with("mcp_servers."))
    );
    assert!(
        plan.enforcement
            .gaps
            .iter()
            .any(|gap| gap.contains("installed harness plugin"))
    );
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
    if !common::in_harness_fixture(
        "no_enforcement_control_denies_a_flag_the_launch_actually_passes",
    ) {
        return;
    }
    for permissions in [
        ahu::agent::Permissions::Prompt,
        ahu::agent::Permissions::AcceptEdits,
        ahu::agent::Permissions::Auto,
    ] {
        let plan = common::runtime_plan(
            "sixth",
            &LaunchRequest {
                model: "sixth/exact",
                prompt: "review",
                cwd: Path::new("/tmp"),
                permissions,
            },
        )
        .unwrap();
        for control in &plan.enforcement.applied_controls {
            for flag in denied_flags(control) {
                assert!(!plan.command.args.contains(&flag), "{control}");
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

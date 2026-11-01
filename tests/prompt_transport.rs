//! Safe prompt transport.
//!
//! A pasted prompt may contain anything a shell would interpret. It must reach
//! the harness byte for byte, and it must never be evaluated on the way there.
//! ahu achieves this by keeping the prompt out of every shell string: cmux's
//! startup command carries only ahu's own quoted paths, and the harness is
//! started with an argument vector.

mod common;

use std::path::Path;

use ahu::harness::{self, LaunchRequest};
use ahu::util::shell_single_quote;
use common::{HOSTILE_PROMPT, TestRepo};

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
            vec![
                "@reviewer",
                "--prompt-file",
                path.to_str().unwrap(),
                "--dry-run",
            ],
            "unused stdin",
        ),
        (
            vec!["@reviewer", "--prompt", prompt.as_str(), "--dry-run"],
            "unused stdin",
        ),
        (vec!["@reviewer", "--dry-run"], prompt.as_str()),
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
    if !common::in_harness_fixture("a_source_format_only_decides_how_its_file_is_parsed") {
        return;
    }
    // The parse is what ahu delivers: frontmatter is metadata, the body is the
    // instruction text.
    let repo = TestRepo::new();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.install_runtime("codex");
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

    // The installed fixture defines this opaque format as frontmatter plus
    // body. Core does not infer a parser from the format name.
    let plain = ahu::agent::find(repo.path(), "plain").unwrap();
    assert_eq!(plain.instructions, "plain body\n");

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

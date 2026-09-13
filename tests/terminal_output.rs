//! What repository content is allowed to do to the terminal.
//!
//! ahu's output is a security surface: the launch preview is the only thing
//! standing between a repository's hooks and a session that runs them. A
//! repository controls file names, hook commands, agent descriptions, and
//! configuration values, so none of those may carry a sequence that moves the
//! cursor, clears the screen, or reorders what the reader sees.

mod common;

#[test]
fn doctor_shows_project_harness_readiness_without_executable_paths() {
    let repo = common::TestRepo::new();
    repo.init_config();
    let scratch = tempfile::tempdir().unwrap();
    let bin = common::fake_harnesses(scratch.path(), &["claude", "codex", "agy"], |name| {
        scratch.path().join(format!("{name}-probed"))
    });
    std::fs::write(bin.join("claude"), "#!/bin/sh\nprintf '2.1.269\\n'\n").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
        .arg("doctor")
        .current_dir(repo.path())
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env_remove("AHU_STATE_DIR")
        .env("AHU_CMUX_BIN", scratch.path().join("missing-cmux"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    let harness_lines: Vec<_> = text
        .lines()
        .filter(|line| line.starts_with("harness "))
        .collect();
    assert_eq!(harness_lines, ["harness      claude-code 2.1.269 — ready"]);
    assert!(!text.contains("adapter available"), "{text}");
    assert!(!text.contains(&bin.display().to_string()), "{text}");
    assert!(!scratch.path().join("codex-probed").exists());
    assert!(!scratch.path().join("agy-probed").exists());
    assert!(text.contains("state        .ahu/state\n"), "{text}");
}

#[test]
fn normal_harness_capabilities_are_not_warnings_but_failures_still_surface() {
    let repo = common::TestRepo::new();
    repo.init_config();
    repo.add_agent("chris", "1.0.0", "claude-opus-5");
    repo.commit("fixture");
    let scratch = tempfile::tempdir().unwrap();
    let bin = common::fake_harness(scratch.path(), &scratch.path().join("args"));
    for args in [
        vec!["doctor"],
        vec![
            "launch",
            "@chris",
            "--prompt",
            "review",
            "--dry-run",
            "--output",
            "json",
        ],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
            .args(&args)
            .current_dir(repo.path())
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("AHU_STATE_DIR", repo.state_path())
            .env("AHU_CMUX_BIN", scratch.path().join("missing-cmux"))
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        for text in [&stdout, &stderr] {
            assert!(!text.contains(ahu::harness::RELIABILITY_WARNING), "{text}");
            assert!(!text.contains("Reliability warning"), "{text}");
        }
        if args[0] == "doctor" {
            assert!(
                !output.status.success(),
                "missing cmux must still fail doctor"
            );
            assert!(stdout.contains("cmux"), "{stdout}");
            assert!(!stdout.contains("in-session model switching"), "{stdout}");
        } else {
            assert!(output.status.success(), "{stderr}");
            let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(plan["enforcement"]["model_fixed_for_session"], false);
            assert!(!plan["enforcement"]["gaps"].as_array().unwrap().is_empty());
            assert!(
                plan["warnings"].as_array().unwrap().iter().all(|warning| {
                    warning.as_str().is_some_and(|text| !text.contains("model"))
                })
            );
            assert!(!stderr.contains("in-session model switching"), "{stderr}");
        }
    }
}

use ahu::util::{display_safe, display_safe_block};
use common::TestRepo;

/// Characters that reorder or hide text without being category `Cc`.
///
/// `char::is_control` covers `Cc` only, so bidi overrides and the zero-width
/// family used by Trojan Source pass straight through a `Cc`-only filter.
const INVISIBLE: &[char] = &[
    '\u{00ad}', // soft hyphen
    '\u{061c}', // arabic letter mark
    '\u{200b}', // zero width space
    '\u{200e}', // left-to-right mark
    '\u{200f}', // right-to-left mark
    '\u{202a}', // left-to-right embedding
    '\u{202e}', // right-to-left override
    '\u{2060}', // word joiner
    '\u{2066}', // left-to-right isolate
    '\u{2069}', // pop directional isolate
    '\u{2028}', // line separator
    '\u{2029}', // paragraph separator
    '\u{feff}', // zero width no-break space
];

#[test]
fn display_safe_neutralises_bidi_overrides_and_zero_width_characters() {
    for hostile in INVISIBLE {
        let rendered = display_safe(&format!("a{hostile}b"));
        assert!(
            !rendered.contains(*hostile),
            "U+{:04X} survived display_safe: {rendered:?}",
            *hostile as u32
        );
        assert!(
            rendered.starts_with('a') && rendered.ends_with('b'),
            "{rendered:?}"
        );
    }
}

#[test]
fn display_safe_block_neutralises_them_too_while_keeping_line_structure() {
    for hostile in INVISIBLE {
        let rendered = display_safe_block(&format!("a{hostile}b\nc\n"));
        assert!(
            !rendered.contains(*hostile),
            "U+{:04X} survived display_safe_block: {rendered:?}",
            *hostile as u32
        );
        assert_eq!(rendered.lines().count(), 2, "{rendered:?}");
    }
}

/// A `\xNN` escape cannot name a character above 0xFF unambiguously, so the
/// wide ones are rendered in the `\u{...}` form instead.
#[test]
fn escapes_name_the_character_they_replace() {
    assert_eq!(display_safe("\u{1b}[2J"), "\\x1b[2J");
    assert_eq!(display_safe("\u{202e}"), "\\u{202e}");
    assert_eq!(display_safe("\u{feff}"), "\\u{feff}");
}

/// Ordinary text, including non-ASCII text, must be left exactly alone.
#[test]
fn display_safe_leaves_legitimate_text_untouched() {
    for safe in ["plain", "héllo wörld", "日本語", "a-b_c.d/e", "emoji 🌱"] {
        assert_eq!(display_safe(safe), safe);
        assert_eq!(display_safe_block(safe), safe);
    }
}

/// The launch summary prints notes that carry repository-relative
/// configuration paths verbatim, so it has to escape them like every other
/// renderer does.
#[test]
fn launch_notes_are_escaped_before_they_reach_the_terminal() {
    let rendered = ahu::commands::render_launch_notes(&[
        "agent configuration changed while the task was being prepared: \
         .claude/x\u{1b}[2J\u{1b}[1;31mSAFE.json"
            .to_string(),
    ]);
    assert!(!rendered.contains('\u{1b}'), "{rendered:?}");
    assert!(rendered.contains("\\x1b[2J"), "{rendered:?}");
}

/// The error path is a renderer too. A repository controls the names of the
/// files ahu reads, and those names are embedded in error messages.
#[test]
fn a_hostile_file_name_in_an_error_cannot_repaint_the_terminal() {
    let repo = TestRepo::new();
    // An unparseable manifest whose *name* carries a screen-clearing sequence.
    // `ahu agents` reads every `.toml` here, so this is reached with no race
    // and no user action beyond running ahu in the checkout.
    repo.write(
        ".agents/ahu/agents/x\u{1b}[2J\u{1b}[1;31mSAFE.toml",
        "this is not valid toml\n",
    );

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
        .arg("agents")
        .current_dir(repo.path())
        .env("AHU_STATE_DIR", repo.state_path())
        .output()
        .expect("ahu runs");

    assert!(
        !output.status.success(),
        "the bad manifest should be an error"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains('\u{1b}'),
        "a raw escape reached the terminal: {stderr:?}"
    );
    assert!(stderr.contains("\\x1b[2J"), "{stderr:?}");
}

/// `ahu doctor` is a renderer like any other.
///
/// It catches errors and prints them inline, so it does not pass through
/// `main`'s `display_safe_block`. A repository-chosen `catalog_version` reaching
/// the terminal raw lets a checkout clear the screen in one of the two commands
/// the README names as the way to list hooks — erasing the hook disclosure
/// printed moments later in the same report.
#[test]
fn doctor_cannot_be_used_to_repaint_the_terminal() {
    let repo = TestRepo::new();
    repo.write(
        ".agents/ahu/config.toml",
        "schema_version = 1\n\
         harness_preferences = [\"claude-code\"]\n\
         model_selection = \"project-ranked\"\n\
         catalog_version = \"2026-09-12\u{1b}[2J\u{1b}[Hahu doctor: no problems found.\"\n\
         \n[model_rankings]\n\
         \"claude-code\" = [\"claude-opus-5\"]\n",
    );

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
        .arg("doctor")
        .current_dir(repo.path())
        .env("AHU_STATE_DIR", repo.state_path())
        .output()
        .expect("ahu runs");

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains('\u{1b}'),
        "a raw escape reached the terminal from doctor: {combined:?}"
    );
    assert!(combined.contains("\\x1b[2J"), "{combined:?}");
}

/// The task title is the one prompt-derived string that never passes through a
/// renderer: it becomes the cmux workspace name via `new-workspace --name`.
///
/// Filtering `char::is_control` alone left every character in `INVISIBLE`
/// intact, so a title could render in the sidebar as something other than what
/// it is.
#[test]
fn a_task_title_carries_no_character_that_reorders_or_hides_text() {
    for hostile in INVISIBLE {
        let title = ahu::util::task_title_from_prompt(&format!("fix the {hostile} parser"));
        assert!(
            !title.contains(*hostile),
            "U+{:04X} survived into a task title: {title:?}",
            *hostile as u32
        );
    }
    let title = ahu::util::task_title_from_prompt("fix \u{1b}[2J the parser");
    assert!(!title.contains('\u{1b}'), "{title:?}");

    // The length bound still holds: hostile characters are replaced before the
    // title is truncated, not escaped into something longer.
    let long = ahu::util::task_title_from_prompt(&format!("{}x", "a\u{202e}".repeat(200)));
    assert!(long.chars().count() <= 60, "{}", long.chars().count());
}

/// `ahu tasks` prints straight out of `task.json`, whose title came from the
/// prompt and whose branch and agent label came from repository configuration.
#[test]
fn tasks_output_escapes_what_it_prints() {
    let rendered = ahu::commands::render_launch_notes(&["x\u{202e}y".to_string()]);
    assert!(!rendered.contains('\u{202e}'), "{rendered:?}");
}

/// Styling belongs outside repository-derived spans, including text that was
/// neutralized before it reached the style helper.
#[test]
fn forced_styling_contains_hostile_descriptions() {
    let repo = TestRepo::new();
    repo.add_agent_on("fixture", "1.0.0", "codex", "gpt-6-astra");
    let hostile = format!("BEGIN{}\x1b[2JEND", INVISIBLE.iter().collect::<String>());
    let mut manifest: toml::Value =
        toml::from_str(&repo.read(".agents/ahu/agents/fixture.toml")).unwrap();
    manifest["description"] = toml::Value::String(hostile.clone());
    repo.write(
        ".agents/ahu/agents/fixture.toml",
        &toml::to_string(&manifest).unwrap(),
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
        .args(["agents", "--color=always"])
        .current_dir(repo.path())
        .env("NO_COLOR", "")
        .env("TERM", "dumb")
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains('\x1b'),
        "explicit always must override environment"
    );
    let start = text.find("BEGIN").unwrap();
    let end = text[start..].find("END").unwrap() + start + 3;
    assert_eq!(&text[start..end], display_safe(&hostile));
    assert!(!text[start..end].contains('\x1b'));
    for hostile in INVISIBLE {
        assert!(!text.contains(*hostile));
    }
}

#[test]
fn forced_styling_keeps_invalid_identity_fields_safe_in_errors() {
    for field in ["name", "version", "harness", "model"] {
        let repo = TestRepo::new();
        repo.add_agent_on("fixture", "1.0.0", "codex", "gpt-6-astra");
        let mut manifest: toml::Value =
            toml::from_str(&repo.read(".agents/ahu/agents/fixture.toml")).unwrap();
        manifest[field] = toml::Value::String("BEGIN\x1b[2J\u{202e}END".into());
        repo.write(
            ".agents/ahu/agents/fixture.toml",
            &toml::to_string(&manifest).unwrap(),
        );
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
            .args(["--color=always", "agents"])
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        let text = String::from_utf8(output.stderr).unwrap();
        let start = text.find("BEGIN").unwrap();
        let end = text[start..].find("END").unwrap() + start + 3;
        assert!(!text[start..end].contains('\x1b'), "{field}: {text:?}");
        assert!(!text.contains('\u{202e}'), "{field}: {text:?}");
        assert!(
            text.contains("[2J"),
            "hostile value remains visible: {text:?}"
        );
    }
}

#[test]
fn redirected_commands_match_explicit_plain_output() {
    use std::process::{Command, Stdio};
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent_on("fixture", "1.0.0", "codex", "gpt-6-astra");
    repo.commit("fixture configuration");
    // Invalid launch/focus/runner targets exercise their errors without creating
    // sessions or worktrees; the read-only command paths exercise their output.
    let cases: &[&[&str]] = &[
        &[],
        &["help"],
        &["--version"],
        &["explain"],
        &["explain", "--markdown"],
        &["explain", "--mermaid"],
        &["explain", "--open"],
        &["init"],
        &["agents"],
        &["onboard"],
        &["inventory", "@fixture"],
        &["hygiene", "@fixture"],
        &["tasks"],
        &["task", "missing"],
        &["task", "missing", "--output", "json"],
        &["diff", "missing"],
        &["codex"],
        &["doctor"],
        &["knowledge", "lint"],
        &["knowledge", "lint", "--output", "json"],
        &["focus", "missing"],
        &["launch", "@missing", "--prompt", "fixture", "--dry-run"],
        &["run-task", "--task-dir", "missing"],
    ];
    for args in cases {
        let run = |color: &str| {
            std::fs::remove_dir_all(repo.state_path()).unwrap();
            std::fs::create_dir(repo.state_path()).unwrap();
            let out = tempfile::NamedTempFile::new().unwrap();
            let output = Command::new(env!("CARGO_BIN_EXE_ahu"))
                .arg(color)
                .args(*args)
                .current_dir(repo.path())
                .env("AHU_STATE_DIR", repo.state_path())
                .env("AHU_CMUX_BIN", repo.state_path().join("absent-cmux"))
                .env("PATH", "/usr/bin:/bin")
                .env("TERM", "xterm-256color")
                .env_remove("NO_COLOR")
                .stdin(Stdio::null())
                .stdout(out.reopen().unwrap())
                .output()
                .unwrap();
            (
                output.status.code(),
                std::fs::read(out.path()).unwrap(),
                output.stderr,
            )
        };
        let plain = run("--color=never");
        let auto = run("--color=auto");
        assert_eq!(auto, plain, "command {args:?}");
        assert!(!auto.1.contains(&0x1b), "command {args:?}");
        assert!(!auto.2.contains(&0x1b), "command {args:?}");
    }
}

#[test]
fn composer_output_fixture() {
    let Ok(path) = std::env::var("AHU_TEST_COMPOSER_OUTPUT") else {
        return;
    };
    let choice = match std::env::var("AHU_TEST_COMPOSER_COLOR").unwrap().as_str() {
        "never" => ahu::style::ColorChoice::Never,
        "auto" => ahu::style::ColorChoice::Auto,
        "always" => ahu::style::ColorChoice::Always,
        _ => panic!("invalid fixture color"),
    };
    ahu::style::configure(Some(choice));
    let input = std::env::var("AHU_TEST_COMPOSER_INPUT").unwrap();
    let mut reader = std::io::Cursor::new(input.as_bytes());
    let mut output = Vec::new();
    let mut console = ahu::launcher::Console {
        input: &mut reader,
        output: &mut output,
        interactive: true,
    };
    let prompt = ahu::launcher::read_prompt(&mut console).unwrap();
    assert_eq!(
        prompt.as_deref(),
        (!input.starts_with(".cancel")).then_some("  fixture  \nsecond")
    );
    std::fs::write(path, output).unwrap();
}

#[test]
fn redirected_composer_matches_explicit_plain_output() {
    // Separate processes exercise the real stdout detection and isolate the
    // process-wide style policy. The test runner's output is kept separate from
    // the Console bytes so its timing cannot affect the comparison.
    for input in [
        "  fixture  \nsecond\n.\nyes\n",
        ".cancel\n",
        "  fixture  \nsecond\n",
    ] {
        let run = |color: &str| {
            let rendered = tempfile::NamedTempFile::new().unwrap();
            let stdout = tempfile::NamedTempFile::new().unwrap();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "composer_output_fixture"])
                .env("AHU_TEST_COMPOSER_OUTPUT", rendered.path())
                .env("AHU_TEST_COMPOSER_COLOR", color)
                .env("AHU_TEST_COMPOSER_INPUT", input)
                .env("TERM", "xterm-256color")
                .env_remove("NO_COLOR")
                .stdout(stdout.reopen().unwrap())
                .status()
                .unwrap();
            assert!(
                status.success(),
                "{}",
                std::fs::read_to_string(stdout.path()).unwrap()
            );
            std::fs::read(rendered.path()).unwrap()
        };
        let plain = run("never");
        assert_eq!(run("auto"), plain);
        assert_eq!(
            String::from_utf8(plain).unwrap(),
            concat!(
                "\nTask prompt. Paste or type as many lines as you like.\n",
                "Pasting does not submit. A separate confirmation follows the preview.\n",
                "  Finish: type `.` alone on a line, or end input.\n",
                "  Cancel: type `.cancel` alone on a line to abandon it.\n\n",
            )
        );
        assert!(run("always").contains(&0x1b));
    }
}

#[test]
fn bad_color_options_are_usage_errors() {
    for args in [
        vec!["--color=invalid"],
        vec!["--color"],
        vec!["--color=always", "--color=never"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
}

#[test]
fn preview_output_fixture() {
    let Ok(path) = std::env::var("AHU_TEST_PREVIEW_OUTPUT") else {
        return;
    };
    let color = match std::env::var("AHU_TEST_PREVIEW_COLOR").unwrap().as_str() {
        "always" => ahu::style::ColorChoice::Always,
        "auto" => ahu::style::ColorChoice::Auto,
        "never" => ahu::style::ColorChoice::Never,
        _ => panic!("invalid fixture color"),
    };
    ahu::style::configure(Some(color));
    let root = std::path::PathBuf::from(std::env::var_os("AHU_TEST_PREVIEW_REPO").unwrap());
    let repo = ahu::git::discover(&root).unwrap();
    let loaded = ahu::config::load(&root).unwrap().unwrap();
    let agent = ahu::agent::find(&root, "fixture").unwrap();
    let pair = ahu::selection::ResolvedPair {
        harness: "codex".into(),
        model: "gpt-6-astra".into(),
        basis: "named agent".into(),
        policy_digest: loaded.digest.clone(),
        catalog_version: loaded.config.catalog_version.clone(),
    };
    let prompt = "  synthetic prompt body  \nsecond line";
    let mut plan = ahu::launch::plan(&repo, Some(agent), pair, prompt).unwrap();
    // Mutate the resolved plan to exercise every free-text preview field,
    // including identities rejected earlier by manifest validation.
    let hostile = |field: &str| {
        format!(
            "BEGIN_{field}\x1b[0m\x1b[2J\r\nForged section\t{}END_{field}",
            INVISIBLE.iter().collect::<String>()
        )
    };
    let agent = plan.agent.as_mut().unwrap();
    agent.manifest.name = hostile("agent");
    agent.manifest.version = hostile("version");
    agent.source_path = root.join(hostile("source"));
    plan.pair.harness = hostile("harness");
    plan.pair.model = hostile("model");
    plan.pair.basis = hostile("basis");
    plan.pair.catalog_version = hostile("catalog");
    plan.title = hostile("title");
    plan.branch = hostile("branch");
    plan.worktree = root.join(hostile("worktree"));
    plan.base_commit = Some(hostile("base"));
    plan.snapshot.skipped_directories = vec![hostile("skipped")];
    plan.snapshot.unscanned_config = vec![hostile("unscanned")];
    plan.snapshot.symlinks = vec![hostile("symlink")];
    plan.hooks.unreadable = vec![hostile("unreadable")];
    plan.hooks.unscanned_harness = Some(hostile("hook_harness"));
    plan.hooks.hooks.push(ahu::hooks::Hook {
        event: hostile("event"),
        matcher: None,
        kind: "command".into(),
        command: Some("fixture-hook".into()),
        command_digest: "0".repeat(64),
        scope: ahu::hooks::Scope::ProjectLocal,
        source: hostile("hook_source"),
    });
    plan.enforcement.applied_controls = vec![hostile("control")];
    plan.enforcement.gaps = vec![hostile("gap")];
    plan.harness_executable = root.join(hostile("executable"));
    plan.command.args = vec![hostile("argument"), prompt.into()];
    plan.command.prompt_arg = Some(1);
    plan.delivery.nonce = "synthetic-delivery-nonce".into();
    plan.parent_dirty = true;
    plan.permissions = ahu::agent::Permissions::Auto;
    let compact = std::env::var("AHU_TEST_PREVIEW_KIND").unwrap() == "compact";
    let render = if compact {
        ahu::commands::render_launch_preview
    } else {
        ahu::commands::render_preview
    };
    let mut text = render(&repo, &plan, prompt, Some("a1b2c3"));
    assert!(!render(&repo, &plan, prompt, None).contains("Confirmation code"));
    assert!(
        !text.contains(prompt),
        "the preview must not echo or restyle the prompt body"
    );
    let mut input = std::io::Cursor::new(b"a1b2c3\n");
    let mut question = Vec::new();
    assert!(
        ahu::launcher::confirm_submit(
            &mut ahu::launcher::Console {
                input: &mut input,
                output: &mut question,
                interactive: true,
            },
            "a1b2c3"
        )
        .unwrap()
    );
    text.push_str(&String::from_utf8(question).unwrap());
    for field in [
        "agent",
        "version",
        "harness",
        "model",
        "title",
        "worktree",
        "unreadable",
    ]
    .into_iter()
    .chain(
        (!compact)
            .then_some([
                "source",
                "basis",
                "catalog",
                "branch",
                "base",
                "skipped",
                "unscanned",
                "symlink",
                "hook_harness",
                "event",
                "hook_source",
                "control",
                "executable",
                "argument",
            ])
            .into_iter()
            .flatten(),
    ) {
        let start = text.find(&format!("BEGIN_{field}")).unwrap();
        let end_marker = format!("END_{field}");
        let end = start + text[start..].find(&end_marker).unwrap() + end_marker.len();
        assert_eq!(&text[start..end], display_safe(&hostile(field)), "{field}");
        assert!(!text[start..end].contains('\x1b'), "{field}");
    }
    assert!(!text.contains("\nForged section"));
    for ch in INVISIBLE {
        assert!(!text.contains(*ch));
    }
    std::fs::write(path, text).unwrap();
}

#[test]
fn previews_contain_hostile_fields_and_preserve_plain_structure() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent_on("fixture", "1.0.0", "codex", "gpt-6-astra");
    repo.commit("fixture");
    let scratch = tempfile::tempdir().unwrap();
    let bin = common::fake_harnesses(scratch.path(), &["codex"], |_| scratch.path().join("args"));
    for kind in ["compact", "detailed"] {
        let run = |color: &str| {
            let rendered = tempfile::NamedTempFile::new().unwrap();
            let stdout = tempfile::NamedTempFile::new().unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "preview_output_fixture", "--nocapture"])
                .env("AHU_TEST_PREVIEW_OUTPUT", rendered.path())
                .env("AHU_TEST_PREVIEW_COLOR", color)
                .env("AHU_TEST_PREVIEW_KIND", kind)
                .env("AHU_TEST_PREVIEW_REPO", repo.path())
                .env("AHU_STATE_DIR", repo.state_path())
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .env("TERM", "xterm-256color")
                .env_remove("NO_COLOR")
                .stdout(stdout.reopen().unwrap())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                std::fs::read_to_string(stdout.path()).unwrap(),
                String::from_utf8_lossy(&output.stderr)
            );
            std::fs::read(rendered.path()).unwrap()
        };
        let plain = run("never");
        assert_eq!(run("auto"), plain, "{kind}");
        assert!(!plain.contains(&0x1b));
        let plain = String::from_utf8(plain).unwrap();
        for label in [
            "approvals",
            "Checkout changes",
            "1 capability limit(s); details: ahu inventory",
            "Confirmation code for this submission: a1b2c3",
        ] {
            assert!(
                plain.to_lowercase().contains(&label.to_lowercase()),
                "{kind}: {label}"
            );
        }
        assert!(plain.contains(if kind == "compact" {
            "  gaps       "
        } else {
            "Enforcement gaps\n  ! "
        }));
        assert!(plain.contains(ahu::hooks::NON_PROJECT_HOOK_WARNING));
        assert!(plain.find("BEGIN_harness").unwrap() < plain.find("Confirmation code").unwrap());
        assert!(plain.find("BEGIN_model").unwrap() < plain.find("Confirmation code").unwrap());
        assert!(plain.ends_with(
            "\nTo submit, type the confirmation code a1b2c3 shown above (anything else cancels): "
        ));
        let colored = String::from_utf8(run("always")).unwrap();
        for sgr in ["1;36", "36", "33", "35", "1;33", "1"] {
            assert!(
                colored.contains(&format!("\x1b[{sgr}m")),
                "{kind}: missing style {sgr}"
            );
        }
    }
}

#[test]
fn selector_output_fixture() {
    let Ok(path) = std::env::var("AHU_TEST_SELECTOR_OUTPUT") else {
        return;
    };
    let color = match std::env::var("AHU_TEST_SELECTOR_COLOR").unwrap().as_str() {
        "always" => ahu::style::ColorChoice::Always,
        "auto" => ahu::style::ColorChoice::Auto,
        "never" => ahu::style::ColorChoice::Never,
        _ => panic!("invalid fixture color"),
    };
    ahu::style::configure(Some(color));
    let repo = TestRepo::new();
    repo.add_agent_on("fixture", "1.0.0", "codex", "gpt-6-astra");
    let mut agents = ahu::agent::load_all(repo.path()).unwrap();
    // Exercise the renderer's backstop even for identities that the manifest
    // reader rejects, without weakening registration validation.
    let hostile = "BEGIN\x1b[2J\u{202e}\u{200b}END";
    agents[0].manifest.name = hostile.into();
    agents[0].manifest.description = format!("{hostile} {}", "界".repeat(80));
    let mut reader = std::io::Cursor::new("9\nunknown\n1\n");
    let mut output = Vec::new();
    let mut console = ahu::launcher::Console {
        input: &mut reader,
        output: &mut output,
        interactive: true,
    };
    assert_eq!(
        ahu::launcher::read_selector(&mut console, &agents).unwrap(),
        Some(hostile.into())
    );
    std::fs::write(path, output).unwrap();
}

#[test]
fn selector_styling_contains_hostile_fields_and_preserves_selection() {
    let render = |color: &str| {
        let rendered = tempfile::NamedTempFile::new().unwrap();
        let stdout = tempfile::NamedTempFile::new().unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "selector_output_fixture"])
            .env("AHU_TEST_SELECTOR_OUTPUT", rendered.path())
            .env("AHU_TEST_SELECTOR_COLOR", color)
            .env("TERM", "xterm-256color")
            .env_remove("NO_COLOR")
            .stdout(stdout.reopen().unwrap())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            std::fs::read_to_string(stdout.path()).unwrap()
        );
        std::fs::read_to_string(rendered.path()).unwrap()
    };
    let plain = render("never");
    assert_eq!(render("auto"), plain);
    let colored = render("always");
    let safe = display_safe("BEGIN\x1b[2J\u{202e}\u{200b}END");
    assert!(colored.contains(&format!("  1. \x1b[1;36m@{safe}\x1b[0m")));
    assert!(colored.contains("     \x1b[36mcodex / gpt-6-astra\x1b[0m\n"));
    assert!(colored.contains(&format!("     \x1b[2m{safe} ")));
    let mut stripped = colored;
    for sgr in [
        "\x1b[1m",
        "\x1b[1;36m",
        "\x1b[36m",
        "\x1b[2m",
        "\x1b[33m",
        "\x1b[0m",
    ] {
        stripped = stripped.replace(sgr, "");
    }
    assert_eq!(stripped, plain);
    assert!(!plain.contains('\x1b'));
    assert!(!plain.contains('\u{202e}'));
    assert!(!plain.contains('\u{200b}'));
    let description = plain
        .lines()
        .find(|line| line.starts_with("     BEGIN"))
        .unwrap();
    assert!(description.ends_with("..."));
    assert!(
        description
            .chars()
            .map(|c| if c.is_ascii() { 1 } else { 2 })
            .sum::<usize>()
            <= 80
    );
    assert!(plain.contains("range 1–1"));
    assert!(plain.contains(&format!("Valid agents: @{safe}")));
}

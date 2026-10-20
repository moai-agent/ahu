mod common;

/// Rank a model for every harness a coordinator shortcut can open, so the
/// argument-list assertions below see a configured model rather than the
/// unconfigured path.
fn rank_all_harnesses(repo: &common::TestRepo) {
    repo.write(
        ".agents/ahu/config.toml",
        &format!(
            "schema_version = 1\n\
             harness_preferences = [\"claude-code\", \"codex\", \"antigravity\", \"opencode\"]\n\
             model_selection = \"project-ranked\"\n\
             catalog_version = \"{}\"\n\
             \n[model_rankings]\n\
             \"claude-code\" = [\"claude-opus-5\", \"claude-sonnet-5\"]\n\
             \"codex\" = [\"gpt-6-astra\"]\n\
             \"antigravity\" = [\"gemini-3.1-pro-high\"]\n\
             \"opencode\" = [\"ollama/glm-5.3:cloud\"]\n\
",
            ahu::catalog::CATALOG_VERSION
        ),
    );
    // Committed, so the working tree stays clean for the assertions that prove
    // a coordinator shortcut writes nothing into the checkout.
    repo.commit("rank a model for every coordinator harness");
}

#[cfg(unix)]
#[test]
fn codex_shortcut_has_fixed_yolo_options() {
    assert_eq!(
        ahu::cli::parse(["codex"]).unwrap(),
        ahu::cli::Command::Codex
    );
    for flag in [
        "--sandbox",
        "--ask-for-approval",
        "--dangerously-bypass-approvals-and-sandbox",
    ] {
        assert!(ahu::cli::parse(["codex", flag]).is_err());
    }
}

#[test]
#[cfg(unix)]
fn codex_session_inherits_terminal_context_and_uses_local_state_without_cmux() {
    let repo = common::TestRepo::new();
    let scratch = tempfile::tempdir().unwrap();
    let bin = common::fake_harnesses(scratch.path(), &["codex"], |_| {
        scratch.path().join("unused")
    });
    std::fs::write(
        bin.join("codex"),
        r#"#!/bin/sh
printf '%s\n' "$@" > "$AHU_TEST_ARGS"
pwd > "$AHU_TEST_CWD"
printf '%s' "${AHU_STATE_DIR-unset}" > "$AHU_TEST_STATE"
"$AHU_BIN" tasks --output json > "$AHU_TEST_TASKS" || exit 99
exit 7
"#,
    )
    .unwrap();
    rank_all_harnesses(&repo);
    let nested = repo.path().join("subdirectory");
    std::fs::create_dir(&nested).unwrap();
    let output = common::ahu()
        .arg("codex")
        .env_remove("CMUX_WORKSPACE_ID")
        .current_dir(&nested)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("AHU_STATE_DIR", scratch.path().join("parent-state"))
        .env("AHU_TEST_ARGS", scratch.path().join("args"))
        .env("AHU_TEST_CWD", scratch.path().join("cwd"))
        .env("AHU_TEST_STATE", scratch.path().join("state"))
        .env("AHU_TEST_TASKS", scratch.path().join("tasks"))
        .env("AHU_RUNTIME_DIR", scratch.path().join("runtime"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(scratch.path().join("args")).unwrap(),
        "-m\ngpt-6-astra\n--dangerously-bypass-approvals-and-sandbox\n"
    );
    // The disclosed line is the whole argument list, model included.
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "Codex coordinator: -m gpt-6-astra --dangerously-bypass-approvals-and-sandbox\n"
    );
    let cwd = std::fs::read_to_string(scratch.path().join("cwd")).unwrap();
    assert_eq!(
        std::path::Path::new(cwd.trim()).canonicalize().unwrap(),
        nested.canonicalize().unwrap()
    );
    let state = std::fs::read_to_string(scratch.path().join("state")).unwrap();
    assert_eq!(
        std::path::Path::new(&state),
        scratch.path().join("parent-state")
    );
    assert!(output.stdout.is_empty());
    // The config fixture owns `.agents`; ahu itself registered nothing there.
    assert!(!repo.path().join(".agents/ahu/agents").exists());
    assert!(!repo.path().join(".worktrees").exists());
    assert!(!scratch.path().join("parent-state").exists());
    assert_eq!(common::git(repo.path(), &["status", "--porcelain"]), "");
}

#[test]
fn claude_shortcut_has_fixed_permission_bypass_and_rejects_overrides() {
    assert_eq!(
        ahu::cli::parse(["claude"]).unwrap(),
        ahu::cli::Command::Claude
    );
    for flag in [
        "--model",
        "--permission-mode",
        "--dangerously-skip-permissions",
    ] {
        assert!(ahu::cli::parse(["claude", flag]).is_err());
    }
}
#[test]
#[cfg(unix)]
fn claude_session_inherits_terminal_context_and_uses_local_state_without_cmux() {
    let repo = common::TestRepo::new();
    let scratch = tempfile::tempdir().unwrap();
    let bin = common::fake_harnesses(scratch.path(), &["claude"], |_| {
        scratch.path().join("unused")
    });
    std::fs::write(
        bin.join("claude"),
        r#"#!/bin/sh
printf '%s\n' "$@" > "$AHU_TEST_ARGS"
pwd > "$AHU_TEST_CWD"
printf '%s' "${AHU_STATE_DIR-unset}" > "$AHU_TEST_STATE"
"$AHU_BIN" tasks --output json > "$AHU_TEST_TASKS" || exit 99
exit 7
"#,
    )
    .unwrap();
    rank_all_harnesses(&repo);
    let nested = repo.path().join("subdirectory");
    std::fs::create_dir(&nested).unwrap();
    let output = common::ahu()
        .arg("claude")
        .env_remove("CMUX_WORKSPACE_ID")
        .current_dir(&nested)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("AHU_STATE_DIR", scratch.path().join("parent-state"))
        .env("AHU_TEST_ARGS", scratch.path().join("args"))
        .env("AHU_TEST_CWD", scratch.path().join("cwd"))
        .env("AHU_TEST_STATE", scratch.path().join("state"))
        .env("AHU_TEST_TASKS", scratch.path().join("tasks"))
        .env("AHU_RUNTIME_DIR", scratch.path().join("runtime"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(scratch.path().join("args")).unwrap(),
        "--model\nclaude-opus-5\n--dangerously-skip-permissions\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "Claude coordinator: --model claude-opus-5 --dangerously-skip-permissions\n"
    );
    let cwd = std::fs::read_to_string(scratch.path().join("cwd")).unwrap();
    assert_eq!(
        std::path::Path::new(cwd.trim()).canonicalize().unwrap(),
        nested.canonicalize().unwrap()
    );
    let state = std::fs::read_to_string(scratch.path().join("state")).unwrap();
    assert_eq!(
        std::path::Path::new(&state),
        scratch.path().join("parent-state")
    );
    assert!(output.stdout.is_empty());
    // The config fixture owns `.agents`; ahu itself registered nothing there.
    assert!(!repo.path().join(".agents/ahu/agents").exists());
    assert!(!repo.path().join(".worktrees").exists());
    assert!(!scratch.path().join("parent-state").exists());
    assert_eq!(common::git(repo.path(), &["status", "--porcelain"]), "");
}

#[test]
fn opencode_shortcut_owns_its_options_and_rejects_overrides() {
    assert_eq!(
        ahu::cli::parse(["opencode"]).unwrap(),
        ahu::cli::Command::OpenCode
    );
    // The shortcut owns `--model` and `--auto`; a caller cannot replace either,
    // and ahu still passes no `--pure` and no `--agent`.
    for flag in ["--model", "--auto", "--pure", "--agent"] {
        assert!(ahu::cli::parse(["opencode", flag]).is_err());
    }
}

#[test]
#[cfg(unix)]
fn opencode_session_inherits_terminal_context_and_uses_local_state_without_cmux() {
    let repo = common::TestRepo::new();
    let scratch = tempfile::tempdir().unwrap();
    let bin = common::fake_harnesses(scratch.path(), &["opencode"], |_| {
        scratch.path().join("unused")
    });
    std::fs::write(
        bin.join("opencode"),
        r#"#!/bin/sh
if [ "$1" = --help ]; then printf '%s\n' '  -m, --model  model' '      --prompt  prompt' '      --auto  auto-approve'; exit 0; fi
if [ "$1" = models ]; then echo 'ollama/glm-5.3:cloud'; exit 0; fi
printf '%s\n' "$@" > "$AHU_TEST_ARGS"
pwd > "$AHU_TEST_CWD"
printf '%s' "${AHU_STATE_DIR-unset}" > "$AHU_TEST_STATE"
"$AHU_BIN" tasks --output json > "$AHU_TEST_TASKS" || exit 99
exit 7
"#,
    )
    .unwrap();
    rank_all_harnesses(&repo);
    let nested = repo.path().join("subdirectory");
    std::fs::create_dir(&nested).unwrap();
    let output = common::ahu()
        .arg("opencode")
        .env_remove("CMUX_WORKSPACE_ID")
        .current_dir(&nested)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("AHU_STATE_DIR", scratch.path().join("parent-state"))
        .env("AHU_TEST_ARGS", scratch.path().join("args"))
        .env("AHU_TEST_CWD", scratch.path().join("cwd"))
        .env("AHU_TEST_STATE", scratch.path().join("state"))
        .env("AHU_TEST_TASKS", scratch.path().join("tasks"))
        .env("AHU_RUNTIME_DIR", scratch.path().join("runtime"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // `--auto` is OpenCode's only permission-widening flag, and the model is
    // passed provider-qualified, as OpenCode's --model requires.
    assert_eq!(
        std::fs::read_to_string(scratch.path().join("args")).unwrap(),
        "--model\nollama/glm-5.3:cloud\n--auto\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "OpenCode coordinator: --model ollama/glm-5.3:cloud --auto\n"
    );
    let cwd = std::fs::read_to_string(scratch.path().join("cwd")).unwrap();
    assert_eq!(
        std::path::Path::new(cwd.trim()).canonicalize().unwrap(),
        nested.canonicalize().unwrap()
    );
    let state = std::fs::read_to_string(scratch.path().join("state")).unwrap();
    assert_eq!(
        std::path::Path::new(&state),
        scratch.path().join("parent-state")
    );
    assert!(output.stdout.is_empty());
    // The config fixture owns `.agents`; ahu itself registered nothing there.
    assert!(!repo.path().join(".agents/ahu/agents").exists());
    assert!(!repo.path().join(".worktrees").exists());
    assert!(!scratch.path().join("parent-state").exists());
    assert_eq!(common::git(repo.path(), &["status", "--porcelain"]), "");
}

#[test]
fn antigravity_shortcut_uses_yolo_mode_and_rejects_overrides() {
    assert_eq!(
        ahu::cli::parse(["agy"]).unwrap(),
        ahu::cli::Command::Antigravity
    );
    // The shortcut owns the fixed YOLO flag; callers cannot replace its mode.
    for flag in [
        "--model",
        "--mode",
        "--dangerously-skip-permissions",
        "--agent",
    ] {
        assert!(ahu::cli::parse(["agy", flag]).is_err());
    }
}

#[test]
#[cfg(unix)]
fn antigravity_session_inherits_terminal_context_and_uses_local_state_without_cmux() {
    let repo = common::TestRepo::new();
    let scratch = tempfile::tempdir().unwrap();
    let bin = common::fake_harnesses(scratch.path(), &["agy"], |_| scratch.path().join("unused"));
    std::fs::write(
        bin.join("agy"),
        r#"#!/bin/sh
printf '%s\n' "$@" > "$AHU_TEST_ARGS"
pwd > "$AHU_TEST_CWD"
printf '%s' "${AHU_STATE_DIR-unset}" > "$AHU_TEST_STATE"
"$AHU_BIN" tasks --output json > "$AHU_TEST_TASKS" || exit 99
exit 7
"#,
    )
    .unwrap();
    rank_all_harnesses(&repo);
    let nested = repo.path().join("subdirectory");
    std::fs::create_dir(&nested).unwrap();
    let output = common::ahu()
        .arg("agy")
        .env_remove("CMUX_WORKSPACE_ID")
        .current_dir(&nested)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("AHU_STATE_DIR", scratch.path().join("parent-state"))
        .env("AHU_TEST_ARGS", scratch.path().join("args"))
        .env("AHU_TEST_CWD", scratch.path().join("cwd"))
        .env("AHU_TEST_STATE", scratch.path().join("state"))
        .env("AHU_TEST_TASKS", scratch.path().join("tasks"))
        .env("AHU_RUNTIME_DIR", scratch.path().join("runtime"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // YOLO mode is fixed by the coordinator shortcut, alongside the project's
    // top-ranked Antigravity model.
    assert_eq!(
        std::fs::read_to_string(scratch.path().join("args")).unwrap(),
        "--model\ngemini-3.1-pro-high\n--dangerously-skip-permissions\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "Antigravity CLI coordinator: --model gemini-3.1-pro-high \
         --dangerously-skip-permissions\n"
    );
    let cwd = std::fs::read_to_string(scratch.path().join("cwd")).unwrap();
    assert_eq!(
        std::path::Path::new(cwd.trim()).canonicalize().unwrap(),
        nested.canonicalize().unwrap()
    );
    let state = std::fs::read_to_string(scratch.path().join("state")).unwrap();
    assert_eq!(
        std::path::Path::new(&state),
        scratch.path().join("parent-state")
    );
    assert!(output.stdout.is_empty());
    // The config fixture owns `.agents`; ahu itself registered nothing there.
    assert!(!repo.path().join(".agents/ahu/agents").exists());
    assert!(!repo.path().join(".worktrees").exists());
    assert!(!scratch.path().join("parent-state").exists());
    assert_eq!(common::git(repo.path(), &["status", "--porcelain"]), "");
}

/// A harness with no ranked model gets no model flag and an explicit note.
///
/// The alternative — ahu picking a model the project never agreed to — is the
/// failure this guards: the shortcut launches on the harness's own default and
/// says so, rather than inventing an identifier.
#[test]
#[cfg(unix)]
fn coordinator_without_a_ranked_model_passes_no_model_flag_and_says_so() {
    let repo = common::TestRepo::new();
    let scratch = tempfile::tempdir().unwrap();
    let bin = common::fake_harnesses(scratch.path(), &["codex"], |_| {
        scratch.path().join("unused")
    });
    std::fs::write(
        bin.join("codex"),
        r#"#!/bin/sh
printf '%s\n' "$@" > "$AHU_TEST_ARGS"
exit 7
"#,
    )
    .unwrap();
    // Rankings exist, but none for Codex: the gap is per harness, not per project.
    repo.write(
        ".agents/ahu/config.toml",
        &format!(
            "schema_version = 1\n\
             harness_preferences = [\"claude-code\"]\n\
             model_selection = \"project-ranked\"\n\
             catalog_version = \"{}\"\n\
             \n[model_rankings]\n\
             \"claude-code\" = [\"claude-opus-5\"]\n",
            ahu::catalog::CATALOG_VERSION
        ),
    );
    let output = common::ahu()
        .arg("codex")
        .env_remove("CMUX_WORKSPACE_ID")
        .current_dir(repo.path())
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("AHU_STATE_DIR", scratch.path().join("parent-state"))
        .env("AHU_TEST_ARGS", scratch.path().join("args"))
        .env("AHU_RUNTIME_DIR", scratch.path().join("runtime"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(scratch.path().join("args")).unwrap(),
        "--dangerously-bypass-approvals-and-sandbox\n"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no model is ranked for codex"),
        "expected a harness-default note, got {stderr:?}"
    );
    assert!(
        stderr.contains("Codex coordinator: --dangerously-bypass-approvals-and-sandbox\n"),
        "{stderr:?}"
    );
    assert!(!stderr.contains("unconfigured"), "{stderr:?}");
}

/// Coordinator shortcuts and headless agents name the model with one mapping.
///
/// A second copy of this mapping is how a coordinator comes to pass `--model` to
/// a CLI whose adapter passes `-m`, so the flags are asserted against the argv
/// the adapters actually build.
#[test]
fn coordinator_model_flags_match_the_headless_adapters() {
    let worktree = tempfile::tempdir().unwrap();
    for (harness, model, flag) in [
        ("claude-code", "claude-opus-5", "--model"),
        ("codex", "gpt-6-astra", "-m"),
        ("antigravity", "gemini-3.1-pro-high", "--model"),
        ("opencode", "ollama/glm-5.3:cloud", "--model"),
    ] {
        assert_eq!(
            ahu::harness::model_args(harness, model).unwrap(),
            vec![flag.to_string(), model.to_string()],
            "{harness}"
        );
        let adapter = ahu::harness::adapter_for(harness).unwrap();
        let built = adapter
            .launch_command(&ahu::harness::LaunchRequest {
                model,
                prompt: "fixture prompt",
                cwd: worktree.path(),
                permissions: ahu::agent::Permissions::Prompt,
            })
            .unwrap();
        assert_eq!(
            &built.args[..2],
            &[flag.to_string(), model.to_string()],
            "{harness} adapter"
        );
    }
    // An unranked or malformed identifier is refused, not smuggled onto a
    // command line as an option.
    assert!(ahu::harness::model_args("codex", "").is_err());
    assert!(ahu::harness::model_args("claude-code", "--help").is_err());
    assert!(ahu::harness::model_args("opencode", "glm-5.3").is_err());
}

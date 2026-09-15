mod common;

#[test]
fn codex_shortcut_has_fixed_approval_and_sandbox_options() {
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
printf '%s' "$AHU_STATE_DIR" > "$AHU_TEST_STATE"
exit 7
"#,
    )
    .unwrap();
    let nested = repo.path().join("subdirectory");
    std::fs::create_dir(&nested).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
        .arg("codex")
        .current_dir(&nested)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("AHU_STATE_DIR", scratch.path().join("parent-state"))
        .env("AHU_CMUX_BIN", scratch.path().join("missing-cmux"))
        .env("AHU_TEST_ARGS", scratch.path().join("args"))
        .env("AHU_TEST_CWD", scratch.path().join("cwd"))
        .env("AHU_TEST_STATE", scratch.path().join("state"))
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
        "--sandbox\nworkspace-write\n--ask-for-approval\non-request\n"
    );
    let cwd = std::fs::read_to_string(scratch.path().join("cwd")).unwrap();
    assert_eq!(
        std::path::Path::new(cwd.trim()).canonicalize().unwrap(),
        nested.canonicalize().unwrap()
    );
    let state = std::fs::read_to_string(scratch.path().join("state")).unwrap();
    assert_eq!(
        std::path::Path::new(&state).canonicalize().unwrap(),
        repo.path().join(".ahu/state").canonicalize().unwrap()
    );
    assert!(output.stdout.is_empty());
    assert!(!repo.path().join(".agents").exists());
    assert!(!repo.path().join(".worktrees").exists());
    assert!(!scratch.path().join("parent-state").exists());
    assert_eq!(common::git(repo.path(), &["status", "--porcelain"]), "");
}

#[test]
fn claude_shortcut_preserves_native_defaults_and_rejects_overrides() {
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
printf '%s' "$AHU_STATE_DIR" > "$AHU_TEST_STATE"
exit 7
"#,
    )
    .unwrap();
    let nested = repo.path().join("subdirectory");
    std::fs::create_dir(&nested).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
        .arg("claude")
        .current_dir(&nested)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("AHU_STATE_DIR", scratch.path().join("parent-state"))
        .env("AHU_CMUX_BIN", scratch.path().join("missing-cmux"))
        .env("AHU_TEST_ARGS", scratch.path().join("args"))
        .env("AHU_TEST_CWD", scratch.path().join("cwd"))
        .env("AHU_TEST_STATE", scratch.path().join("state"))
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
        "\n"
    );
    let cwd = std::fs::read_to_string(scratch.path().join("cwd")).unwrap();
    assert_eq!(
        std::path::Path::new(cwd.trim()).canonicalize().unwrap(),
        nested.canonicalize().unwrap()
    );
    let state = std::fs::read_to_string(scratch.path().join("state")).unwrap();
    assert_eq!(
        std::path::Path::new(&state).canonicalize().unwrap(),
        repo.path().join(".ahu/state").canonicalize().unwrap()
    );
    assert!(output.stdout.is_empty());
    assert!(!repo.path().join(".agents").exists());
    assert!(!repo.path().join(".worktrees").exists());
    assert!(!scratch.path().join("parent-state").exists());
    assert_eq!(common::git(repo.path(), &["status", "--porcelain"]), "");
}

#[test]
fn opencode_shortcut_preserves_native_defaults_and_rejects_overrides() {
    assert_eq!(
        ahu::cli::parse(["opencode"]).unwrap(),
        ahu::cli::Command::OpenCode
    );
    // `--auto` widens to every permission OpenCode does not explicitly deny and
    // `--pure` disables the user's own plugins. Neither is ahu's call to make
    // on a coordinating session, so neither is accepted here.
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
printf '%s\n' "$@" > "$AHU_TEST_ARGS"
pwd > "$AHU_TEST_CWD"
printf '%s' "$AHU_STATE_DIR" > "$AHU_TEST_STATE"
exit 7
"#,
    )
    .unwrap();
    let nested = repo.path().join("subdirectory");
    std::fs::create_dir(&nested).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ahu"))
        .arg("opencode")
        .current_dir(&nested)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("AHU_STATE_DIR", scratch.path().join("parent-state"))
        .env("AHU_CMUX_BIN", scratch.path().join("missing-cmux"))
        .env("AHU_TEST_ARGS", scratch.path().join("args"))
        .env("AHU_TEST_CWD", scratch.path().join("cwd"))
        .env("AHU_TEST_STATE", scratch.path().join("state"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // No arguments at all: the session is whatever the user's own OpenCode
    // configuration makes it.
    assert_eq!(
        std::fs::read_to_string(scratch.path().join("args")).unwrap(),
        "\n"
    );
    let cwd = std::fs::read_to_string(scratch.path().join("cwd")).unwrap();
    assert_eq!(
        std::path::Path::new(cwd.trim()).canonicalize().unwrap(),
        nested.canonicalize().unwrap()
    );
    let state = std::fs::read_to_string(scratch.path().join("state")).unwrap();
    assert_eq!(
        std::path::Path::new(&state).canonicalize().unwrap(),
        repo.path().join(".ahu/state").canonicalize().unwrap()
    );
    assert!(output.stdout.is_empty());
    assert!(!repo.path().join(".agents").exists());
    assert!(!repo.path().join(".worktrees").exists());
    assert!(!scratch.path().join("parent-state").exists());
    assert_eq!(common::git(repo.path(), &["status", "--porcelain"]), "");
}

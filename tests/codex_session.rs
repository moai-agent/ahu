//! Direct sessions consume installed runtime plans without terminal-provider assumptions.
mod common;

#[test]
fn direct_selectors_are_generic_and_reject_native_overrides() {
    for selector in ["codex", "claude", "agy", "opencode", "sixth"] {
        assert_eq!(
            ahu::cli::parse([selector]).unwrap(),
            ahu::cli::Command::Harness {
                selector: selector.into()
            }
        );
        for flag in ["--model", "--sandbox", "--permission-mode"] {
            assert!(ahu::cli::parse([selector, flag]).is_err());
        }
    }
}

#[test]
fn direct_sessions_preserve_terminal_directory_environment_and_exit_status() {
    for (harness, program) in [
        ("codex", "codex"),
        ("claude-code", "claude"),
        ("antigravity", "agy"),
        ("opencode", "opencode"),
        ("sixth", "sixth-native"),
    ] {
        let repo = common::TestRepo::new();
        repo.install_runtime(harness);
        repo.write(".agents/ahu/config.toml", &format!("schema_version = 1\nharness_preferences = [{harness:?}]\nmodel_selection = \"project-ranked\"\n[model_rankings]\n{harness:?} = [\"fixture/exact\"]\n"));
        repo.commit("rank exact model");
        let scratch = tempfile::tempdir().unwrap();
        let bin = common::fake_harnesses(scratch.path(), &[program], |_| {
            scratch.path().join("unused")
        });
        std::fs::write(
            bin.join(program),
            r#"#!/bin/sh
printf '%s\n' "$@" > "$AHU_TEST_ARGS"
pwd > "$AHU_TEST_CWD"
printf '%s' "${AHU_STATE_DIR-unset}" > "$AHU_TEST_STATE"
"$AHU_BIN" tasks --output json > "$AHU_TEST_TASKS" || exit 99
exit 7
"#,
        )
        .unwrap();
        let nested = repo.path().join("subdirectory");
        std::fs::create_dir(&nested).unwrap();
        let output = common::ahu()
            .args(["harness", harness])
            .current_dir(&nested)
            .env("AHU_INTERACTIVE_PLUGIN", "terminal")
            .env_remove("CMUX_WORKSPACE_ID")
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("AHU_STATE_DIR", scratch.path().join("state-root"))
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
            "{harness}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(scratch.path().join("args")).unwrap(),
            "--model\nfixture/exact\n--auto\n"
        );
        let cwd = std::fs::read_to_string(scratch.path().join("cwd")).unwrap();
        assert_eq!(
            std::path::Path::new(cwd.trim()).canonicalize().unwrap(),
            nested.canonicalize().unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(scratch.path().join("state")).unwrap(),
            scratch.path().join("state-root").to_str().unwrap()
        );
        let tasks: serde_json::Value =
            serde_json::from_slice(&std::fs::read(scratch.path().join("tasks")).unwrap()).unwrap();
        assert_eq!(tasks["tasks"], serde_json::json!([]));
        assert!(!repo.path().join(".worktrees").exists());
        assert!(output.stdout.is_empty());
        assert_eq!(common::git(repo.path(), &["status", "--porcelain"]), "");
    }
}

#[test]
fn direct_session_without_ranked_model_refuses_before_native_execution() {
    let repo = common::TestRepo::new();
    repo.install_runtime("codex");
    let output = common::ahu()
        .args(["harness", "codex"])
        .current_dir(repo.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("exact project-ranked model"));
    assert!(!repo.path().join(".worktrees").exists());
}

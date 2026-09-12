mod common;

use common::{HOSTILE_PROMPT, TestRepo};
use std::process::Command;

fn launch(repo: &TestRepo, name: &str, dry_run: bool) -> std::process::Output {
    let temp = repo.state_path();
    let bin = common::fake_harness(temp, &temp.join("argv"));
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ahu"));
    cmd.current_dir(repo.path())
        .args(["launch", name, "--prompt-file"])
        .arg(repo.path().join("assignment.txt"))
        .env("AHU_STATE_DIR", temp)
        .env("AHU_CMUX_BIN", temp.join("missing-cmux"))
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        );
    if dry_run {
        cmd.arg("--dry-run");
    }
    cmd.output().unwrap()
}

#[test]
fn launch_parser_requires_an_explicit_registered_identity_and_prompt_file() {
    for args in [
        vec!["launch"],
        vec!["launch", "@"],
        vec!["launch", "../agent", "--prompt-file", "p"],
        vec!["launch", "@sable"],
        vec!["launch", "@sable", "--prompt-file", "p", "--model", "other"],
    ] {
        assert!(ahu::cli::parse(args).is_err());
    }
    assert!(
        matches!(ahu::cli::parse(["launch", "@sable", "--prompt-file", "task.txt", "--dry-run"]).unwrap(),
        ahu::cli::Command::Launch { agent, dry_run: true, .. } if agent == "sable")
    );
}

#[test]
fn dry_run_resolves_the_named_agent_without_reading_confirmation_or_launching() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("sable", "1.0.0", "claude-sonnet-5");
    repo.write("assignment.txt", HOSTILE_PROMPT);
    repo.commit("fixture");
    let output = launch(&repo, "@sable", true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("sable@1.0.0"), "{text}");
    assert!(text.contains("claude-sonnet-5"));
    assert!(text.contains("--disallowedTools"));
    assert!(text.contains("Dry run"));
    assert!(!text.contains(HOSTILE_PROMPT));
    assert_eq!(common::git(repo.path(), &["branch", "--list", "ahu/*"]), "");
}

#[test]
fn failed_assignment_never_falls_back_to_the_coordinator_or_leaves_a_branch() {
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent("sable", "1.0.0", "claude-sonnet-5");
    repo.write("assignment.txt", "review security");
    repo.commit("fixture");
    let missing = launch(&repo, "@unknown", false);
    assert!(!missing.status.success());
    let unavailable = launch(&repo, "@sable", false);
    assert!(!unavailable.status.success());
    assert!(String::from_utf8_lossy(&unavailable.stderr).contains("cmux"));
    assert_eq!(common::git(repo.path(), &["branch", "--list", "ahu/*"]), "");
}

#[test]
fn every_harness_receives_delegation_guidance_without_replacing_its_identity() {
    for (harness, model) in [
        ("claude-code", "claude-opus-5"),
        ("codex", "gpt-6-astra"),
        ("antigravity", "gemini-3.1-pro-high"),
    ] {
        let adapter = ahu::harness::adapter_for(harness).unwrap();
        let raw = adapter
            .launch_command(&ahu::harness::LaunchRequest {
                model,
                native_agent: Some("reviewer"),
                prompt: HOSTILE_PROMPT,
                cwd: std::path::Path::new("/tmp"),
                permissions: Default::default(),
            })
            .unwrap();
        let command = ahu::orchestration::configure(raw.clone()).unwrap();
        assert_eq!(command.program, raw.program);
        assert!(command.args.iter().any(|a| a == model));
        let prompt = &command.args[command.prompt_arg.unwrap()];
        assert!(prompt.ends_with(HOSTILE_PROMPT));
        assert!(
            !command
                .redacted()
                .args
                .iter()
                .any(|a| a.contains(HOSTILE_PROMPT))
        );
        assert!(
            command
                .args
                .iter()
                .any(|a| a.contains(ahu::orchestration::INSTRUCTIONS))
        );
        if harness == "claude-code" {
            assert_eq!(prompt, HOSTILE_PROMPT);
            assert!(
                command
                    .args
                    .windows(2)
                    .any(|p| p == ["--disallowedTools", "Agent,Task,TeamCreate"])
            );
        }
    }
}

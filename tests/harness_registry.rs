//! Installed invented provider through core CLI, direct launch and setup UI.
mod common;
use common::TestRepo;
use std::io::Cursor;

fn configured() -> TestRepo {
    let repo = TestRepo::new();
    repo.install_runtime("sixth");
    let config = ahu::config::ProjectConfig {
        schema_version: ahu::config::SUPPORTED_SCHEMA_VERSION,
        harness_preferences: vec!["sixth".into()],
        model_selection: "project-ranked".into(),
        harness_version_pins: Default::default(),
        model_rankings: [("sixth".into(), vec!["sixth/exact".into()])].into(),
        knowledge: Default::default(),
        headless: Default::default(),
        telemetry: Default::default(),
    };
    repo.write(
        ahu::config::CONFIG_RELATIVE_PATH,
        &ahu::config::try_render(&config).unwrap(),
    );
    repo.commit("synthetic project policy");
    repo
}

#[test]
fn invented_provider_cli_onboarding_parses_native_source_without_core_registry() {
    let repo = configured();
    repo.write(
        ".sixth/agents/reviewer.md",
        "---\nmodel: sixth/exact\ndescription: fixture\n---\nReview synthetic inputs.\n",
    );
    let output = common::ahu()
        .arg("--repo")
        .arg(repo.path())
        .arg("onboard")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("reviewer") && text.contains("sixth"),
        "{text}"
    );
    let candidates = ahu::onboard::preview(repo.path()).unwrap();
    let candidate = &candidates[0];
    assert_eq!(candidate.harness, "sixth");
    let manifest = ahu::onboard::proposed_manifest(candidate, "sixth/exact", "1.0.0");
    repo.write(".agents/ahu/agents/reviewer.md", &manifest);
    let agent = ahu::agent::find(repo.path(), "reviewer").unwrap();
    assert_eq!(agent.instructions, "Review synthetic inputs.\n");
    repo.write(
        ".sixth/agents/reviewer.md",
        "---\nmodel: sixth/different\n---\nReview synthetic inputs.\n",
    );
    assert!(ahu::agent::find(repo.path(), "reviewer").is_err());
}

#[test]
fn invented_provider_direct_cli_executes_exact_ranked_model() {
    let repo = configured();
    let fixtures = tempfile::tempdir().unwrap();
    let record = fixtures.path().join("argv");
    let bin = common::fake_harnesses(fixtures.path(), &["sixth-native"], |_| record.clone());
    let output = common::ahu()
        .arg("--repo")
        .arg(repo.path())
        .args(["harness", "sixth"])
        .env("HOME", fixtures.path())
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("AHU_INTERACTIVE_PLUGIN", "terminal")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let args = std::fs::read_to_string(record).unwrap();
    assert_eq!(
        args.lines().collect::<Vec<_>>(),
        ["--model", "sixth/exact", "--auto"]
    );
}

#[test]
fn invented_provider_setup_publishes_reviewed_proposal_and_cancels_without_writes() {
    const CASE: &str =
        "invented_provider_setup_publishes_reviewed_proposal_and_cancels_without_writes";
    if !common::is_child_case(CASE) {
        let fixtures = tempfile::tempdir().unwrap();
        let bin = common::fake_harnesses(fixtures.path(), &["sixth-native"], |_| {
            fixtures.path().join("argv")
        });
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ahu"), bin.join("ahu")).unwrap();
        let output = common::run_child_case(CASE, |command| {
            command
                .env("HOME", fixtures.path())
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()));
        });
        common::assert_child_passed(CASE, &output);
        return;
    }
    let repo = configured();
    let discovered = ahu::git::discover(repo.path()).unwrap();
    for (answer, code) in [("n\n", 1), ("y\n", 0)] {
        let mut input = Cursor::new(answer);
        let mut output = Vec::new();
        let result = ahu::setup::run(
            &mut ahu::launcher::Console {
                input: &mut input,
                output: &mut output,
                interactive: true,
            },
            &discovered,
        )
        .unwrap();
        assert_eq!(result, code, "{}", String::from_utf8_lossy(&output));
        assert_eq!(
            repo.path().join(".fixture/settings.json").exists(),
            code == 0
        );
    }
    assert_eq!(
        repo.read(".fixture/settings.json"),
        "{\"configured\":true}\n"
    );
    let agent = ahu::agent::find(repo.path(), "dev-sixth").unwrap();
    assert_eq!(agent.manifest.harness, "sixth");
    assert_eq!(agent.manifest.model, "sixth/exact");
    assert!(common::git(repo.path(), &["diff", "--cached", "--name-only"]).is_empty());
}

mod common;

// Provider-neutral disclosure and unknown-provider behavior.
use ahu::native_admission::{self, HeadlessPolicy, Status};
#[test]
fn unknown_provider_never_becomes_conformant_or_admitted() {
    let temp = tempfile::tempdir().unwrap();
    let status = native_admission::inspect(temp.path(), "invented-provider");
    assert!(!status.headless.allowed);
    assert!(!status.gaps.is_empty());
    assert_eq!(status.conformance, "untested");
}
#[test]
fn native_strings_cannot_forge_terminal_status_lines() {
    let status = Status {
        harness: "invented\nFORGED".into(),
        native_agent: None,
        profile: None,
        cli_version: None,
        version_checked: false,
        components: vec![],
        gaps: vec!["gap\x1b[2J".into()],
        conformance: "untested".into(),
        headless: HeadlessPolicy {
            allowed: false,
            reasons: vec!["reason\nFORGED".into()],
            evidence_digest: "a".repeat(64),
        },
        next_action: "review".into(),
    };
    let text = native_admission::render(&status);
    assert!(!text.contains('\x1b'));
    assert!(!text.lines().any(|line| line.starts_with("FORGED")));
}

#[test]
fn installed_runtime_validates_headless_and_interactive_launch_modes() {
    if !common::in_harness_fixture(
        "installed_runtime_validates_headless_and_interactive_launch_modes",
    ) {
        return;
    }
    let repo = common::TestRepo::new();
    repo.install_runtime("kiro");
    repo.commit("synthetic installed runtime");
    let executable = ahu::selection::resolve_executable("kiro-cli").unwrap();
    for mode in ["headless", "interactive", "direct"] {
        let status = native_admission::validate_launch(
            repo.path(),
            "kiro",
            "synthetic/exact",
            ahu::agent::Permissions::Prompt,
            mode,
            "1.2.3",
            std::path::Path::new(&executable),
        )
        .unwrap();
        assert!(status.headless.allowed);
    }
    assert!(
        native_admission::validate_launch(
            repo.path(),
            "kiro",
            "synthetic/exact",
            ahu::agent::Permissions::Prompt,
            "unknown",
            "1.2.3",
            std::path::Path::new(&executable)
        )
        .is_err()
    );
}

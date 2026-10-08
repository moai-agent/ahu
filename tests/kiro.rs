mod common;

#[test]
fn shortcut_and_capabilities_are_explicit() {
    assert_eq!(ahu::cli::parse(["kiro"]).unwrap(), ahu::cli::Command::Kiro);
    assert!(ahu::cli::parse(["kiro", "--model", "auto"]).is_err());
    let entry = ahu::catalog::harness("kiro").unwrap();
    assert!(entry.supports(ahu::catalog::Feature::InteractiveLaunch));
    assert!(!entry.supports(ahu::catalog::Feature::HeadlessLaunch));
    assert!(!entry.supports(ahu::catalog::Feature::AcceptEditsApprovals));
    assert!(!entry.supports(ahu::catalog::Feature::HookInventory));
    let repo = tempfile::tempdir().unwrap();
    let status = ahu::cmux::integration::inspect_in(
        repo.path(),
        "kiro",
        &ahu::cmux::integration::Locations::default(),
    )
    .with_version(Some("kiro-cli 2.28.0"));
    assert!(!status.headless.allowed);
    assert!(
        status
            .next_action
            .contains("no validated ahu headless adapter")
    );
    assert!(!status.next_action.contains("parseable"));
}

#[test]
#[cfg(unix)]
fn coordinator_checks_exact_model_before_starting_and_withholds_native_errors() {
    let repo = common::TestRepo::new();
    repo.write(".agents/ahu/config.toml", &format!(
        "schema_version = 1\nharness_preferences = [\"kiro\"]\nmodel_selection = \"project-ranked\"\ncatalog_version = \"{}\"\n[model_rankings]\nkiro = [\"claude-sonnet-4.5\"]\n",
        ahu::catalog::CATALOG_VERSION
    ));
    repo.commit("Kiro policy");
    let scratch = tempfile::tempdir().unwrap();
    let bin = common::fake_harnesses(scratch.path(), &["kiro-cli"], |_| {
        scratch.path().join("unused")
    });
    std::fs::write(
        bin.join("kiro-cli"),
        r#"#!/bin/sh
if [ "$1" = whoami ]; then
  test "$AHU_TEST_KIRO" != signed-out || exit 1
  printf '%s\n' '{"accountType":"SocialGitHub","email":"fixture@example.invalid"}'
  exit 0
fi
if [ "$3" = --list-models ]; then
  case "$AHU_TEST_KIRO" in
    malformed) printf 'private-marker';;
    unavailable) printf '%s\n' '{"models":[{"model_id":"auto"}]}' ;;
    *) printf '%s\n' '{"models":[{"model_id":"claude-sonnet-4.5"}]}' ;;
  esac
  exit 0
fi
printf '%s\n' "$@" > "$AHU_TEST_ARGS"
exit 7
"#,
    )
    .unwrap();
    for mode in ["available", "unavailable", "malformed", "signed-out"] {
        let args_path = scratch.path().join(mode);
        let result = common::ahu()
            .arg("kiro")
            .current_dir(repo.path())
            .env_remove("CMUX_WORKSPACE_ID")
            .env("AHU_CMUX_BIN", scratch.path().join("missing-cmux"))
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("AHU_TEST_KIRO", mode)
            .env("AHU_TEST_ARGS", &args_path)
            .output()
            .unwrap();
        let diagnostic = String::from_utf8_lossy(&result.stderr);
        assert!(!diagnostic.contains("private-marker"));
        if mode == "available" {
            assert_eq!(result.status.code(), Some(7), "{diagnostic}");
            assert_eq!(
                std::fs::read_to_string(args_path).unwrap(),
                "chat\n--v3\n--model\nclaude-sonnet-4.5\n--trust-all-tools\n"
            );
        } else {
            assert!(!result.status.success());
            assert!(
                !args_path.exists(),
                "must refuse before starting the interactive session"
            );
        }
    }
    assert_eq!(common::git(repo.path(), &["status", "--porcelain"]), "");
}

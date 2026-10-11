//! Plugin inventory and activation are tested in isolated processes.
mod common;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn fixture_command(root: &std::path::Path) -> Command {
    let executable = root.join("cmux");
    std::fs::write(
        &executable,
        "#!/bin/sh\ntouch \"$PLUGIN_EXECUTED\"\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut command = common::ahu();
    command
        .current_dir(root)
        .env("AHU_CMUX_BIN", executable)
        .env("PLUGIN_EXECUTED", root.join("executed"))
        .env_remove("AHU_INTERACTIVE_PLUGIN");
    command
}

#[test]
fn inventory_never_executes_plugins_or_requires_a_repository() {
    let root = tempfile::tempdir().unwrap();
    for selection in [None, Some("none"), Some("cmux")] {
        let mut command = fixture_command(root.path());
        if let Some(value) = selection {
            command.env("AHU_INTERACTIVE_PLUGIN", value);
        }
        let output = command
            .args(["plugins", "--output", "json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let plugin = &value["plugins"][0];
        assert_eq!(plugin["compiled"], cfg!(feature = "cmux-plugin"));
        assert_eq!(
            plugin["selected"],
            selection.map_or(cfg!(feature = "cmux-plugin"), |v| v == "cmux")
        );
        assert_eq!(plugin["host_api"], 1);
        assert!(!root.path().join("executed").exists());
        assert!(!root.path().join(".ahu").exists());
    }
}

#[test]
fn unknown_or_empty_selection_fails_without_execution() {
    let root = tempfile::tempdir().unwrap();
    for selection in ["", "other", "CMUX", " cmux"] {
        let output = fixture_command(root.path())
            .env("AHU_INTERACTIVE_PLUGIN", selection)
            .arg("plugins")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("must be cmux or none"));
        assert!(!root.path().join("executed").exists());
    }
}

#[test]
fn activation_and_coordinator_selection() {
    if common::is_child_case("activation_and_coordinator_selection") {
        let case = std::env::var("PLUGIN_CASE").unwrap();
        let result = ahu::interactive::discover_for_coordinator();
        match case.as_str() {
            "disabled" | "outside" => assert!(result.unwrap().is_none()),
            "default-inside" if !cfg!(feature = "cmux-plugin") => {
                assert!(result.unwrap().is_none())
            }
            _ if !cfg!(feature = "cmux-plugin") => assert!(
                result
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("--features cmux-plugin")
            ),
            _ => assert!(result.unwrap().is_some()),
        }
        let activated = std::path::Path::new(&std::env::var("PLUGIN_EXECUTED").unwrap()).exists();
        assert_eq!(
            activated,
            cfg!(feature = "cmux-plugin")
                && matches!(case.as_str(), "default-inside" | "explicit-inside")
        );
        if case == "disabled" || !cfg!(feature = "cmux-plugin") {
            let error = ahu::interactive::discover().err().unwrap();
            assert_eq!(error.kind(), ahu::util::ErrorKind::Prerequisite);
            assert!(!std::path::Path::new(&std::env::var("PLUGIN_EXECUTED").unwrap()).exists());
        }
        return;
    }
    for case in ["disabled", "outside", "default-inside", "explicit-inside"] {
        let root = tempfile::tempdir().unwrap();
        let _ = fixture_command(root.path());
        let output = common::run_child_case("activation_and_coordinator_selection", |command| {
            command
                .env("PLUGIN_CASE", case)
                .env("AHU_CMUX_BIN", root.path().join("cmux"))
                .env("PLUGIN_EXECUTED", root.path().join("executed"))
                .env_remove("AHU_INTERACTIVE_PLUGIN")
                .env_remove("CMUX_WORKSPACE_ID");
            if case != "outside" {
                command.env("CMUX_WORKSPACE_ID", "inherited");
            }
            if case == "disabled" {
                command.env("AHU_INTERACTIVE_PLUGIN", "none");
            }
            if case == "explicit-inside" {
                command.env("AHU_INTERACTIVE_PLUGIN", "cmux");
            }
        });
        assert!(output.status.success(), "{case}: {output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    }
}

#[test]
fn plugin_cli_rejects_mutation_and_extra_arguments() {
    for args in [
        vec!["plugins", "install"],
        vec!["plugins", "--output", "yaml"],
        vec!["plugins", "--output", "json", "extra"],
    ] {
        let error = ahu::cli::parse(args).unwrap_err();
        assert_eq!(error.kind(), ahu::util::ErrorKind::Usage);
    }
    assert!(
        ahu::cli::help_for(Some("plugins"))
            .unwrap()
            .contains("without executing")
    );
}

#[test]
fn disabled_plugin_keeps_actual_coordinators_in_the_current_terminal() {
    for (shortcut, program) in [
        ("codex", "codex"),
        ("claude", "claude"),
        ("agy", "agy"),
        ("opencode", "opencode"),
        ("kiro", "kiro-cli"),
    ] {
        let repo = common::TestRepo::new();
        let scratch = tempfile::tempdir().unwrap();
        let bin = common::fake_harnesses(scratch.path(), &[program], |_| {
            scratch.path().join("unused")
        });
        std::fs::write(
            bin.join(program),
            "#!/bin/sh\ntouch \"$COORDINATOR_STARTED\"\nexit 7\n",
        )
        .unwrap();
        let output = fixture_command(scratch.path())
            .current_dir(repo.path())
            .arg(shortcut)
            .env("AHU_INTERACTIVE_PLUGIN", "none")
            .env("CMUX_WORKSPACE_ID", "caller")
            .env("COORDINATOR_STARTED", scratch.path().join("started"))
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(7), "{shortcut}: {output:?}");
        assert!(scratch.path().join("started").exists());
        assert!(!scratch.path().join("executed").exists());
    }
}

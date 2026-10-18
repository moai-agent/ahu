//! Native admission must happen before interactive task/session creation.
mod common;
use common::TestRepo;
use std::os::unix::fs::PermissionsExt;

#[test]
fn interactive_and_coordinator_refuse_an_unlisted_opencode_model() {
    let repo = TestRepo::new();
    repo.write(".agents/ahu/config.toml", &format!(
        "schema_version = 1\nharness_preferences = [\"opencode\"]\nmodel_selection = \"project-ranked\"\ncatalog_version = \"{}\"\n[model_rankings]\nopencode = [\"ollama/glm-5.3:cloud\"]\n", ahu::catalog::CATALOG_VERSION));
    repo.add_agent_on("smoke", "1.0.0", "opencode", "ollama/glm-5.3:cloud");
    repo.commit("synthetic native admission");
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let exe = bin.join("opencode");
    // The listing command takes no positional provider and answers with
    // provider-qualified identifiers; none of them is the requested model.
    std::fs::write(&exe, format!("#!/bin/sh\nif [ \"$1\" = --help ]; then printf '%s\\n' {}; exit 0; fi\nif [ \"$1\" = --version ]; then echo 1.18.34; exit 0; fi\nif [ \"$1\" = models ] && [ $# -eq 1 ]; then printf 'ollama/other\\nopencode/big-pickle\\n'; exit 0; fi\nexit 91\n", common::shell_quoted(include_str!("fixtures/opencode/1.x-help.txt")))).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    for args in [
        vec!["@smoke", "--prompt", "inspect synthetic data"],
        vec!["opencode"],
    ] {
        let result = common::ahu()
            .args(args)
            .current_dir(repo.path())
            .env("HOME", home.path())
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("AHU_CMUX_BIN", home.path().join("no-cmux"))
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success());
        assert!(
            stderr.contains("refuses a possible model fallback"),
            "{stderr}"
        );
        assert_eq!(
            common::git(repo.path(), &["worktree", "list", "--porcelain"])
                .lines()
                .filter(|l| l.starts_with("worktree "))
                .count(),
            1
        );
    }
}

/// Help fixtures are bounded excerpts of the native option declarations:
/// 1.x preserves the previously verified 1.18 shape (also checked on 1.19.0);
/// 2.0.32 has --model only on run/mini, never the default command.
#[test]
fn opencode_v2_uses_mini_for_persistent_interactive_launches() {
    let repo = TestRepo::new();
    repo.write(".agents/ahu/config.toml", &format!(
        "schema_version = 1\nharness_preferences = [\"opencode\"]\nmodel_selection = \"project-ranked\"\ncatalog_version = \"{}\"\n[model_rankings]\nopencode = [\"ollama/glm-5.3:cloud\"]\n", ahu::catalog::CATALOG_VERSION));
    repo.add_agent_on("smoke", "1.0.0", "opencode", "ollama/glm-5.3:cloud");
    repo.add_agent_on("auto", "1.0.0", "opencode", "ollama/glm-5.3:cloud");
    let auto_path = ".agents/ahu/agents/auto.md";
    repo.write(
        auto_path,
        &repo
            .read(auto_path)
            .replace("permissions: prompt", "permissions: auto"),
    );
    repo.commit("synthetic CLI compatibility");
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let exe = bin.join("opencode");
    std::fs::write(&exe, format!(
        "#!/bin/sh\ncase \"$1\" in\n--version) echo 'opencode v2.0.32';;\n--help) printf '%s\\n' {};;\nmini) test \"$2\" = --help || exit 91; printf '%s\\n' {};;\nmodels) printf 'ollama/glm-5.3:cloud\\n';;\n*) echo 'unexpected native invocation' >&2; exit 91;;\nesac\n",
        common::shell_quoted(include_str!("fixtures/opencode/2.0.32-help.txt")),
        common::shell_quoted(include_str!("fixtures/opencode/2.0.32-mini-help.txt"))
    )).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
    let result = common::ahu()
        .args([
            "@smoke",
            "--prompt",
            "inspect synthetic data",
            "--dry-run",
            "--output",
            "json",
        ])
        .current_dir(repo.path())
        .env("HOME", home.path())
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("AHU_CMUX_BIN", home.path().join("no-cmux"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let preview: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let command = preview["argv"].as_array().unwrap();
    assert_eq!(command[1], "mini");
    assert!(command.iter().any(|arg| arg == "--model"), "{preview}");
    assert!(command.iter().any(|arg| arg == "--prompt"), "{preview}");

    let auto_result = common::ahu()
        .args([
            "@auto",
            "--prompt",
            "inspect synthetic data",
            "--allow-widened-approvals",
            "--dry-run",
            "--output",
            "json",
        ])
        .current_dir(repo.path())
        .env("HOME", home.path())
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("AHU_CMUX_BIN", home.path().join("no-cmux"))
        .output()
        .unwrap();
    assert!(
        auto_result.status.success(),
        "{}",
        String::from_utf8_lossy(&auto_result.stderr)
    );
    let auto_preview: serde_json::Value = serde_json::from_slice(&auto_result.stdout).unwrap();
    let auto_command = auto_preview["argv"].as_array().unwrap();
    assert_eq!(auto_command[1], "--auto", "{auto_preview}");
    assert_eq!(auto_command[2], "mini", "{auto_preview}");
    assert!(
        auto_command.iter().any(|arg| arg == "--model"),
        "{auto_preview}"
    );
    assert!(
        auto_command.iter().any(|arg| arg == "--prompt"),
        "{auto_preview}"
    );
    assert_eq!(
        common::git(repo.path(), &["worktree", "list", "--porcelain"])
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count(),
        1
    );
}

#[test]
fn opencode_interactive_help_is_checked_on_the_resolved_executable_each_time() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let exe = root.path().join("opencode");
    let args: Vec<String> = [
        "--model",
        "ollama/glm-5.3:cloud",
        "--auto",
        "--prompt",
        "task",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for (help, mini_help, expected_mini) in [
        (include_str!("fixtures/opencode/1.x-help.txt"), "", false),
        (
            include_str!("fixtures/opencode/2.0.32-help.txt"),
            include_str!("fixtures/opencode/2.0.32-mini-help.txt"),
            true,
        ),
        (include_str!("fixtures/opencode/1.x-help.txt"), "", false),
    ] {
        std::fs::write(
            &exe,
            format!(
                "#!/bin/sh\nif [ \"$1\" = --help ] && [ $# -eq 1 ]; then printf '%s\\n' {}; exit 0; fi\nif [ \"$1\" = mini ] && [ \"$2\" = --help ] && [ $# -eq 2 ]; then printf '%s\\n' {}; exit 0; fi\nexit 91\n",
                common::shell_quoted(help),
                common::shell_quoted(mini_help)
            ),
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut launch_args = args.clone();
        ahu::selection::check_interactive_cli(&exe, "opencode", &mut launch_args, &project)
            .unwrap();
        assert_eq!(
            launch_args.iter().take(2).any(|arg| arg == "mini"),
            expected_mini
        );
        if expected_mini {
            assert_eq!(launch_args[0], "--auto");
            assert_eq!(launch_args[1], "mini");
        }
    }
    // Successful-looking output with a failed probe is never admission evidence.
    std::fs::write(
        &exe,
        "#!/bin/sh\nprintf '%s\\n' '--model' '--auto' '--prompt'\nexit 1\n",
    )
    .unwrap();
    assert!(
        ahu::selection::check_interactive_cli(&exe, "opencode", &mut args.clone(), &project)
            .unwrap_err()
            .to_string()
            .contains("probe failed or timed out")
    );
}

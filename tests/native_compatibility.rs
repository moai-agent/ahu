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
    std::fs::write(&exe, "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 1.18.34; exit 0; fi\nif [ \"$1\" = models ]; then echo ollama/other; exit 0; fi\nexit 91\n").unwrap();
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

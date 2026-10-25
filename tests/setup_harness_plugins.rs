//! Offline setup workflow; synthetic package code must never execute.
mod common;
use std::os::unix::fs::PermissionsExt;
use std::{fs, path::Path, process::Output};

fn temp() -> tempfile::TempDir {
    tempfile::tempdir_in(fs::canonicalize(std::env::temp_dir()).unwrap()).unwrap()
}
fn commit(root: &Path) {
    common::git(root, &["add", "."]);
    common::git(
        root,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "fixture",
        ],
    );
}
fn repo() -> tempfile::TempDir {
    let repo = temp();
    common::git(repo.path(), &["init", "-q"]);
    fs::write(repo.path().join("README.md"), "fixture\n").unwrap();
    commit(repo.path());
    repo
}
fn package(root: &Path, id: &str, declaration: bool) {
    let launch = if declaration {
        "harness_launch:\n  harness: codex\n  path: planner\n  protocol_version: 1\n  timeout_ms: 1000\n"
    } else {
        ""
    };
    fs::write(root.join("ahu-plugin.md"), format!("---\nokf_version: '0.2'\ntype: ahu:plugin\nschema_version: 1\nid: {id}\npackage: fixture\nhost_api: 1\ncargo_manifest: Cargo.toml\n{launch}---\n")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = 'fixture'\nversion = '1.0.0'\nbuild = 'build.rs'\n",
    )
    .unwrap();
    fs::write(root.join("build.rs"), "compile_error!(\"must not build\");").unwrap();
    // A synthetic executable that leaves observable evidence if accidentally run.
    fs::write(
        root.join("planner"),
        format!(
            "#!/bin/sh\ntouch '{}'\nexit 91\n",
            root.join("executed").display()
        ),
    )
    .unwrap();
    fs::set_permissions(root.join("planner"), fs::Permissions::from_mode(0o755)).unwrap();
}
fn run(root: &Path, args: &[&str]) -> Output {
    common::ahu()
        .current_dir(root)
        .env_remove("AHU_PLUGIN_PATH")
        .args(args)
        .output()
        .unwrap()
}
fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
#[test]
fn prepare_commit_install_inventory_and_enable_are_separate() {
    let repo = repo();
    let source = temp();
    package(source.path(), "harness-codex", true);
    let args = ["setup", "--harness-plugin", source.path().to_str().unwrap()];
    let output = success(run(repo.path(), &args));
    assert!(output.contains("codex-plugin-pilot"));
    let (binding, _) = ahu::plugin_binding::load(repo.path(), "harness-codex")
        .unwrap()
        .unwrap();
    assert!(!binding.enabled);
    assert!(binding.grants.is_empty());
    assert_eq!(
        binding.source.sha256,
        ahu::plugin_install::digest(source.path()).unwrap()
    );
    assert!(repo.path().join(".agents/ahu/lock.md").is_file());
    success(run(repo.path(), &args)); // Identical preparation preserves the binding.
    assert!(
        !run(repo.path(), &["plugins", "install", "harness-codex"])
            .status
            .success()
    );
    commit(repo.path());
    success(run(repo.path(), &["plugins", "install", "harness-codex"]));
    let listed = success(run(repo.path(), &["setup", "--list-harness-plugins"]));
    assert!(listed.contains("harness-codex for codex"));
    assert!(
        !ahu::plugin_binding::load(repo.path(), "harness-codex")
            .unwrap()
            .unwrap()
            .0
            .enabled
    );
    success(run(repo.path(), &["plugins", "enable", "harness-codex"]));
    let (mut binding, mut document) = ahu::plugin_binding::load(repo.path(), "harness-codex")
        .unwrap()
        .unwrap();
    binding
        .grants
        .push(ahu::plugin_binding::Grant::HarnessLaunch);
    document.set_settings(&binding).unwrap();
    fs::write(
        repo.path().join(".agents/ahu/plugins/harness-codex.md"),
        document.render().unwrap(),
    )
    .unwrap();
    success(run(repo.path(), &["lock", "--update"]));
    commit(repo.path());
    assert!(!run(repo.path(), &args).status.success()); // Never overwrite enabled policy.
    assert!(!source.path().join("executed").exists());
}
#[test]
fn invalid_packages_leave_no_binding() {
    for variant in [
        "missing",
        "nonexecutable",
        "source-only",
        "identity",
        "version",
        "symlink",
    ] {
        let repo = repo();
        let source = temp();
        package(
            source.path(),
            if variant == "identity" {
                "other"
            } else {
                "harness-codex"
            },
            variant != "source-only",
        );
        match variant {
            "missing" => fs::remove_file(source.path().join("planner")).unwrap(),
            "nonexecutable" => fs::set_permissions(
                source.path().join("planner"),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap(),
            "version" => fs::write(
                source.path().join("Cargo.toml"),
                "[package]\nname = 'fixture'\nversion.workspace = true\n",
            )
            .unwrap(),
            "symlink" => {
                fs::remove_file(source.path().join("planner")).unwrap();
                std::os::unix::fs::symlink("build.rs", source.path().join("planner")).unwrap();
            }
            _ => {}
        }
        assert!(
            !run(
                repo.path(),
                &["setup", "--harness-plugin", source.path().to_str().unwrap()]
            )
            .status
            .success(),
            "{variant}"
        );
        assert!(!repo.path().join(".agents/ahu/plugins").exists());
        assert!(!source.path().join("executed").exists());
    }
}
#[test]
fn discovery_reports_ambiguity_rejects_duplicate_ids_and_never_executes() {
    let repo = repo();
    let first = temp();
    let second = temp();
    package(first.path(), "harness-codex", true);
    package(second.path(), "alternative", true);
    let paths = std::env::join_paths([first.path(), second.path()]).unwrap();
    let discover = || {
        common::ahu()
            .current_dir(repo.path())
            .env("AHU_PLUGIN_PATH", &paths)
            .args(["setup", "--list-harness-plugins"])
            .output()
            .unwrap()
    };
    assert!(success(discover()).contains("multiple candidates; no automatic selection"));
    package(second.path(), "harness-codex", true);
    assert!(!discover().status.success());
    assert!(!first.path().join("executed").exists());
    assert!(!second.path().join("executed").exists());
    assert!(!repo.path().join(".agents").exists());
}

#[test]
fn source_drift_and_conflicts_preserve_prepared_binding_and_lock() {
    let repo = repo();
    let source = temp();
    package(source.path(), "harness-codex", true);
    let args = ["setup", "--harness-plugin", source.path().to_str().unwrap()];
    success(run(repo.path(), &args));
    let binding = repo.path().join(".agents/ahu/plugins/harness-codex.md");
    let lock = repo.path().join(".agents/ahu/lock.md");
    let before = (fs::read(&binding).unwrap(), fs::read(&lock).unwrap());
    commit(repo.path());
    fs::write(source.path().join("extra"), "changed package").unwrap();
    assert!(!run(repo.path(), &args).status.success());
    assert_eq!(
        before,
        (fs::read(&binding).unwrap(), fs::read(&lock).unwrap())
    );
    assert!(
        !run(repo.path(), &["plugins", "install", "harness-codex"])
            .status
            .success()
    );
    assert!(!source.path().join("executed").exists());
}

#[test]
fn installed_discovery_checks_pins_even_without_a_current_binding() {
    let repo = repo();
    let source = temp();
    package(source.path(), "harness-codex", true);
    success(run(
        repo.path(),
        &["setup", "--harness-plugin", source.path().to_str().unwrap()],
    ));
    commit(repo.path());
    success(run(repo.path(), &["plugins", "install", "harness-codex"]));
    let output = success(run(
        repo.path(),
        &["plugins", "installed", "--output", "json"],
    ));
    let inventory: serde_json::Value = serde_json::from_str(&output).unwrap();
    let installation = Path::new(
        inventory["installations"][0]["installation"]
            .as_str()
            .unwrap(),
    );
    fs::remove_file(repo.path().join(".agents/ahu/plugins/harness-codex.md")).unwrap();
    commit(repo.path());
    assert!(
        success(run(repo.path(), &["setup", "--list-harness-plugins"]))
            .contains("verified-installed-package")
    );
    fs::write(installation.join("package/planner"), "tampered").unwrap();
    assert!(
        !run(repo.path(), &["setup", "--list-harness-plugins"])
            .status
            .success()
    );
    assert!(!source.path().join("executed").exists());
}

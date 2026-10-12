//! Committed project inventory uses synthetic Git objects, never live sources.
mod common;
use std::path::Path;
use std::process::Output;

fn document(id: &str) -> String {
    format!(
        "---\nokf_version: '0.2'\ntype: ahu:plugin-binding\nahu:\n  schema_version: 1\n  id: {id}\n  version: 1.0.0\n  source:\n    repository: https://forge.example.invalid/team/host.git\n    revision: {}\n    sha256: {}\n  compatibility:\n    ahu: {}\n    host_api: 1\n  enabled: true\n  grants: [interactive-backend]\nexecutable: {{path: ./never-run}}\n---\n\n$(touch executed)\n",
        "a".repeat(40),
        "b".repeat(64),
        env!("CARGO_PKG_VERSION")
    )
}

fn write(root: &Path, name: &str, text: &str) {
    let path = root.join(".agents/ahu/plugins").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn commit(root: &Path) {
    common::git(root, &["add", "-f", ".agents"]);
    common::git(
        root,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "synthetic bindings",
        ],
    );
}

fn repo() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    common::git(root.path(), &["init", "-q"]);
    write(root.path(), "fixture.md", &document("fixture"));
    commit(root.path());
    root
}

fn inventory(root: &Path) -> Output {
    common::ahu()
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        // The project mode must not consult process package selection at all.
        .env("AHU_PLUGIN_PATH", "/missing/package")
        .env("AHU_INTERACTIVE_PLUGIN", "unknown")
        .args(["plugins", "--project", "--output", "json"])
        .output()
        .unwrap()
}

#[test]
fn committed_bindings_are_offline_inert_and_separate_from_local_selection() {
    let root = repo();
    // Unrelated configuration is deliberately unreadable as configuration.
    std::fs::write(root.path().join(".agents/ahu/config.md"), "invalid").unwrap();
    let output = inventory(root.path());
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["origin"], "committed-project-bindings");
    assert_eq!(
        value["revision"],
        common::git(root.path(), &["rev-parse", "HEAD"])
    );
    let status = &value["bindings"][0];
    assert_eq!(status["binding"]["id"], "fixture");
    assert_eq!(status["binding"]["enabled"], true);
    assert_eq!(status["ahu_compatible"], true);
    assert_eq!(status["source_verified"], false);
    assert_eq!(status["activation_supported"], true);
    assert_eq!(
        status["activation_blockers"],
        serde_json::json!(["source-unverified"])
    );
    assert!(!root.path().join("executed").exists());
    assert!(!root.path().join(".ahu").exists());
    assert!(inventory(&root.path().join(".agents")).status.success());
}

#[test]
fn index_worktree_and_untracked_changes_do_not_change_the_committed_view() {
    let root = repo();
    let before = inventory(root.path()).stdout;
    write(root.path(), "fixture.md", "invalid staged replacement");
    common::git(root.path(), &["add", ".agents/ahu/plugins/fixture.md"]);
    std::fs::remove_file(root.path().join(".agents/ahu/plugins/fixture.md")).unwrap();
    write(root.path(), "untracked.md", "invalid untracked binding");
    let output = inventory(root.path());
    assert!(output.status.success(), "{output:?}");
    assert_eq!(before, output.stdout);
}

#[test]
fn disabled_and_incompatible_declarations_have_explicit_blockers_and_sorted_ids() {
    let root = repo();
    write(
        root.path(),
        "alpha.md",
        &document("alpha")
            .replace("enabled: true", "enabled: false")
            .replace(
                &format!("ahu: {}", env!("CARGO_PKG_VERSION")),
                "ahu: 999.0.0",
            ),
    );
    commit(root.path());
    let output = inventory(root.path());
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["bindings"][0]["binding"]["id"], "alpha");
    assert_eq!(value["bindings"][0]["ahu_compatible"], false);
    assert_eq!(
        value["bindings"][0]["activation_blockers"],
        serde_json::json!(["disabled", "ahu-version-mismatch", "source-unverified"])
    );
}

#[test]
fn invalid_committed_bindings_fail_without_partial_inventory() {
    for text in [
        document("other"),
        document("fixture").replace("host_api: 1", "host_api: 2"),
        document("fixture").replace("https://forge", "https://user:secret@forge"),
        document("fixture").replace("[interactive-backend]", "[shell]"),
        "x".repeat(ahu::okf::MAX_BYTES as usize + 1),
    ] {
        let root = repo();
        write(root.path(), "fixture.md", &text);
        commit(root.path());
        let output = inventory(root.path());
        assert!(!output.status.success(), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("secret"));
    }
}

#[test]
fn committed_symlinks_and_nested_directories_are_refused() {
    for path in [
        ".agents",
        ".agents/ahu",
        ".agents/ahu/plugins",
        ".agents/ahu/plugins/fixture.md",
    ] {
        let root = repo();
        let original = root.path().join(path);
        if original.is_dir() {
            std::fs::remove_dir_all(&original).unwrap();
        } else {
            std::fs::remove_file(&original).unwrap();
        }
        std::os::unix::fs::symlink("missing", original).unwrap();
        commit(root.path());
        let output = inventory(root.path());
        assert!(!output.status.success(), "{path}: {output:?}");
        assert!(output.stdout.is_empty());
    }
    let root = repo();
    write(root.path(), "nested/fixture.md", &document("fixture"));
    commit(root.path());
    assert!(!inventory(root.path()).status.success());
}

#[test]
fn absent_committed_bindings_are_empty_but_excess_entries_fail() {
    let root = tempfile::tempdir().unwrap();
    assert!(!inventory(root.path()).status.success());
    common::git(root.path(), &["init", "-q"]);
    let output = inventory(root.path());
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["revision"], serde_json::Value::Null);
    assert_eq!(value["bindings"], serde_json::json!([]));
    common::git(
        root.path(),
        &[
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "--allow-empty",
            "-qm",
            "empty fixture",
        ],
    );
    let output = inventory(root.path());
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["bindings"], serde_json::json!([]));
    for n in 0..33 {
        write(
            root.path(),
            &format!("fixture-{n}.md"),
            &document(&format!("fixture-{n}")),
        );
    }
    commit(root.path());
    assert!(!inventory(root.path()).status.success());
}

#[test]
fn project_inventory_cannot_be_combined_with_activation() {
    let root = repo();
    for flags in [
        vec!["--project", "--probe"],
        vec!["--probe", "--project"],
        vec!["--project", "--project"],
    ] {
        let output = common::ahu()
            .current_dir(root.path())
            .arg("plugins")
            .args(flags)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}

fn files(root: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    fn collect(root: &Path, result: &mut std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(&path, result);
            } else {
                result.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    let mut result = std::collections::BTreeMap::new();
    collect(root, &mut result);
    result
}

#[test]
fn missing_promisor_object_never_runs_a_transport_or_changes_git_files() {
    use std::os::unix::fs::PermissionsExt;
    let root = repo();
    let blob = common::git(
        root.path(),
        &["rev-parse", "HEAD:.agents/ahu/plugins/fixture.md"],
    );
    let helper = root.path().join("fetch-helper");
    let marker = root.path().join("transport-executed");
    std::fs::write(
        &helper,
        format!(
            "#!/bin/sh\ntouch {}\nexit 1\n",
            common::shell_quoted(&marker.to_string_lossy())
        ),
    )
    .unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
    common::git(
        root.path(),
        &[
            "config",
            "remote.origin.url",
            &format!("ext::{}", helper.display()),
        ],
    );
    common::git(root.path(), &["config", "remote.origin.promisor", "true"]);
    common::git(root.path(), &["config", "protocol.ext.allow", "always"]);
    std::fs::remove_file(
        root.path()
            .join(".git/objects")
            .join(&blob[..2])
            .join(&blob[2..]),
    )
    .unwrap();
    let before = files(&root.path().join(".git"));
    let output = inventory(root.path());
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("offline"));
    assert!(output.stdout.is_empty());
    assert!(!marker.exists());
    assert_eq!(files(&root.path().join(".git")), before);
}

#[test]
fn replacement_objects_cannot_change_committed_bindings() {
    let root = repo();
    let before = inventory(root.path());
    assert!(before.status.success(), "{before:?}");
    let original = common::git(
        root.path(),
        &["rev-parse", "HEAD:.agents/ahu/plugins/fixture.md"],
    );
    let replacement = root.path().join("replacement");
    std::fs::write(&replacement, "invalid replacement").unwrap();
    let replacement_oid = common::git(
        root.path(),
        &["hash-object", "-w", replacement.to_str().unwrap()],
    );
    common::git(root.path(), &["replace", &original, &replacement_oid]);
    let output = inventory(root.path());
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, before.stdout);
}

#[test]
fn repository_selector_and_text_help_explain_the_inspection_boundary() {
    let root = repo();
    let outside = tempfile::tempdir().unwrap();
    let output = common::ahu()
        .current_dir(outside.path())
        .args([
            "--repo",
            root.path().to_str().unwrap(),
            "plugins",
            "--project",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("fixture@1.0.0"));
    assert!(text.contains("committed files only"));
    assert!(text.contains("source-unverified"));
    let help = common::ahu().args(["help", "plugins"]).output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--project"));
}

fn selected_inventory(root: &Path, selection: Option<&str>) -> Output {
    let mut command = common::ahu();
    command
        .current_dir(root)
        .env_remove("AHU_PLUGIN_PATH")
        .env_remove("AHU_INTERACTIVE_PLUGIN");
    if let Some(selection) = selection {
        command.env("AHU_INTERACTIVE_PLUGIN", selection);
    }
    command
        .args(["plugins", "--output", "json"])
        .output()
        .unwrap()
}

#[test]
fn reserved_committed_binding_ids_are_rejected() {
    for id in ["none", "cmux"] {
        for enabled in [true, false] {
            let root = tempfile::tempdir().unwrap();
            common::git(root.path(), &["init", "-q"]);
            write(
                root.path(),
                &format!("{id}.md"),
                &document(id).replace("enabled: true", &format!("enabled: {enabled}")),
            );
            commit(root.path());
            for output in [
                inventory(root.path()),
                selected_inventory(root.path(), Some("none")),
                selected_inventory(root.path(), Some("cmux")),
                selected_inventory(root.path(), None),
            ] {
                assert!(!output.status.success(), "{id}: {output:?}");
                assert!(output.stdout.is_empty(), "{output:?}");
                assert!(
                    String::from_utf8_lossy(&output.stderr)
                        .contains(&format!("plugin binding id '{id}' is reserved")),
                    "{output:?}"
                );
            }
        }
    }
}

#[test]
fn none_selects_no_committed_binding_and_never_activates() {
    let root = repo();
    let output = selected_inventory(root.path(), Some("none"));
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["selected"] == false)
    );
    let output = common::ahu()
        .current_dir(root.path())
        .env_remove("AHU_PLUGIN_PATH")
        .env("AHU_INTERACTIVE_PLUGIN", "none")
        .args(["plugins", "--probe"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
}

#[test]
fn corrupt_refs_missing_objects_and_malformed_policy_are_not_unborn() {
    for damage in [
        "ref",
        "commit",
        "tree",
        "blob",
        "policy",
        "config",
        "detached",
        "invalid-head",
    ] {
        let root = repo();
        match damage {
            "ref" => {
                let reference = common::git(root.path(), &["symbolic-ref", "HEAD"]);
                std::fs::write(root.path().join(".git").join(reference), "broken\n").unwrap();
            }
            "invalid-head" => {
                std::fs::write(
                    root.path().join(".git/HEAD"),
                    "ref: refs/heads/invalid..branch\n",
                )
                .unwrap();
            }
            "config" => {
                std::fs::write(root.path().join(".git/config"), "[invalid\n").unwrap();
            }
            "detached" => {
                std::fs::write(
                    root.path().join(".git/HEAD"),
                    format!("{}\n", "a".repeat(40)),
                )
                .unwrap();
            }
            "policy" => {
                write(root.path(), "fixture.md", "invalid");
                commit(root.path());
            }
            _ => {
                let spec = match damage {
                    "commit" => "HEAD",
                    "tree" => "HEAD^{tree}",
                    _ => "HEAD:.agents/ahu/plugins/fixture.md",
                };
                let object = common::git(root.path(), &["rev-parse", spec]);
                std::fs::remove_file(
                    root.path()
                        .join(".git/objects")
                        .join(&object[..2])
                        .join(&object[2..]),
                )
                .unwrap();
            }
        }
        let output = inventory(root.path());
        assert!(!output.status.success(), "{damage}: {output:?}");
        assert!(output.stdout.is_empty());
        if !matches!(damage, "config" | "invalid-head") {
            for selection in [None, Some("none"), Some("cmux")] {
                let output = selected_inventory(root.path(), selection);
                assert!(
                    !output.status.success(),
                    "{damage}/{selection:?}: {output:?}"
                );
                assert!(output.stdout.is_empty());
            }
        }
    }
}

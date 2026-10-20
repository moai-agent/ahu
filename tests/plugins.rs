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
        .env("PLUGIN_EXECUTED", root.join("executed"))
        .env_remove("AHU_PLUGIN_PATH")
        .env_remove("AHU_INTERACTIVE_PLUGIN");
    command
}

#[test]
fn inventory_never_executes_plugins_or_requires_a_repository() {
    let root = tempfile::tempdir().unwrap();
    for selection in [None, Some("none"), Some("terminal")] {
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
        let plugin = value["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|plugin| plugin["id"] == "terminal")
            .unwrap();
        assert_eq!(plugin["selected"], selection != Some("none"));
        assert_eq!(plugin["host_api"], 0);
        assert!(!root.path().join("executed").exists());
        assert!(!root.path().join(".ahu").exists());
    }
}

#[test]
fn default_build_selects_the_current_terminal_backend() {
    let root = tempfile::tempdir().unwrap();
    let output = common::ahu()
        .current_dir(root.path())
        .env_remove("AHU_INTERACTIVE_PLUGIN")
        .env_remove("AHU_PLUGIN_PATH")
        .args(["plugins", "--output", "json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let plugins = value["plugins"].as_array().unwrap();
    assert!(
        plugins
            .iter()
            .any(|plugin| { plugin["id"] == "terminal" && plugin["selected"] == true })
    );
    assert!(!root.path().join(".ahu").exists());
}

#[test]
fn unborn_inventory_and_activation_share_selection() {
    let root = tempfile::tempdir().unwrap();
    common::git(root.path(), &["init", "-q"]);
    // Uncommitted policy is not authority, even when it is malformed.
    std::fs::create_dir_all(root.path().join(".agents/ahu/plugins")).unwrap();
    std::fs::write(
        root.path().join(".agents/ahu/plugins/fixture.md"),
        "invalid",
    )
    .unwrap();
    for selection in [None, Some("none"), Some("terminal")] {
        let mut command = fixture_command(root.path());
        if let Some(value) = selection {
            command.env("AHU_INTERACTIVE_PLUGIN", value);
        }
        let output = command
            .args(["plugins", "--output", "json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{selection:?}: {output:?}");
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let plugins = value["plugins"].as_array().unwrap();
        assert_eq!(plugins.len(), 1);
        let terminal = plugins
            .iter()
            .find(|plugin| plugin["id"] == "terminal")
            .unwrap();
        assert_eq!(terminal["selected"], selection != Some("none"));
        assert!(!root.path().join("executed").exists());

        let mut probe = fixture_command(root.path());
        std::fs::write(
            root.path().join("cmux"),
            include_str!("fixtures/cmux-host.py"),
        )
        .unwrap();
        probe
            .env("HOST_ROOT", root.path())
            .env("HOST_MODE", "normal");
        if let Some(value) = selection {
            probe.env("AHU_INTERACTIVE_PLUGIN", value);
        }
        let output = probe
            .args(["plugins", "--probe", "--output", "json"])
            .output()
            .unwrap();
        let active = false;
        assert_eq!(output.status.success(), active, "{selection:?}: {output:?}");
        assert_eq!(root.path().join("pinged").exists(), active);
        if active {
            let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["plugin_id"], "cmux");
            std::fs::remove_file(root.path().join("pinged")).unwrap();
        }
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
        assert!(String::from_utf8_lossy(&output.stderr).contains("must be terminal, none"));
        assert!(!root.path().join("executed").exists());
    }
}

#[test]
fn terminal_default_and_explicit_none_never_activate_a_provider() {
    for selection in [None, Some("none"), Some("cmux")] {
        let root = tempfile::tempdir().unwrap();
        let mut command = common::ahu();
        command
            .current_dir(root.path())
            .args(["plugins", "--output", "json"])
            .env_remove("AHU_PLUGIN_PATH")
            .env_remove("AHU_INTERACTIVE_PLUGIN");
        if let Some(selection) = selection {
            command.env("AHU_INTERACTIVE_PLUGIN", selection);
        }
        let output = command.output().unwrap();
        if selection == Some("cmux") {
            assert!(!output.status.success());
        } else {
            assert!(output.status.success(), "{output:?}");
            let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["plugins"][0]["id"], "terminal");
            assert_eq!(value["plugins"][0]["selected"], selection != Some("none"));
        }
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
            "#!/bin/sh\nif [ \"$1\" = --help ]; then printf '  -m, --model MODEL\\n  --prompt PROMPT\\n  --auto\\n'; exit 0; fi\ntouch \"$COORDINATOR_STARTED\"\nexit 7\n",
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

fn local_package(root: &std::path::Path, directory: &str, id: &str) -> std::path::PathBuf {
    let package = root.join(directory);
    std::fs::create_dir_all(package.join("source")).unwrap();
    std::fs::write(
        package.join("ahu-plugin.md"),
        format!(
            "---\nokf_version: \"0.2\"\ntype: ahu:plugin\ntitle: Fixture package\ndescription: Local test package\ntags: [testing, plugins]\nvendor_metadata:\n  documentation: https://example.invalid/plugin\nschema_version: 1\nid: {id:?}\npackage: \"fixture-host\"\nhost_api: 1\ncargo_manifest: \"source/Cargo.toml\"\n---\n\n# Fixture package\n"
        ),
    )
    .unwrap();
    std::fs::write(
        package.join("source/Cargo.toml"),
        "[package]\nname = \"fixture-host\"\nversion = \"0.1.0\"\nbuild = \"build.rs\"\n",
    )
    .unwrap();
    package.canonicalize().unwrap()
}

fn inventory(
    root: &std::path::Path,
    paths: &[&std::path::Path],
    selected: Option<&str>,
) -> std::process::Output {
    let mut command = fixture_command(root);
    command.env("AHU_PLUGIN_PATH", std::env::join_paths(paths).unwrap());
    if let Some(id) = selected {
        command.env("AHU_INTERACTIVE_PLUGIN", id);
    }
    command
        .args(["plugins", "--output", "json"])
        .output()
        .unwrap()
}

#[test]
fn local_metadata_discovery_is_sorted_and_selection_never_executes() {
    let root = tempfile::tempdir().unwrap();
    let zeta = local_package(root.path(), "zeta", "zeta");
    let alpha = local_package(root.path(), "alpha", "alpha");
    // Even a Cargo build script and an executable with a marker are inert.
    std::fs::write(
        alpha.join("source/build.rs"),
        "compile_error!(\"must not build during discovery\");",
    )
    .unwrap();
    for selected in [None, Some("none"), Some("alpha")] {
        let output = inventory(root.path(), &[&zeta, &alpha], selected);
        assert!(output.status.success(), "{output:?}");
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let plugins = value["plugins"].as_array().unwrap();
        let actual = plugins
            .iter()
            .map(|p| p["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        let expected = vec!["alpha", "terminal", "zeta"];
        assert_eq!(actual, expected);
        let alpha_status = plugins.iter().find(|p| p["id"] == "alpha").unwrap();
        assert_eq!(alpha_status["selected"], selected == Some("alpha"));
        assert_eq!(alpha_status["origin"], "local-manifest");
        assert_eq!(alpha_status["metadata"]["title"], "Fixture package");
        assert_eq!(
            alpha_status["metadata"]["description"],
            "Local test package"
        );
        assert_eq!(alpha_status["metadata"]["tags"][0], "testing");
        assert_eq!(
            alpha_status["metadata"]["vendor_metadata"]["documentation"],
            "https://example.invalid/plugin"
        );
        assert_eq!(
            alpha_status["manifest"],
            alpha.join("ahu-plugin.md").to_str().unwrap()
        );
        assert!(
            alpha_status["activation_blocker"]
                .as_str()
                .unwrap()
                .contains("no executable host declaration")
        );
        assert_eq!(
            plugins
                .iter()
                .find(|plugin| plugin["id"] == "zeta")
                .unwrap()["selected"],
            false
        );
        assert!(!root.path().join("executed").exists());
        assert!(!root.path().join(".ahu").exists());
        assert!(!alpha.join("source/target").exists());
    }
    // An unlisted package cannot be selected; there is no implicit cwd scan.
    let output = fixture_command(root.path())
        .current_dir(&alpha)
        .env("AHU_INTERACTIVE_PLUGIN", "alpha")
        .arg("plugins")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn local_selection_refuses_activation_without_plugin_or_terminal_fallback() {
    const CASE: &str = "local_selection_refuses_activation_without_plugin_or_terminal_fallback";
    if common::is_child_case(CASE) {
        let error = ahu::interactive::discover().err().unwrap();
        assert_eq!(error.kind(), ahu::util::ErrorKind::Prerequisite);
        assert!(error.to_string().contains("committed project binding"));
        let coordinator = ahu::interactive::discover_for_coordinator();
        if std::env::var_os("CMUX_WORKSPACE_ID").is_some() {
            assert!(coordinator.is_err());
        } else {
            assert!(coordinator.unwrap().is_none());
        }
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let package = local_package(root.path(), "package", "fixture");
    let _ = fixture_command(root.path());
    for inside in [false, true] {
        let output = common::run_child_case(CASE, |command| {
            command
                .env("AHU_PLUGIN_PATH", &package)
                .env("AHU_INTERACTIVE_PLUGIN", "fixture")
                .env("PLUGIN_EXECUTED", root.path().join("executed"))
                .env_remove("CMUX_WORKSPACE_ID");
            if inside {
                command.env("CMUX_WORKSPACE_ID", "caller");
            }
        });
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        assert!(!root.path().join("executed").exists());
    }
}

#[test]
fn malformed_or_incompatible_local_manifests_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let package = local_package(root.path(), "package", "fixture");
    let path = package.join("ahu-plugin.md");
    let valid = std::fs::read_to_string(&path).unwrap();
    let unknown_execution_like_metadata =
        valid.replace("host_api: 1\n", "host_api: 1\nexecute: \"touch marker\"\n");
    std::fs::write(&path, unknown_execution_like_metadata).unwrap();
    let output = inventory(root.path(), &[&package], None);
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "fixture")
            .unwrap()["metadata"]["execute"],
        "touch marker"
    );
    assert!(!root.path().join("executed").exists());
    for invalid in [
        String::new(),
        "not frontmatter".into(),
        valid.replace("schema_version: 1", "schema_version: 2"),
        valid.replace("host_api: 1", "host_api: 2"),
        valid.replace("host_api: 1", "host_api: \"1\""),
        valid.replace("host_api: 1\n", ""),
        valid.replace("host_api: 1", "id: \"duplicate-key\"\nhost_api: 1"),
        valid.replace(
            "title: Fixture package\n",
            "title: Fixture package\ntitle: Duplicate\n",
        ),
        valid.replace("\"fixture\"", "\"none\""),
        valid.replace("\"fixture\"", "\"cmux\""),
        valid.replace("\"fixture\"", "\"../escape\""),
        valid.replace("\"fixture\"", "\"Uppercase\""),
        valid.replace("\"fixture\"", "\"\""),
        valid.replace("\"fixture\"", &format!("{:?}", "x".repeat(65))),
        valid.replace("fixture-host", "different-package"),
        "#".repeat(65537),
    ] {
        std::fs::write(&path, invalid).unwrap();
        let output = inventory(root.path(), &[&package], Some("none"));
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(!root.path().join("executed").exists());
    }
    std::fs::write(&path, [0xff]).unwrap();
    assert_eq!(
        inventory(root.path(), &[&package], None).status.code(),
        Some(2)
    );
    std::fs::write(&path, valid).unwrap();
    for cargo in [
        "[workspace]",
        "not toml",
        "[package]\nname = 7",
        "[package]\nname = \"wrong\"",
    ] {
        std::fs::write(package.join("source/Cargo.toml"), cargo).unwrap();
        assert_eq!(
            inventory(root.path(), &[&package], None).status.code(),
            Some(2)
        );
    }
}

#[test]
fn duplicate_identities_and_invalid_search_paths_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let one = local_package(root.path(), "one", "fixture");
    let two = local_package(root.path(), "two", "fixture");
    for paths in [
        vec![one.as_path(), two.as_path()],
        vec![one.as_path(), one.as_path()],
    ] {
        let output = inventory(root.path(), &paths, None);
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("duplicate plugin identity"));
    }
    for path in [
        "",
        "relative",
        "/does-not-exist",
        "https://example.invalid/plugin",
    ] {
        let output = fixture_command(root.path())
            .env("AHU_PLUGIN_PATH", path)
            .arg("plugins")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
    let output = inventory(root.path(), &[&one.join("../one")], None);
    assert_eq!(output.status.code(), Some(2));
    let output = inventory(root.path(), &[&one.join(".")], None);
    assert_eq!(output.status.code(), Some(2));
    let paths: Vec<_> = (0..33)
        .map(|n| local_package(root.path(), &format!("package-{n}"), &format!("plugin-{n}")))
        .collect();
    let output = inventory(
        root.path(),
        &paths.iter().map(|p| p.as_path()).collect::<Vec<_>>(),
        None,
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("exceeds 32"));
}

#[test]
fn package_paths_cannot_escape_the_manifest_directory() {
    let root = tempfile::tempdir().unwrap();
    let package = local_package(root.path(), "package", "fixture");
    let path = package.join("ahu-plugin.md");
    let valid = std::fs::read_to_string(&path).unwrap();
    for target in [
        "../Cargo.toml",
        "source/../../Cargo.toml",
        "/Cargo.toml",
        "./source/Cargo.toml",
        "source/../source/Cargo.toml",
        "source//Cargo.toml",
        "source",
        "",
        "missing/Cargo.toml",
        "source/Cargo.toml/child",
    ] {
        std::fs::write(&path, valid.replace("source/Cargo.toml", target)).unwrap();
        let output = inventory(root.path(), &[&package], None);
        assert_eq!(output.status.code(), Some(2), "{target}: {output:?}");
    }
}

#[test]
fn symlinks_and_nonregular_metadata_are_rejected_at_every_component() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let package = local_package(root.path(), "package", "fixture");
    let alias = root.path().canonicalize().unwrap().join("alias");
    symlink(&package, &alias).unwrap();
    assert_eq!(
        inventory(root.path(), &[&alias], None).status.code(),
        Some(2)
    );
    // An ancestor symlink is refused too, even when it resolves inside the root.
    let parent_alias = root.path().canonicalize().unwrap().join("parent-alias");
    symlink(root.path(), &parent_alias).unwrap();
    assert_eq!(
        inventory(root.path(), &[&parent_alias.join("package")], None)
            .status
            .code(),
        Some(2)
    );
    for target in ["ahu-plugin.md", "source", "source/Cargo.toml"] {
        let original = package.join(target);
        let moved = package.join("saved");
        std::fs::rename(&original, &moved).unwrap();
        symlink(&moved, &original).unwrap();
        assert_eq!(
            inventory(root.path(), &[&package], None).status.code(),
            Some(2),
            "{target}"
        );
        std::fs::remove_file(&original).unwrap();
        std::fs::rename(&moved, &original).unwrap();
    }
    for target in ["ahu-plugin.md", "source/Cargo.toml"] {
        let original = package.join(target);
        let moved = package.join("saved");
        std::fs::rename(&original, &moved).unwrap();
        std::fs::create_dir(&original).unwrap();
        assert_eq!(
            inventory(root.path(), &[&package], None).status.code(),
            Some(2)
        );
        std::fs::remove_dir(&original).unwrap();
        let name = std::ffi::CString::new(original.to_str().unwrap()).unwrap();
        // SAFETY: name is NUL-terminated and names a disposable fixture path.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert_eq!(
            inventory(root.path(), &[&package], None).status.code(),
            Some(2)
        );
        std::fs::remove_file(&original).unwrap();
        std::fs::rename(&moved, &original).unwrap();
    }
}

#[test]
fn metadata_bounds_apply_to_both_files_and_special_paths_stay_literal() {
    let root = tempfile::tempdir().unwrap();
    let package = local_package(root.path(), "space ; $literal", "fixture");
    for target in ["ahu-plugin.md", "source/Cargo.toml"] {
        let path = package.join(target);
        let valid = std::fs::read_to_string(&path).unwrap();
        let at_limit = format!("{valid}#{}", "x".repeat(65536 - valid.len() - 1));
        std::fs::write(&path, &at_limit).unwrap();
        assert!(inventory(root.path(), &[&package], None).status.success());
        std::fs::write(&path, format!("{at_limit}x")).unwrap();
        assert_eq!(
            inventory(root.path(), &[&package], None).status.code(),
            Some(2)
        );
        std::fs::write(&path, valid).unwrap();
    }
    let output = fixture_command(root.path())
        .env("AHU_PLUGIN_PATH", "x".repeat(65537))
        .arg("plugins")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn external_and_dangling_symlink_targets_are_refused() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let package = local_package(root.path(), "package", "fixture");
    for target in ["ahu-plugin.md", "source", "source/Cargo.toml"] {
        let original = package.join(target);
        let external = outside.path().join("target");
        std::fs::rename(&original, &external).unwrap();
        symlink(&external, &original).unwrap();
        assert_eq!(
            inventory(root.path(), &[&package], None).status.code(),
            Some(2)
        );
        std::fs::remove_file(&original).unwrap();
        symlink(outside.path().join("missing"), &original).unwrap();
        assert_eq!(
            inventory(root.path(), &[&package], None).status.code(),
            Some(2)
        );
        std::fs::remove_file(&original).unwrap();
        std::fs::rename(&external, &original).unwrap();
    }
}

#[test]
fn okf_plugin_frontmatter_accepts_line_endings_and_documentation_bodies() {
    let root = tempfile::tempdir().unwrap();
    let package = local_package(root.path(), "package", "fixture");
    let path = package.join("ahu-plugin.md");
    let valid = std::fs::read_to_string(&path).unwrap();
    for document in [
        valid.clone(),
        valid.replace('\n', "\r\n"),
        valid.replace("\"0.2\"", "'0.2'"),
        valid.replace("\"0.2\"", "0.2"),
        valid
            .split("\n# Fixture package")
            .next()
            .unwrap()
            .trim_end()
            .to_string(),
        format!("{valid}\n---\nid: body-only\nexecutable: [invalid YAML\n$(not-executed)\n"),
    ] {
        std::fs::write(&path, document).unwrap();
        let output = inventory(root.path(), &[&package], Some("fixture"));
        assert!(output.status.success(), "{output:?}");
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            value["plugins"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["selected"] == true)
                .unwrap()["id"],
            "fixture"
        );
        assert_eq!(
            value["plugins"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["id"] == "fixture")
                .unwrap()["selected"],
            true
        );
        assert!(!root.path().join("executed").exists());
    }
}

#[test]
fn malformed_okf_plugin_frontmatter_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let package = local_package(root.path(), "package", "fixture");
    let path = package.join("ahu-plugin.md");
    let valid = std::fs::read_to_string(&path).unwrap();
    for document in [
        valid.trim_start_matches("---\n").to_string(),
        format!("# Not first-line frontmatter\n{valid}"),
        format!("\u{feff}{valid}"),
        valid.replace("\n---\n", "\n--- trailing\n"),
        valid.replace("\n---\n", "\n...\n"),
        "---\n---\n# Empty frontmatter\n".into(),
        "---\n- not a mapping\n---\n".into(),
        valid.replace("okf_version: \"0.2\"\n", ""),
        valid.replace("\"0.2\"", "\"0.3\""),
        valid.replace("\"0.2\"", "[\"0.2\"]"),
        valid.replace("type: ahu:plugin\n", ""),
        valid.replace("ahu:plugin", "ahu:agent"),
        valid.replace("type: ahu:plugin", "type: ahu:plugin\ntype: ahu:plugin"),
        valid.replace("okf_version:", "okf_version: \"0.2\"\nokf_version:"),
        valid.replace("host_api: 1", "host_api: [unterminated"),
        valid.replace("host_api: 1", "host_api = 1"),
        valid.replace("host_api: 1", "executable: []\nhost_api: 1"),
    ] {
        std::fs::write(&path, document).unwrap();
        let output = inventory(root.path(), &[&package], None);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty());
        assert!(!root.path().join("executed").exists());
    }
}

#[test]
fn yaml_alias_expansion_is_bounded_during_manifest_parsing() {
    let root = tempfile::tempdir().unwrap();
    let package = local_package(root.path(), "package", "fixture");
    let path = package.join("ahu-plugin.md");
    // Keep this scalar small enough that the expanded text stays below the
    // separate byte budget; this regression must reach the node-count limit.
    let scalar = "x".repeat(8);
    let mut document = format!(
        "---\nokf_version: \"0.2\"\ntype: ahu:plugin\ntitle: Fixture package\ndescription: Alias bound test\nschema_version: 1\nid: \"fixture\"\npackage: \"fixture-host\"\nhost_api: 1\ncargo_manifest: \"source/Cargo.toml\"\nbase: &a0 \"{scalar}\"\n"
    );
    let first_level = std::iter::repeat_n("*a0", 1000)
        .collect::<Vec<_>>()
        .join(", ");
    document.push_str(&format!("first: &a1 [{first_level}]\n"));
    let second_level = std::iter::repeat_n("*a1", 100)
        .collect::<Vec<_>>()
        .join(", ");
    document.push_str(&format!("second: [{second_level}]\n"));
    document.push_str("---\n\n# Alias expansion must remain bounded.\n");
    std::fs::write(path, document).unwrap();

    let output = inventory(root.path(), &[&package], Some("none"));
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(!root.path().join("executed").exists());
}

#[test]
fn yaml_alias_expansion_is_bounded_by_retained_string_bytes() {
    let root = tempfile::tempdir().unwrap();
    let package = local_package(root.path(), "package", "fixture");
    let path = package.join("ahu-plugin.md");
    let scalar = "x".repeat(16 * 1024);
    let aliases = std::iter::repeat_n("*a0", 80)
        .collect::<Vec<_>>()
        .join(", ");
    let document = format!(
        "---\nokf_version: \"0.2\"\ntype: ahu:plugin\ntitle: Fixture package\ndescription: Alias byte bound test\nschema_version: 1\nid: \"fixture\"\npackage: \"fixture-host\"\nhost_api: 1\ncargo_manifest: \"source/Cargo.toml\"\nbase: &a0 \"{scalar}\"\nexpanded: [{aliases}]\n---\n\n# Alias expansion must remain bounded.\n"
    );
    assert!(document.len() < 64 * 1024);
    std::fs::write(path, document).unwrap();

    let output = inventory(root.path(), &[&package], Some("none"));
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(!root.path().join("executed").exists());
}

#[test]
fn legacy_plugin_filename_has_no_discovery_fallback() {
    let root = tempfile::tempdir().unwrap();
    let package = local_package(root.path(), "package", "fixture");
    std::fs::remove_file(package.join("ahu-plugin.md")).unwrap();
    std::fs::write(package.join("ahu-plugin.toml"), "schema_version = 1\nid = \"fixture\"\npackage = \"fixture-host\"\nhost_api = 1\ncargo_manifest = \"source/Cargo.toml\"\n").unwrap();
    assert_eq!(
        inventory(root.path(), &[&package], None).status.code(),
        Some(2)
    );
    assert!(!root.path().join("executed").exists());
}

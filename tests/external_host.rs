//! Offline peers exercise the actual selected executable, framing, and Host API.
mod common;
use ahu::interactive::{self, external::ExternalHost};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn fixture(root: &Path, mode: &str) -> PathBuf {
    let package = root.join("package ; $literal");
    std::fs::create_dir_all(package.join("bin")).unwrap();
    std::fs::write(
        package.join("Cargo.toml"),
        "[package]\nname = \"fixture-host\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(package.join("ahu-plugin.md"), "---\nokf_version: \"0.2\"\ntype: ahu:plugin\nschema_version: 1\nid: \"fixture\"\npackage: \"fixture-host\"\nhost_api: 1\ncargo_manifest: \"Cargo.toml\"\nexecutable:\n  path: \"bin/host\"\n  protocol_version: 2\n  timeout_ms: 1000\n---\n\n# Executable fixture\n").unwrap();
    let script = include_str!("fixtures/external-host.py")
        .replace(
            "MODE = \"normal\"",
            &format!("MODE = {}", serde_json::to_string(mode).unwrap()),
        )
        .replace(
            "MARKER = \"\"",
            &format!(
                "MARKER = {}",
                serde_json::to_string(&root.join("started")).unwrap()
            ),
        );
    let script = if mode.starts_with("durable") {
        script.replace(
            "STATE = \"\"",
            &format!(
                "STATE = {}",
                serde_json::to_string(&root.join("backend.json")).unwrap()
            ),
        )
    } else {
        script
    };
    std::fs::write(package.join("bin/host"), script).unwrap();
    std::fs::set_permissions(
        package.join("bin/host"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    let package = package.canonicalize().unwrap();
    authorize(root, &package);
    package
}
fn authorize(root: &Path, package: &Path) -> ahu::plugin_binding::Binding {
    use ahu::plugin_binding::*;
    let digest = ahu::plugin_install::digest(package).unwrap();
    let binding = Binding {
        schema_version: 1,
        id: "fixture".into(),
        version: "0.1.0".into(),
        source: Source {
            repository: package.to_str().unwrap().into(),
            revision: digest.clone(),
            sha256: digest,
            subdirectory: None,
            kind: SourceKind::Local,
        },
        compatibility: Compatibility {
            ahu: env!("CARGO_PKG_VERSION").into(),
            host_api: 1,
        },
        enabled: true,
        grants: vec![Grant::InteractiveBackend],
    };
    common::git(root, &["init", "-q"]);
    save_binding(root, &binding);
    binding
}
fn save_binding(root: &Path, binding: &ahu::plugin_binding::Binding) {
    let directory = root.join(".agents/ahu/plugins");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join(format!("{}.md", binding.id)),
        ahu::okf::Document::new("ahu:plugin-binding", binding, "")
            .unwrap()
            .render()
            .unwrap(),
    )
    .unwrap();
    common::git(root, &["add", ".agents"]);
    common::git(
        root,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "synthetic binding",
        ],
    );
}
fn command(package: &Path) -> std::process::Command {
    let mut command = common::ahu();
    command
        .current_dir(package.parent().unwrap())
        .env("AHU_PLUGIN_PATH", package)
        .env("AHU_INTERACTIVE_PLUGIN", "fixture");
    command
}
fn marker(root: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(root.join("started")).unwrap()).unwrap()
}
fn assert_stopped(root: &Path) {
    let data = marker(root);
    let pid = data["pid"].as_i64().unwrap() as i32;
    let cwd_mode = data["cwd_mode"].as_u64().unwrap();
    assert_eq!(
        cwd_mode & 0o077,
        0,
        "snapshot directory is accessible to other users"
    );
    // SAFETY: signal zero only checks the synthetic process recorded by our fixture.
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "fixture child was not reaped"
    );
    assert!(
        !Path::new(data["cwd"].as_str().unwrap()).exists(),
        "executable snapshot was not removed"
    );
}

#[test]
fn executable_discovery_never_runs_and_explicit_probe_negotiates() {
    for mode in ["normal", "fragmented", "stderr_flood"] {
        let root = tempfile::tempdir().unwrap();
        let package = fixture(root.path(), mode);
        let output = command(&package)
            .args(["plugins", "--output", "json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(!root.path().join("started").exists());
        let output = command(&package)
            .env("SYNTHETIC_SECRET", "must-not-pass")
            .args(["plugins", "--probe", "--output", "json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{mode}: {output:?}");
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["plugin_id"], "fixture");
        assert_eq!(value["protocol_version"], 2);
        assert_eq!(value["persistent_sessions"], true);
        assert_eq!(
            marker(root.path())["argv"],
            serde_json::json!(["--ahu-host-protocol=2"])
        );
        assert!(marker(root.path())["env"].get("SYNTHETIC_SECRET").is_none());
        assert!(!root.path().join(".ahu").exists());
        assert_stopped(root.path());
    }
}

#[test]
fn handshake_and_framing_fail_closed_with_bounded_cleanup() {
    for mode in [
        "exit_hello",
        "hang_hello",
        "oversize",
        "zero",
        "partial_header",
        "partial_body",
        "garbage",
        "bad_negotiation",
        "wrong_identity",
        "missing_capability",
        "duplicate_capability",
        "wrong_id",
        "wrong_version",
        "unknown_envelope_field",
        "both_result_error",
    ] {
        let root = tempfile::tempdir().unwrap();
        let package = fixture(root.path(), mode);
        let started = Instant::now();
        let output = command(&package)
            .args(["plugins", "--probe"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{mode}: {output:?}");
        assert!(started.elapsed() < Duration::from_secs(5), "{mode}");
        assert!(output.stdout.is_empty());
        assert_stopped(root.path());
    }
}

#[test]
fn host_snapshot_is_private_with_permissive_umask() {
    const CASE: &str = "host_snapshot_is_private_with_permissive_umask";
    if common::is_child_case(CASE) {
        // Run the whole launch in a child process so this process-wide setting
        // cannot affect other tests or the test harness.
        unsafe {
            libc::umask(0);
        }
        let package = PathBuf::from(std::env::var("PLUGIN_PACKAGE").unwrap());
        let output = command(&package)
            .args(["plugins", "--probe", "--output", "json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let root = package.parent().unwrap();
        assert_eq!(marker(root)["cwd_mode"], 0o700);
        assert_stopped(root);
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let package = fixture(root.path(), "normal");
    let output = common::run_child_case(CASE, |command| {
        command.env("PLUGIN_PACKAGE", &package);
    });
    common::assert_child_passed(CASE, &output);
}

#[test]
fn durable_paths_require_committed_authority_before_execution() {
    let root = tempfile::tempdir().unwrap();
    let package = fixture(root.path(), "normal");
    let mut binding = authorize(root.path(), &package);
    for reason in ["disabled", "ungranted", "incompatible", "absent"] {
        binding.enabled = reason != "disabled";
        binding.grants = if reason == "ungranted" {
            vec![]
        } else {
            vec![ahu::plugin_binding::Grant::InteractiveBackend]
        };
        binding.compatibility.ahu = if reason == "incompatible" {
            "99.0.0".into()
        } else {
            env!("CARGO_PKG_VERSION").into()
        };
        save_binding(root.path(), &binding);
        if reason == "absent" {
            common::git(root.path(), &["rm", ".agents/ahu/plugins/fixture.md"]);
            common::git(
                root.path(),
                &[
                    "-c",
                    "core.hooksPath=/dev/null",
                    "commit",
                    "-qm",
                    "remove synthetic binding",
                ],
            );
        }
        let output = command(&package)
            .args(["plugins", "--probe"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{reason}: {output:?}");
        assert!(!root.path().join("started").exists());
    }
}

#[test]
fn executable_paths_and_metadata_are_validated_without_execution() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let package = fixture(root.path(), "normal");
    let manifest_path = package.join("ahu-plugin.md");
    let manifest = std::fs::read_to_string(&manifest_path).unwrap();
    for invalid in [
        manifest.replace("bin/host", "../host"),
        manifest.replace("bin/host", "/bin/sh"),
        manifest.replace("bin/host", "bin/../bin/host"),
        manifest.replace("protocol_version: 2", "protocol_version: 1"),
        manifest.replace("timeout_ms: 1000", "timeout_ms: 0"),
        manifest.replace("timeout_ms: 1000", "timeout_ms: 30001"),
        manifest.replace("  path:", "  arguments: [\"unsafe\"]\n  path:"),
    ] {
        std::fs::write(&manifest_path, invalid).unwrap();
        assert!(
            !command(&package)
                .arg("plugins")
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    std::fs::write(&manifest_path, manifest).unwrap();
    for target in ["bin/host", "bin"] {
        let saved = root.path().join("saved");
        std::fs::rename(package.join(target), &saved).unwrap();
        symlink(&saved, package.join(target)).unwrap();
        assert!(
            !command(&package)
                .arg("plugins")
                .output()
                .unwrap()
                .status
                .success()
        );
        std::fs::remove_file(package.join(target)).unwrap();
        std::fs::rename(saved, package.join(target)).unwrap();
    }
    std::fs::set_permissions(
        package.join("bin/host"),
        std::fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert!(
        !command(&package)
            .arg("plugins")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!root.path().join("started").exists());
}

#[test]
fn activation_rechecks_paths_after_discovery() {
    const CASE: &str = "activation_rechecks_paths_after_discovery";
    if common::is_child_case(CASE) {
        let selected = interactive::plugins::status().unwrap();
        let root = selected.manifest.as_ref().unwrap().parent().unwrap();
        let executable = root.join("bin/host");
        let saved = root.join("saved");
        std::fs::rename(&executable, &saved).unwrap();
        std::os::unix::fs::symlink(&saved, &executable).unwrap();
        assert!(ExternalHost::connect(&selected).is_err());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let package = fixture(root.path(), "normal");
    let output = common::run_child_case(CASE, |command| {
        command
            .current_dir(root.path())
            .env("AHU_PLUGIN_PATH", &package)
            .env("AHU_INTERACTIVE_PLUGIN", "fixture");
    });
    assert!(output.status.success(), "{output:?}");
    assert!(!root.path().join("started").exists());
}

#[test]
fn executable_size_special_files_and_setid_are_rejected_before_spawn() {
    let root = tempfile::tempdir().unwrap();
    let package = fixture(root.path(), "normal");
    let executable = package.join("bin/host");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o4700)).unwrap();
    if std::fs::metadata(&executable).unwrap().permissions().mode() & 0o6000 != 0 {
        assert!(
            !command(&package)
                .arg("plugins")
                .output()
                .unwrap()
                .status
                .success()
        );
    } else {
        eprintln!(
            "setid fixture skipped: this filesystem strips setid bits; production refusal remains enabled"
        );
    }
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&executable)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    assert!(
        !command(&package)
            .arg("plugins")
            .output()
            .unwrap()
            .status
            .success()
    );
    std::fs::remove_file(&executable).unwrap();
    let name = std::ffi::CString::new(executable.to_str().unwrap()).unwrap();
    // SAFETY: name is a disposable fixture path; no file exists there.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o700) }, 0);
    assert!(
        !command(&package)
            .arg("plugins")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!root.path().join("started").exists());
}

//! Offline peers exercise the actual selected executable, framing, and Host API.
mod common;
use ahu::interactive::{
    self,
    external::ExternalHost,
    v1::{Host, WorkspaceCreationError, WorkspaceRequest},
};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

fn fixture(root: &Path, mode: &str) -> PathBuf {
    let package = root.join("package ; $literal");
    std::fs::create_dir_all(package.join("bin")).unwrap();
    std::fs::write(
        package.join("Cargo.toml"),
        "[package]\nname = \"fixture-host\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(package.join("ahu-plugin.md"), "---\nokf_version: \"0.2\"\ntype: ahu:plugin\nschema_version: 1\nid: \"fixture\"\npackage: \"fixture-host\"\nhost_api: 1\ncargo_manifest: \"Cargo.toml\"\nexecutable:\n  path: \"bin/host\"\n  protocol_version: 1\n  timeout_ms: 1000\n---\n\n# Executable fixture\n").unwrap();
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
    std::fs::write(package.join("bin/host"), script).unwrap();
    std::fs::set_permissions(
        package.join("bin/host"),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    package.canonicalize().unwrap()
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
        assert_eq!(value["protocol_version"], 1);
        assert_eq!(value["persistent_sessions"], false);
        assert_eq!(
            marker(root.path())["argv"],
            serde_json::json!(["--ahu-host-protocol=1"])
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
fn host_operations_and_partial_start_classification() {
    const CASE: &str = "host_operations_and_partial_start_classification";
    if common::is_child_case(CASE) {
        let mode = std::env::var("FIXTURE_MODE").unwrap();
        let root = PathBuf::from(std::env::var_os("FIXTURE_ROOT").unwrap());
        let selected = interactive::plugins::status().unwrap();
        let host = ExternalHost::connect(&selected).unwrap();
        assert!(host.check_persistence().is_err());
        if mode == "duplicate_workspace" {
            assert!(host.workspaces().is_err());
            drop(host);
            assert_stopped(&root);
            return;
        }
        if mode == "normal" {
            let fixture_repo = common::TestRepo::new();
            let repo = ahu::git::discover(fixture_repo.path()).unwrap();
            let manager = interactive::repository::RepositoryManager::new(&host, &repo);
            assert!(
                manager
                    .ensure_group(&mut Vec::new())
                    .unwrap_err()
                    .to_string()
                    .contains("cannot own durable sessions")
            );
            assert!(
                manager
                    .group_coordinator("unused", "fixture", "fixture", "fixture", &[])
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("cannot own durable sessions")
            );
            assert!(matches!(
                manager.create_task_workspace("task", "", &root, "unused", false, &mut Vec::new()),
                Err(WorkspaceCreationError::NotCreated(_))
            ));
        }
        if mode != "backpressure" {
            host.create_group("fixture", &root).unwrap();
        }
        let startup = if matches!(mode.as_str(), "oversize_request" | "backpressure") {
            "x".repeat(if mode == "oversize_request" {
                1024 * 1024
            } else {
                800_000
            })
        } else {
            "literal ' $(false)".into()
        };
        if mode == "cancel_before" {
            host.cancel();
        }
        let cancel_thread = if mode == "cancel_create" {
            let token = host.cancellation_token();
            Some(std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(80));
                token.store(true, Ordering::Relaxed);
            }))
        } else {
            None
        };
        let started = Instant::now();
        let result = host.create_task_workspace(&WorkspaceRequest {
            group_id: "group",
            window_id: Some("window"),
            title: "task ' $literal",
            summary: "summary",
            cwd: &root,
            startup_command: &startup,
            focus: false,
        });
        assert!(started.elapsed() < Duration::from_secs(5));
        match mode.as_str() {
            "normal" => {
                let created = result.unwrap();
                assert_eq!(created.workspace_id, "workspace");
                assert_eq!(
                    host.workspaces().unwrap()["workspace"].title.as_deref(),
                    Some("task ' $literal")
                );
                host.set_status("workspace", "working").unwrap();
                host.set_agent_metadata("workspace", "fixture", "fixture", "fixture")
                    .unwrap();
                host.select_workspace("workspace").unwrap();
                host.close_workspace("workspace").unwrap();
                assert!(!host.workspaces().unwrap().contains_key("workspace"));
                // Replacing the source after activation cannot replace the pinned process.
                std::fs::write(
                    selected
                        .manifest
                        .unwrap()
                        .parent()
                        .unwrap()
                        .join("bin/host"),
                    "not an executable",
                )
                .unwrap();
                assert!(host.workspaces().is_ok());
            }
            "not_created" | "oversize_request" | "cancel_before" => assert!(
                matches!(result, Err(WorkspaceCreationError::NotCreated(_))),
                "{result:?}"
            ),
            _ => assert!(
                matches!(result, Err(WorkspaceCreationError::OutcomeUnknown(_))),
                "{mode}: {result:?}"
            ),
        }
        if let Some(thread) = cancel_thread {
            thread.join().unwrap();
        }
        if mode == "created_then_exit" {
            assert_eq!(
                std::fs::read_to_string(root.join("startup-observed")).unwrap(),
                "synthetic startup side effect"
            );
        }
        drop(host);
        assert_stopped(&root);
        return;
    }
    for mode in [
        "normal",
        "duplicate_workspace",
        "not_created",
        "unknown_create",
        "failed_create",
        "exit_create",
        "created_then_exit",
        "malformed_create",
        "hang_create",
        "wrong_cwd",
        "wrong_group",
        "wrong_window",
        "incomplete_inventory",
        "oversize_request",
        "backpressure",
        "cancel_before",
        "cancel_create",
    ] {
        let root = tempfile::tempdir().unwrap();
        let package = fixture(root.path(), mode);
        let root_path = root.path().canonicalize().unwrap();
        let output = common::run_child_case(CASE, |command| {
            command
                .env("AHU_PLUGIN_PATH", &package)
                .env("AHU_INTERACTIVE_PLUGIN", "fixture")
                .env("FIXTURE_MODE", mode)
                .env("FIXTURE_ROOT", &root_path);
        });
        assert!(output.status.success(), "{mode}: {output:?}");
    }
}

#[test]
fn durable_paths_refuse_external_identity_before_execution() {
    const CASE: &str = "durable_paths_refuse_external_identity_before_execution";
    if common::is_child_case(CASE) {
        for result in [
            interactive::discover_for_tasks(),
            interactive::discover_for_coordinator().map(|_| unreachable!()),
        ] {
            assert!(
                result
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("cannot own durable sessions")
            );
        }
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let package = fixture(root.path(), "normal");
    let output = common::run_child_case(CASE, |command| {
        command
            .env("AHU_PLUGIN_PATH", package)
            .env("AHU_INTERACTIVE_PLUGIN", "fixture");
    });
    assert!(output.status.success(), "{output:?}");
    assert!(!root.path().join("started").exists());
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
        manifest.replace("protocol_version: 1", "protocol_version: 2"),
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

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
        assert_eq!(value["protocol_version"], 1);
        assert_eq!(value["persistent_sessions"], true);
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
        assert!(host.check_persistence().is_ok());
        if mode == "duplicate_workspace" {
            assert!(host.workspaces().is_err());
            drop(host);
            assert_stopped(&root);
            return;
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
                .current_dir(root.path())
                .env("AHU_PLUGIN_PATH", &package)
                .env("AHU_INTERACTIVE_PLUGIN", "fixture")
                .env("FIXTURE_MODE", mode)
                .env("FIXTURE_ROOT", &root_path);
        });
        assert!(output.status.success(), "{mode}: {output:?}");
    }
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

fn installed_command(root: &Path) -> std::process::Command {
    let mut command = common::ahu();
    command
        .current_dir(root)
        .env_remove("AHU_PLUGIN_PATH")
        .env("AHU_INTERACTIVE_PLUGIN", "fixture")
        .env("AHU_CMUX_BIN", root.join("no-cmux"))
        .env("OTEL_SDK_DISABLED", "true");
    command
}
fn succeeds(output: std::process::Output) {
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
fn launch_fixture(
    mode: &str,
    discovered: bool,
) -> (
    common::TestRepo,
    tempfile::TempDir,
    PathBuf,
    PathBuf,
    ahu::task::TaskRecord,
) {
    let source = tempfile::tempdir().unwrap();
    let package = fixture(source.path(), mode);
    let binding = authorize(source.path(), &package);
    let repo = common::TestRepo::new();
    repo.init_config();
    repo.add_agent("worker", "1.0.0", "claude-opus-5");
    save_binding(repo.path(), &binding);
    repo.commit("synthetic launch context");
    if !discovered {
        succeeds(
            installed_command(repo.path())
                .args(["plugins", "install", "fixture"])
                .output()
                .unwrap(),
        );
    }
    let mut launch = installed_command(repo.path());
    launch.env_remove("AHU_INTERACTIVE_PLUGIN");
    if discovered {
        launch.env("AHU_PLUGIN_PATH", &package);
    }
    let output = launch
        .args(["@worker", "--prompt", "synthetic task"])
        .output()
        .unwrap();
    if mode == "durable_uncertain" {
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("may have started"),
            "{output:?}"
        );
    } else {
        succeeds(output);
    }
    let git = ahu::git::discover(repo.path()).unwrap();
    let listing = ahu::task::list(&git).unwrap();
    assert_eq!(listing.records.len(), 1, "{listing:?}");
    let (dir, record) = listing.records.into_iter().next().unwrap();
    assert!(record.session_host.is_some());
    assert!(record.worktree.exists());
    (repo, source, package, dir, record)
}

#[test]
fn public_launch_focus_cancel_remove_and_uncertain_creation() {
    const CASE: &str = "public_launch_focus_cancel_remove_and_uncertain_creation";
    if !common::in_harness_fixture(CASE) {
        return;
    }
    for discovered in [false, true] {
        let (repo, source, package, dir, record) = launch_fixture("durable", discovered);
        let frozen = record.session_host.as_ref().unwrap();
        assert_eq!(frozen.discovered_root.is_some(), discovered);
        assert_eq!(frozen.binding.version, "0.1.0");
        assert_eq!(record.schema_version, 5);
        let task_path = dir.join("task.json");
        let saved_record = std::fs::read(&task_path).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&saved_record).unwrap();
        value.as_object_mut().unwrap().remove("session_host");
        std::fs::write(&task_path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(ahu::task::load(&dir).is_err());
        value["schema_version"] = 4.into();
        std::fs::write(&task_path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(ahu::task::load(&dir).unwrap().session_host.is_none());
        value["session_host"] = serde_json::to_value(frozen).unwrap();
        std::fs::write(&task_path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(ahu::task::load(&dir).is_err());
        std::fs::write(&task_path, saved_record).unwrap();

        assert!(!repo.path().join(".ahu/state/cmux.json").exists());
        // No discovery environment is needed to address a recorded task.
        succeeds(
            installed_command(repo.path())
                .args(["focus", &record.task_id])
                .output()
                .unwrap(),
        );
        let git = ahu::git::discover(repo.path()).unwrap();
        let host = interactive::discover_for_record(&git, &record).unwrap();
        let manager = interactive::repository::RepositoryManager::new(host.as_ref(), &git);
        let mut forged = record.clone();
        forged.session_host = None;
        assert!(manager.close_task_workspace(&forged).is_err());
        forged = record.clone();
        forged.cmux_group_id = Some("someone-else".into());
        assert!(manager.select_task_workspace(&forged).is_err());
        forged = record.clone();
        forged.worktree = repo.path().to_path_buf();
        assert!(manager.close_task_workspace(&forged).is_err());
        drop(host);
        // Missing discovered packages fail before any host execution.
        if discovered {
            let saved = package.with_extension("saved");
            std::fs::rename(&package, &saved).unwrap();
            let output = installed_command(repo.path())
                .args(["focus", &record.task_id])
                .output()
                .unwrap();
            assert!(!output.status.success());
            std::fs::rename(saved, &package).unwrap();
        } else {
            // Installed selection also works without AHU_PLUGIN_PATH or an environment selector.
            succeeds(
                installed_command(repo.path())
                    .env_remove("AHU_INTERACTIVE_PLUGIN")
                    .args(["plugins", "--probe"])
                    .output()
                    .unwrap(),
            );
        }
        // Core cancellation requires a durable supervisor confirmation before close.
        ahu::task::set_state(&dir, ahu::task::TaskState::Running).unwrap();
        let mut cancel = installed_command(repo.path())
            .args(["cancel", &record.task_id, "--output", "json"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        while !dir.join("cancel.json").exists() {
            assert!(Instant::now() < deadline);
            assert!(cancel.try_wait().unwrap().is_none());
            std::thread::sleep(Duration::from_millis(10));
        }
        let backend: serde_json::Value =
            serde_json::from_slice(&std::fs::read(source.path().join("backend.json")).unwrap())
                .unwrap();
        assert!(backend["workspaces"].get("workspace").is_some());
        ahu::task::set_state(&dir, ahu::task::TaskState::Cancelled).unwrap();
        succeeds(cancel.wait_with_output().unwrap());
        let backend: serde_json::Value =
            serde_json::from_slice(&std::fs::read(source.path().join("backend.json")).unwrap())
                .unwrap();
        assert!(backend["workspaces"].get("workspace").is_none());
        succeeds(
            installed_command(repo.path())
                .args(["remove", &record.task_id])
                .output()
                .unwrap(),
        );
        assert!(!record.worktree.exists());
    }
    // Terminal removal itself closes a still-present host workspace.
    let (repo, source, _package, dir, record) = launch_fixture("durable", false);
    succeeds(
        installed_command(repo.path())
            .args(["run-task", "--task-dir", dir.to_str().unwrap()])
            .output()
            .unwrap(),
    );
    assert_eq!(
        ahu::task::load(&dir).unwrap().state,
        ahu::task::TaskState::Exited
    );
    succeeds(
        installed_command(repo.path())
            .args(["remove", &record.task_id])
            .output()
            .unwrap(),
    );
    let backend: serde_json::Value =
        serde_json::from_slice(&std::fs::read(source.path().join("backend.json")).unwrap())
            .unwrap();
    assert!(backend["workspaces"].get("workspace").is_none());

    let (_repo, source, _package, dir, record) = launch_fixture("durable_uncertain", false);
    assert!(record.cmux_workspace_id.is_none());
    assert!(dir.join("prompt.txt").exists());
    let backend: serde_json::Value =
        serde_json::from_slice(&std::fs::read(source.path().join("backend.json")).unwrap())
            .unwrap();
    assert!(backend["workspaces"].get("workspace").is_some());
}

#[test]
fn frozen_versions_survive_updates_and_tampering_refuses_before_execution() {
    const CASE: &str = "frozen_versions_survive_updates_and_tampering_refuses_before_execution";
    if !common::in_harness_fixture(CASE) {
        return;
    }
    let (repo, source, package, dir, record) = launch_fixture("durable", false);
    let frozen = record.session_host.clone().unwrap();
    let store = repo.path().join(".ahu/state/plugins/fixture");
    let old = std::fs::read_dir(&store)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let executable = old.join("package/bin/host");
    let original_bytes = std::fs::read(&executable).unwrap();
    std::fs::write(
        package.join("Cargo.toml"),
        "[package]\nname = 'fixture-host'\nversion = '0.2.0'\n",
    )
    .unwrap();
    let mut new = frozen.binding.clone();
    new.version = "0.2.0".into();
    new.source.sha256 = ahu::plugin_install::digest(&package).unwrap();
    new.source.revision = new.source.sha256.clone();
    save_binding(repo.path(), &new);
    succeeds(
        installed_command(repo.path())
            .args(["plugins", "update", "fixture"])
            .output()
            .unwrap(),
    );
    assert_eq!(std::fs::read_dir(&store).unwrap().count(), 2);
    succeeds(
        installed_command(repo.path())
            .args(["focus", &record.task_id])
            .output()
            .unwrap(),
    );
    assert_eq!(
        ahu::task::load(&dir).unwrap().session_host,
        Some(frozen.clone())
    );
    let output = installed_command(repo.path())
        .args(["plugins", "disable", "fixture"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("live"));
    assert!(executable.exists());
    let marker = source.path().join("started");
    std::fs::remove_file(&marker).unwrap();
    std::fs::write(&executable, "#!/bin/sh\nexit 99\n").unwrap();
    let output = installed_command(repo.path())
        .args(["focus", &record.task_id])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!marker.exists());
    // The fixed startup entrypoint also refuses the tampered package before a harness starts.
    let output = installed_command(repo.path())
        .args(["run-task", "--task-dir", dir.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("pin mismatch"),
        "{output:?}"
    );
    std::fs::write(&executable, &original_bytes).unwrap();
    let git = ahu::git::discover(repo.path()).unwrap();
    for change in ["digest", "protocol", "package", "source", "revision"] {
        let mut altered = record.clone();
        let pin = altered.session_host.as_mut().unwrap();
        match change {
            "digest" => pin.executable_sha256 = "0".repeat(64),
            "protocol" => pin.executable.protocol_version = 2,
            "package" => pin.package = "different".into(),
            "source" => pin.binding.source.sha256 = "0".repeat(64),
            _ => pin.policy_revision = "0".repeat(40),
        }
        assert!(
            interactive::discover_for_record(&git, &altered).is_err(),
            "{change}"
        );
        assert!(!marker.exists());
    }
    // Committed revocation is authoritative even though the old receipt grants access.
    new.enabled = false;
    save_binding(repo.path(), &new);
    assert!(interactive::discover_for_record(&git, &record).is_err());
    assert!(!marker.exists());
    let output = installed_command(repo.path())
        .args(["plugins", "remove", "fixture"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(executable.exists());
}

#[test]
fn terminal_package_cleanup_requires_task_removal_first() {
    const CASE: &str = "terminal_package_cleanup_requires_task_removal_first";
    if !common::in_harness_fixture(CASE) {
        return;
    }
    for already_closed in [false, true] {
        let (repo, source, _package, dir, record) = launch_fixture("durable", false);
        succeeds(
            installed_command(repo.path())
                .args(["run-task", "--task-dir", dir.to_str().unwrap()])
                .output()
                .unwrap(),
        );
        assert_eq!(
            ahu::task::load(&dir).unwrap().state,
            ahu::task::TaskState::Exited
        );
        if already_closed {
            let git = ahu::git::discover(repo.path()).unwrap();
            interactive::discover_for_record(&git, &record)
                .unwrap()
                .close_workspace(record.cmux_workspace_id.as_deref().unwrap())
                .unwrap();
        }
        let marker = source.path().join("started");
        std::fs::remove_file(&marker).unwrap();
        let binding_path = repo.path().join(".agents/ahu/plugins/fixture.md");
        let original = std::fs::read(&binding_path).unwrap();
        for action in ["disable", "remove"] {
            let output = installed_command(repo.path())
                .args(["plugins", action, "fixture"])
                .output()
                .unwrap();
            assert!(!output.status.success(), "{action}: {output:?}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("workspace cleanup"),
                "{output:?}"
            );
            assert_eq!(std::fs::read(&binding_path).unwrap(), original);
            assert!(
                !marker.exists(),
                "package lifecycle must not execute the host"
            );
        }
        // Direct committed revocation still fails closed; removal retains the pin.
        let mut binding = record.session_host.as_ref().unwrap().binding.clone();
        binding.enabled = false;
        save_binding(repo.path(), &binding);
        let output = installed_command(repo.path())
            .args(["remove", &record.task_id])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("committed enablement"),
            "{output:?}"
        );
        let output = installed_command(repo.path())
            .args(["plugins", "remove", "fixture"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("workspace cleanup"),
            "{output:?}"
        );
        assert!(!marker.exists());
        assert!(dir.exists());
        assert!(repo.path().join(".ahu/state/plugins/fixture").exists());
        // Restore explicit authority, remove the terminal task, then disable/remove.
        binding.enabled = true;
        save_binding(repo.path(), &binding);
        let output = installed_command(repo.path())
            .args(["remove", &record.task_id])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(if already_closed {
                "already absent"
            } else {
                "closed"
            }),
            "{output:?}"
        );
        succeeds(output);
        assert!(!record.worktree.exists());
        succeeds(
            installed_command(repo.path())
                .args(["plugins", "disable", "fixture"])
                .output()
                .unwrap(),
        );
        common::git(repo.path(), &["add", ".agents/ahu/plugins/fixture.md"]);
        common::git(
            repo.path(),
            &["commit", "-qm", "disable fixture after task cleanup"],
        );
        succeeds(
            installed_command(repo.path())
                .args(["plugins", "remove", "fixture"])
                .output()
                .unwrap(),
        );
        assert!(!repo.path().join(".ahu/state/plugins/fixture").exists());
    }
}

#[test]
fn installed_inventory_agrees_with_activation_selection_without_execution() {
    const CASE: &str = "installed_inventory_agrees_with_activation_selection_without_execution";
    if !common::in_harness_fixture(CASE) {
        return;
    }
    let (repo, source, _package, _dir, record) = launch_fixture("durable", false);
    for explicit in [false, true] {
        let marker = source.path().join("started");
        std::fs::remove_file(&marker).unwrap();
        let mut inventory = installed_command(repo.path());
        let mut probe = installed_command(repo.path());
        if !explicit {
            inventory.env_remove("AHU_INTERACTIVE_PLUGIN");
            probe.env_remove("AHU_INTERACTIVE_PLUGIN");
        }
        let output = inventory
            .args(["plugins", "--output", "json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let selected: Vec<_> = value["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["selected"] == true)
            .collect();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0]["id"], "fixture");
        assert_eq!(selected[0]["package"], "fixture-host");
        assert_eq!(
            selected[0]["selection_source"],
            if explicit {
                "environment"
            } else {
                "committed-project-binding"
            }
        );
        assert!(!marker.exists());
        let output = probe
            .args(["plugins", "--probe", "--output", "json"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["plugin_id"], selected[0]["id"]);
        assert_eq!(
            record.session_host.as_ref().unwrap().binding.id,
            selected[0]["id"]
        );
    }
    let mut second = record.session_host.as_ref().unwrap().binding.clone();
    second.id = "second".into();
    save_binding(repo.path(), &second);
    succeeds(
        installed_command(repo.path())
            .args(["lock", "--update"])
            .output()
            .unwrap(),
    );
    common::git(repo.path(), &["add", ".agents/ahu/lock.md"]);
    common::git(
        repo.path(),
        &["commit", "-qm", "lock second fixture binding"],
    );
    let marker = source.path().join("started");
    std::fs::remove_file(&marker).unwrap();
    for args in [
        vec!["plugins"],
        vec!["plugins", "--probe"],
        vec!["@worker", "--prompt", "ambiguous host"],
    ] {
        let output = installed_command(repo.path())
            .env_remove("AHU_INTERACTIVE_PLUGIN")
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("multiple enabled session hosts"),
            "{output:?}"
        );
        assert!(!marker.exists());
    }
    // Explicit selection resolves ambiguity without needing the other installation.
    succeeds(
        installed_command(repo.path())
            .arg("plugins")
            .output()
            .unwrap(),
    );
    // Selection remains inspectable without a verified installation.
    let output = installed_command(repo.path())
        .env("AHU_INTERACTIVE_PLUGIN", "second")
        .args(["plugins", "--output", "json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let selected = value["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["selected"] == true)
        .unwrap();
    assert_eq!(selected["id"], "second");
    assert_eq!(selected["origin"], "committed-binding");
    assert!(selected["activation_blocker"].is_string());
    let output = installed_command(repo.path())
        .env("AHU_INTERACTIVE_PLUGIN", "second")
        .args(["plugins", "--probe"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!marker.exists());
}

#[test]
fn doctor_attributes_external_reachability_separately_from_native_cmux() {
    const CASE: &str = "doctor_attributes_external_reachability_separately_from_native_cmux";
    if !common::in_harness_fixture(CASE) {
        return;
    }
    let (repo, _source, _package, _dir, _record) = launch_fixture("durable", false);
    let output = installed_command(repo.path())
        .env_remove("AHU_INTERACTIVE_PLUGIN")
        .arg("doctor")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("session host fixture (fixture-host)"),
        "{output:?}"
    );
    assert!(stdout.contains("host         reachable"), "{output:?}");
    assert!(stdout.contains("native cmux  unavailable"), "{output:?}");
    assert!(!stdout.contains("cmux         reachable"), "{output:?}");
}

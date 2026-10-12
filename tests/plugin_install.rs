//! Bounded lifecycle tests: only synthetic local packages and Git repositories.
mod common;
use ahu::plugin_binding::{Binding, Compatibility, Grant, Source, SourceKind};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

fn temp() -> tempfile::TempDir {
    let root = fs::canonicalize(std::env::temp_dir()).unwrap();
    tempfile::tempdir_in(root).unwrap()
}
fn write(root: &Path, relative: &str, bytes: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}
fn package(root: &Path) {
    write(
        root,
        "ahu-plugin.md",
        "---\nokf_version: '0.2'\ntype: ahu:plugin\nschema_version: 1\nid: fixture\npackage: fixture-package\nhost_api: 1\ncargo_manifest: Cargo.toml\n---\n$(touch never-executed)\n",
    );
    write(
        root,
        "Cargo.toml",
        "[package]\nname = 'fixture-package'\nversion = '1.0.0'\nbuild = 'build.rs'\n",
    );
    write(
        root,
        "build.rs",
        "compile_error!(\"package code must never run\");\n",
    );
}
fn commit(root: &Path) -> String {
    common::git(root, &["add", "-f", "."]);
    common::git(
        root,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "synthetic fixture",
        ],
    );
    common::git(root, &["rev-parse", "HEAD"])
}
fn source() -> tempfile::TempDir {
    let source = temp();
    common::git(source.path(), &["init", "-q"]);
    package(&source.path().join("packages/fixture"));
    commit(source.path());
    source
}
fn binding(source: &Path) -> Binding {
    Binding {
        schema_version: 1,
        id: "fixture".into(),
        version: "1.0.0".into(),
        source: Source {
            kind: SourceKind::Git,
            repository: source.to_str().unwrap().into(),
            revision: common::git(source, &["rev-parse", "HEAD"]),
            sha256: ahu::plugin_install::digest(&source.join("packages/fixture")).unwrap(),
            subdirectory: Some("packages/fixture".into()),
        },
        compatibility: Compatibility {
            ahu: env!("CARGO_PKG_VERSION").into(),
            host_api: 1,
        },
        enabled: false,
        grants: vec![],
    }
}
fn save(root: &Path, binding: &Binding) {
    let mut doc =
        ahu::okf::Document::new("ahu:plugin-binding", binding, "\nKeep this explanation.\n")
            .unwrap();
    doc.fields
        .insert("custom".into(), serde_json::json!({"preserved": true}));
    write(
        root,
        ".agents/ahu/plugins/fixture.md",
        &doc.render().unwrap(),
    );
}
fn project(binding: &Binding) -> tempfile::TempDir {
    let repo = temp();
    common::git(repo.path(), &["init", "-q"]);
    save(repo.path(), binding);
    commit(repo.path());
    repo
}
fn run(root: &Path, action: &str) -> Output {
    common::ahu()
        .current_dir(root)
        .args(["plugins", action, "fixture"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("OTEL_SDK_DISABLED", "true")
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
fn failure(output: Output, message: &str) {
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(message),
        "{output:?}"
    );
}
fn versions(root: &Path) -> Vec<PathBuf> {
    let dir = root.join(".ahu/state/plugins/fixture");
    if !dir.exists() {
        return vec![];
    }
    let mut paths = fs::read_dir(dir)
        .unwrap()
        .map(|p| p.unwrap().path())
        .collect::<Vec<_>>();
    paths.sort();
    paths
}
fn commit_binding(root: &Path) {
    // Never include the private installation state in a fixture commit.
    common::git(root, &["add", ".agents"]);
    common::git(
        root,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "synthetic binding update",
        ],
    );
}

#[test]
fn git_install_is_verified_private_inert_and_idempotent() {
    let source = source();
    let binding = binding(source.path());
    let repo = project(&binding);
    let before = fs::read(repo.path().join(".agents/ahu/plugins/fixture.md")).unwrap();
    success(run(repo.path(), "install"));
    success(run(repo.path(), "install"));
    let paths = versions(repo.path());
    assert_eq!(paths.len(), 1);
    assert_eq!(
        ahu::plugin_install::digest(&paths[0].join("package")).unwrap(),
        binding.source.sha256
    );
    assert_eq!(
        before,
        fs::read(repo.path().join(".agents/ahu/plugins/fixture.md")).unwrap()
    );
    for path in [
        repo.path().join(".ahu"),
        repo.path().join(".ahu/state/plugins"),
        paths[0].clone(),
    ] {
        assert_eq!(fs::metadata(path).unwrap().permissions().mode() & 0o077, 0);
    }
    assert_eq!(
        fs::metadata(paths[0].join("receipt.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o077,
        0
    );
    assert!(!paths[0].join("git").exists());
    assert!(!repo.path().join("never-executed").exists());
    assert!(
        common::git(
            repo.path(),
            &["status", "--porcelain", "--untracked-files=all"]
        )
        .is_empty()
    );
    assert!(!common::git(repo.path(), &["check-ignore", ".ahu/state/plugins/fixture"]).is_empty());
}

#[test]
fn local_snapshot_has_a_content_revision_and_does_not_follow_source_edits() {
    let source = temp();
    package(source.path());
    let pin = ahu::plugin_install::digest(source.path()).unwrap();
    let mut binding = Binding {
        schema_version: 1,
        id: "fixture".into(),
        version: "1.0.0".into(),
        source: Source {
            kind: SourceKind::Local,
            repository: source.path().to_str().unwrap().into(),
            revision: pin.clone(),
            sha256: pin.clone(),
            subdirectory: None,
        },
        compatibility: Compatibility {
            ahu: env!("CARGO_PKG_VERSION").into(),
            host_api: 1,
        },
        enabled: false,
        grants: vec![],
    };
    let repo = project(&binding);
    let output = common::ahu()
        .args(["plugins", "digest", source.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(success(output).trim(), pin);
    success(run(repo.path(), "install"));
    write(source.path(), "new", "changed source");
    assert_eq!(
        ahu::plugin_install::digest(&versions(repo.path())[0].join("package")).unwrap(),
        pin
    );
    binding.source.sha256 = ahu::plugin_install::digest(source.path()).unwrap();
    binding.source.revision = binding.source.sha256.clone();
    save(repo.path(), &binding);
    commit_binding(repo.path());
    failure(run(repo.path(), "install"), "use plugins update");
    success(run(repo.path(), "update"));
    assert_eq!(versions(repo.path()).len(), 2);
}

#[test]
fn pin_failure_is_atomic_and_retry_recovers_without_replacing_old_versions() {
    let source = source();
    let mut binding = binding(source.path());
    let repo = project(&binding);
    success(run(repo.path(), "install"));
    let old = versions(repo.path());
    write(source.path(), "packages/fixture/new", "new data");
    binding.source.revision = commit(source.path());
    // Keep the old content pin to exercise a failure after acquisition.
    save(repo.path(), &binding);
    commit_binding(repo.path());
    failure(run(repo.path(), "update"), "pin mismatch");
    assert_eq!(versions(repo.path()), old);
    binding.source.sha256 =
        ahu::plugin_install::digest(&source.path().join("packages/fixture")).unwrap();
    save(repo.path(), &binding);
    commit_binding(repo.path());
    success(run(repo.path(), "update"));
    assert_eq!(versions(repo.path()).len(), 2);
    assert!(old[0].exists());
    // A crashed staging directory cannot become an installed version or block update.
    write(
        repo.path(),
        ".ahu/state/plugins/fixture/.install-crashed/partial",
        "incomplete",
    );
    success(run(repo.path(), "update"));
    success(run(repo.path(), "remove"));
    assert!(versions(repo.path()).is_empty());
}

#[test]
fn enable_disable_preserve_okf_and_require_committed_disable_before_remove() {
    let source = source();
    let binding = binding(source.path());
    let repo = project(&binding);
    failure(run(repo.path(), "enable"), "receipt");
    success(run(repo.path(), "install"));
    success(run(repo.path(), "enable"));
    let (enabled, doc) = ahu::plugin_binding::load(repo.path(), "fixture")
        .unwrap()
        .unwrap();
    assert!(enabled.enabled);
    assert_eq!(doc.fields["custom"]["preserved"], true);
    assert_eq!(doc.body, "\nKeep this explanation.\n");
    failure(
        run(repo.path(), "remove"),
        "staged or unstaged binding changes",
    );
    commit_binding(repo.path());
    success(run(repo.path(), "disable"));
    failure(
        run(repo.path(), "remove"),
        "staged or unstaged binding changes",
    );
    commit_binding(repo.path());
    success(run(repo.path(), "remove"));
    success(run(repo.path(), "remove"));
    assert!(repo.path().join(".agents/ahu/plugins/fixture.md").exists());
}

#[test]
fn manifest_identity_and_version_fail_without_publication() {
    for (file, old, new) in [
        ("ahu-plugin.md", "id: fixture", "id: other"),
        ("Cargo.toml", "version = '1.0.0'", "version = '2.0.0'"),
        ("Cargo.toml", "fixture-package", "other-package"),
    ] {
        let source = source();
        let path = source.path().join("packages/fixture").join(file);
        fs::write(&path, fs::read_to_string(&path).unwrap().replace(old, new)).unwrap();
        commit(source.path());
        let binding = binding(source.path());
        let repo = project(&binding);
        assert!(!run(repo.path(), "install").status.success());
        assert!(versions(repo.path()).is_empty());
    }
}

#[test]
fn unsafe_git_entries_missing_pins_and_wrong_object_types_fail_closed() {
    for case in ["symlink", "submodule", "missing", "tree", "subdirectory"] {
        let source = source();
        let mut binding = binding(source.path());
        match case {
            "symlink" => {
                std::os::unix::fs::symlink(
                    "../../outside",
                    source.path().join("packages/fixture/link"),
                )
                .unwrap();
                binding.source.revision = commit(source.path());
            }
            "submodule" => {
                common::git(
                    source.path(),
                    &[
                        "update-index",
                        "--add",
                        "--cacheinfo",
                        &format!("160000,{},packages/fixture/module", binding.source.revision),
                    ],
                );
                common::git(
                    source.path(),
                    &[
                        "-c",
                        "core.hooksPath=/dev/null",
                        "-c",
                        "commit.gpgsign=false",
                        "commit",
                        "-qm",
                        "synthetic gitlink",
                    ],
                );
                binding.source.revision = common::git(source.path(), &["rev-parse", "HEAD"]);
            }
            "missing" => binding.source.revision = "a".repeat(40),
            "tree" => {
                binding.source.revision = common::git(source.path(), &["rev-parse", "HEAD^{tree}"])
            }
            "subdirectory" => binding.source.subdirectory = Some("missing".into()),
            _ => unreachable!(),
        }
        let repo = project(&binding);
        assert!(!run(repo.path(), "install").status.success(), "{case}");
        assert!(versions(repo.path()).is_empty(), "{case}");
    }
}

#[test]
fn local_source_and_state_symlinks_and_tampered_installs_are_refused() {
    let source = source();
    let binding = binding(source.path());
    let repo = project(&binding);
    let outside = temp();
    std::os::unix::fs::symlink(outside.path(), repo.path().join(".ahu")).unwrap();
    failure(run(repo.path(), "install"), "symlink");
    assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
    fs::remove_file(repo.path().join(".ahu")).unwrap();
    let objects = source.path().join(".git/objects");
    fs::rename(&objects, source.path().join("objects-moved")).unwrap();
    std::os::unix::fs::symlink(source.path().join("objects-moved"), &objects).unwrap();
    failure(run(repo.path(), "install"), "symlink");
    fs::remove_file(&objects).unwrap();
    fs::rename(source.path().join("objects-moved"), &objects).unwrap();
    success(run(repo.path(), "install"));
    write(&versions(repo.path())[0], "package/build.rs", "tampered");
    failure(run(repo.path(), "install"), "pin mismatch");
    failure(run(repo.path(), "enable"), "pin mismatch");
    success(run(repo.path(), "remove"));
    success(run(repo.path(), "install"));
}

#[test]
fn hostile_source_git_config_filters_and_replacements_are_never_used() {
    let source = source();
    write(
        source.path(),
        "packages/fixture/.gitattributes",
        "* filter=fixture\n",
    );
    commit(source.path());
    let binding = binding(source.path());
    let repo = project(&binding);
    let marker = source.path().join("executed");
    let script = format!("touch {}", common::shell_quoted(marker.to_str().unwrap()));
    common::git(
        source.path(),
        &["config", "uploadpack.packObjectsHook", &script],
    );
    common::git(source.path(), &["config", "core.sshCommand", &script]);
    common::git(
        source.path(),
        &["config", "credential.helper", &format!("!{script}")],
    );
    common::git(source.path(), &["config", "filter.fixture.smudge", &script]);
    common::git(
        source.path(),
        &["config", "core.hooksPath", source.path().to_str().unwrap()],
    );
    let oid = common::git(
        source.path(),
        &["rev-parse", "HEAD:packages/fixture/ahu-plugin.md"],
    );
    write(source.path(), "replacement", "invalid replacement");
    let replacement = common::git(source.path(), &["hash-object", "-w", "replacement"]);
    common::git(source.path(), &["replace", &oid, &replacement]);
    success(run(repo.path(), "install"));
    assert!(!marker.exists());
    assert_eq!(
        ahu::plugin_install::digest(&versions(repo.path())[0].join("package")).unwrap(),
        binding.source.sha256
    );
}

#[test]
fn alternate_objects_and_unsafe_declarations_are_rejected_without_transport() {
    let source = source();
    let mut binding = binding(source.path());
    let repo = project(&binding);
    write(
        source.path(),
        ".git/objects/info/alternates",
        "/unavailable/objects\n",
    );
    failure(run(repo.path(), "install"), "alternates");
    assert!(versions(repo.path()).is_empty());
    for repository in [
        "ext::never-run",
        "ssh://example.invalid/repo",
        "file:///private/tmp/source",
        "https://user:secret@example.invalid/repo",
        "https://example.invalid/repo?secret",
    ] {
        binding.source.repository = repository.into();
        save(repo.path(), &binding);
        commit_binding(repo.path());
        let output = run(repo.path(), "install");
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("secret"));
    }
    binding.source.repository = source.path().to_str().unwrap().into();
    for sub in ["../escape", "/absolute", "a//b", ".git", "a/./b", "a\\b"] {
        binding.source.subdirectory = Some(sub.into());
        save(repo.path(), &binding);
        commit_binding(repo.path());
        failure(run(repo.path(), "install"), "plain relative path");
    }
}

#[test]
fn uncertain_tasks_block_cleanup_but_not_non_destructive_install() {
    let source = source();
    let binding = binding(source.path());
    let repo = project(&binding);
    let discovered = ahu::git::discover(repo.path()).unwrap();
    write(
        repo.path(),
        &format!(
            ".ahu/state/repos/{}/tasks/fixture/task.json",
            discovered.identity()
        ),
        "malformed",
    );
    success(run(repo.path(), "install"));
    failure(run(repo.path(), "disable"), "state is uncertain");
    failure(run(repo.path(), "remove"), "state is uncertain");
    assert_eq!(versions(repo.path()).len(), 1);
}

fn record(root: &Path, id: &str) -> std::path::PathBuf {
    let discovered = ahu::git::discover(root).unwrap();
    let record_source = root.to_path_buf();
    let adapter = ahu::harness::adapter_for("claude-code").unwrap();
    let command = adapter
        .launch_command(&ahu::harness::LaunchRequest {
            model: "claude-opus-5",
            prompt: "secret prompt",
            cwd: &record_source,
            permissions: Default::default(),
        })
        .unwrap();
    let enforcement = adapter
        .enforcement("claude-opus-5", Default::default())
        .unwrap();
    let record = ahu::task::TaskRecord {
        schema_version: ahu::task::TASK_SCHEMA_VERSION,
        task_id: id.to_string(),
        title: "private prompt title".to_string(),
        summary: String::new(),
        created_at: ahu::task::now_rfc3339(),
        repo_identity: discovered.identity(),
        repo_root: record_source.clone(),
        branch: "ahu/auto/gone0001".to_string(),
        worktree: record_source.clone(),
        base_commit: discovered.head.clone(),
        identity: ahu::task::LaunchIdentity {
            release_id: None,
            mode: ahu::task::LaunchMode::Automatic,
            agent: "auto".to_string(),
            agent_version: None,
            permissions: Default::default(),
            harness: "claude-code".to_string(),
            model: "claude-opus-5".to_string(),
            instructions_source: None,
            source_digest: None,
            instructions_digest: None,
            identity_digest: None,
            selection_basis: Some("test".to_string()),
        },
        policy_digest: "0".repeat(64),
        catalog_version: ahu::catalog::CATALOG_VERSION.to_string(),
        config_snapshot: Default::default(),
        config_snapshot_digest: "0".repeat(64),
        hooks: Default::default(),
        hooks_digest: String::new(),
        delivery: ahu::orchestration::deliver(None, "prompt").unwrap().1,
        prompt_digest: String::new(),
        harness_executable: std::path::PathBuf::from("claude"),
        materialize: Default::default(),
        launch_command: command,
        reliability_warning: None,
        enforcement,
        session_host: None,
        cmux_group_id: None,
        cmux_workspace_id: None,
        cmux_window_id: None,
        state: ahu::task::TaskState::Running,
    };
    let dir = root
        .join(".ahu/state/repos")
        .join(discovered.identity())
        .join("tasks")
        .join(id);
    ahu::task::save(&dir, &record, "secret prompt").unwrap();
    dir
}

#[test]
fn running_tasks_retain_versions_and_terminal_tasks_allow_cleanup() {
    let source = source();
    let mut binding = binding(source.path());
    let repo = project(&binding);
    success(run(repo.path(), "install"));
    let task_dir = record(repo.path(), "fixture-live");
    failure(run(repo.path(), "disable"), "tasks are live");
    failure(run(repo.path(), "remove"), "tasks are live");
    write(source.path(), "packages/fixture/new", "new version");
    binding.source.revision = commit(source.path());
    binding.source.sha256 =
        ahu::plugin_install::digest(&source.path().join("packages/fixture")).unwrap();
    save(repo.path(), &binding);
    commit_binding(repo.path());
    success(run(repo.path(), "update"));
    assert_eq!(versions(repo.path()).len(), 2);
    ahu::task::set_state(&task_dir, ahu::task::TaskState::Exited).unwrap();
    success(run(repo.path(), "remove"));
    assert!(versions(repo.path()).is_empty());
}

#[test]
fn terminal_external_workspace_task_retains_its_plugin_until_removal() {
    let source = source();
    let mut binding = binding(source.path());
    binding.enabled = true;
    binding.grants = vec![Grant::InteractiveBackend];
    let repo = project(&binding);
    success(run(repo.path(), "install"));

    let task_dir = record(repo.path(), "terminal-host");
    let mut task = ahu::task::load(&task_dir).unwrap();
    task.state = ahu::task::TaskState::Exited;
    task.session_host = Some(ahu::interactive::binding::FrozenHost {
        binding: binding.clone(),
        policy_revision: common::git(repo.path(), &["rev-parse", "HEAD"]),
        package: "fixture-package".into(),
        executable: ahu::interactive::external::Executable {
            path: "bin/host".into(),
            protocol_version: 1,
            timeout_ms: 5_000,
        },
        executable_sha256: "0".repeat(64),
        discovered_root: None,
    });
    task.cmux_workspace_id = Some("already-closed-workspace".into());
    ahu::task::save(&task_dir, &task, "synthetic prompt").unwrap();

    failure(
        run(repo.path(), "disable"),
        "recorded tasks still require this package for workspace cleanup",
    );

    // Reproduce a previously disabled binding as well: cleanup still refuses
    // to remove the exact host package needed to inspect the recorded workspace.
    binding.enabled = false;
    save(repo.path(), &binding);
    commit_binding(repo.path());
    failure(
        run(repo.path(), "remove"),
        "recorded tasks still require this package for workspace cleanup",
    );
    assert_eq!(versions(repo.path()).len(), 1);
}

#[test]
fn installed_inventory_is_offline_and_does_not_create_or_clean_state() {
    let source = source();
    let binding = binding(source.path());
    let repo = project(&binding);
    let inspect = || {
        common::ahu()
            .current_dir(repo.path())
            .args(["plugins", "installed", "--output", "json"])
            .output()
            .unwrap()
    };
    let empty: serde_json::Value = serde_json::from_str(&success(inspect())).unwrap();
    assert_eq!(empty["installations"], serde_json::json!([]));
    assert!(!repo.path().join(".ahu").exists());
    success(run(repo.path(), "install"));
    write(
        repo.path(),
        ".ahu/state/plugins/fixture/.install-crashed/partial",
        "partial",
    );
    source.close().unwrap(); // Inspection cannot use the declared source.
    let result: serde_json::Value = serde_json::from_str(&success(inspect())).unwrap();
    assert_eq!(result["installations"].as_array().unwrap().len(), 1);
    assert_eq!(result["installations"][0]["source_verified"], true);
    assert!(
        repo.path()
            .join(".ahu/state/plugins/fixture/.install-crashed/partial")
            .exists()
    );
    assert!(common::git(repo.path(), &["status", "--porcelain"]).is_empty());
}

#[test]
fn tracked_state_and_dirty_binding_changes_are_not_overwritten() {
    let source = source();
    let mut binding = binding(source.path());
    let repo = project(&binding);
    write(repo.path(), ".ahu/state/plugins/tracked", "keep");
    common::git(repo.path(), &["add", "-f", ".ahu/state/plugins/tracked"]);
    failure(run(repo.path(), "install"), "must not be tracked");
    common::git(
        repo.path(),
        &["rm", "--cached", ".ahu/state/plugins/tracked"],
    );
    fs::remove_file(repo.path().join(".ahu/state/plugins/tracked")).unwrap();
    success(run(repo.path(), "install"));
    binding
        .grants
        .push(ahu::plugin_binding::Grant::ReadProjectContext);
    save(repo.path(), &binding);
    failure(
        run(repo.path(), "enable"),
        "staged or unstaged binding changes",
    );
    assert_eq!(
        ahu::plugin_binding::load(repo.path(), "fixture")
            .unwrap()
            .unwrap()
            .0,
        binding
    );
}

#[test]
fn deepest_valid_paths_can_be_removed_and_concurrent_install_is_idempotent() {
    let source = source();
    let deep = format!("packages/fixture/{}leaf", "d/".repeat(31));
    write(source.path(), &deep, "nested");
    commit(source.path());
    let binding = binding(source.path());
    let repo = project(&binding);
    let mut first = common::ahu()
        .current_dir(repo.path())
        .args(["plugins", "install", "fixture"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    success(run(repo.path(), "install"));
    assert!(first.wait().unwrap().success());
    assert_eq!(versions(repo.path()).len(), 1);
    success(run(repo.path(), "remove"));
    assert!(versions(repo.path()).is_empty());
}

#[test]
fn orphaned_installations_can_be_explicitly_removed_after_binding_deletion() {
    let source = source();
    let binding = binding(source.path());
    let repo = project(&binding);
    success(run(repo.path(), "install"));
    fs::remove_file(repo.path().join(".agents/ahu/plugins/fixture.md")).unwrap();
    commit_binding(repo.path());
    success(run(repo.path(), "remove"));
    assert!(versions(repo.path()).is_empty());
}

#[test]
fn headless_admission_and_resume_ownership_block_cleanup() {
    use std::os::fd::AsRawFd;
    let source = source();
    let binding = binding(source.path());
    let repo = project(&binding);
    success(run(repo.path(), "install"));
    let discovered = ahu::git::discover(repo.path()).unwrap();
    let store = ahu::headless::store(&discovered).unwrap();
    fs::create_dir_all(&store).unwrap();
    fs::set_permissions(&store, fs::Permissions::from_mode(0o700)).unwrap();
    let lock_path = store.join("launch.lock");
    fs::write(&lock_path, "").unwrap();
    fs::set_permissions(&lock_path, fs::Permissions::from_mode(0o600)).unwrap();
    let lock = fs::File::open(&lock_path).unwrap();
    // SAFETY: this fixture owns the descriptor and never shares the repository.
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    failure(run(repo.path(), "remove"), "admission is in progress");
    drop(lock);
    let dir = record(repo.path(), "fixture-terminal");
    ahu::task::set_state(&dir, ahu::task::TaskState::Exited).unwrap();
    write(&dir, "headless.json", "{}");
    let owner_path = dir.join("owner.lock");
    fs::write(&owner_path, "").unwrap();
    fs::set_permissions(&owner_path, fs::Permissions::from_mode(0o600)).unwrap();
    let owner = fs::File::open(&owner_path).unwrap();
    // SAFETY: this fixture owns the descriptor.
    assert_eq!(
        unsafe { libc::flock(owner.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    failure(run(repo.path(), "disable"), "supervisor is live");
    drop(owner);
    success(run(repo.path(), "remove"));
}

#[test]
fn policy_mutations_refuse_index_changes_even_when_worktree_matches_head() {
    for action in ["enable", "disable", "remove"] {
        let source = source();
        let mut binding = binding(source.path());
        let repo = project(&binding);
        success(run(repo.path(), "install"));
        let relative = ".agents/ahu/plugins/fixture.md";
        let path = repo.path().join(relative);
        let original = fs::read(&path).unwrap();
        let installed = versions(repo.path());
        binding.enabled = true;
        save(repo.path(), &binding);
        common::git(repo.path(), &["add", "--", relative]);
        // This defeats a HEAD-vs-working-copy comparison: the index alone
        // carries a different policy decision that must not be discarded.
        fs::write(&path, &original).unwrap();
        let index = common::git(repo.path(), &["ls-files", "--stage", "--", relative]);
        failure(
            run(repo.path(), action),
            "staged or unstaged binding changes",
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(versions(repo.path()), installed);
        assert_eq!(
            common::git(repo.path(), &["ls-files", "--stage", "--", relative]),
            index
        );
    }
}

#[test]
fn policy_mutations_refuse_unstaged_metadata_and_enablement_edits() {
    for action in ["enable", "disable", "remove"] {
        for edit_enabled in [false, true] {
            let source = source();
            let mut binding = binding(source.path());
            let repo = project(&binding);
            success(run(repo.path(), "install"));
            let path = repo.path().join(".agents/ahu/plugins/fixture.md");
            if edit_enabled {
                binding.enabled = true;
                save(repo.path(), &binding);
            } else {
                let mut contents = fs::read_to_string(&path).unwrap();
                contents.push_str("Uncommitted documentation edit.\n");
                fs::write(&path, contents).unwrap();
            }
            let contents = fs::read(&path).unwrap();
            let installed = versions(repo.path());
            failure(
                run(repo.path(), action),
                "staged or unstaged binding changes",
            );
            assert_eq!(fs::read(&path).unwrap(), contents);
            assert_eq!(versions(repo.path()), installed);
        }
    }
}

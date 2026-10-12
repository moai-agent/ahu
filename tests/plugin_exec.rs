//! Synthetic installed packages and temporary Git repositories only. No worktrees.
mod common;
use ahu::agent_task::{
    Controller,
    v1::{self, TaskService},
};
use ahu::plugin_binding::{Binding, Compatibility, Grant, Source, SourceKind};
use ahu::plugin_exec::task_service::{self, TaskReadGrant};
use ahu::plugin_exec::{self, Code, HostGrant};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    package: PathBuf,
    binding: Binding,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let package = root.join("package ; $literal");
        fs::create_dir(&package).unwrap();
        fs::write(
            package.join("Cargo.toml"),
            "[package]\nname = 'fixture-plugin'\nversion = '1.0.0'\n",
        )
        .unwrap();
        fs::write(package.join("ahu-plugin.md"), "---\nokf_version: '0.2'\ntype: ahu:plugin\nschema_version: 1\nid: fixture\npackage: fixture-plugin\nhost_api: 1\ncargo_manifest: Cargo.toml\nentrypoint:\n  path: 'entry ; $literal'\n  protocol_version: 1\n  timeout_ms: 500\n---\n").unwrap();
        let script = include_str!("fixtures/plugin-entrypoint.py")
            .replace("MODE = \"normal\"", &format!("MODE = {}", json!(mode)))
            .replace(
                "MARKER = \"\"",
                &format!("MARKER = {}", json!(root.join("started"))),
            );
        fs::write(package.join("entry ; $literal"), script).unwrap();
        fs::set_permissions(
            package.join("entry ; $literal"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let digest = ahu::plugin_install::digest(&package).unwrap();
        let binding = Binding {
            schema_version: 1,
            id: "fixture".into(),
            version: "1.0.0".into(),
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
            grants: vec![Grant::Execute],
        };
        common::git(&root, &["init", "-q"]);
        let fixture = Self {
            _temp: temp,
            root,
            package,
            binding,
        };
        fixture.save(true);
        fixture
    }
    fn save(&self, commit: bool) {
        let directory = self.root.join(".agents/ahu/plugins");
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("fixture.md"),
            ahu::okf::Document::new("ahu:plugin-binding", &self.binding, "")
                .unwrap()
                .render()
                .unwrap(),
        )
        .unwrap();
        if commit {
            common::git(&self.root, &["add", ".agents"]);
            common::git(
                &self.root,
                &[
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "--allow-empty",
                    "-qm",
                    "synthetic plugin policy",
                ],
            );
        }
    }
    fn cli(&self, args: &[&str]) -> Value {
        let output = common::ahu()
            .current_dir(&self.root)
            .env_remove("AHU_INTERACTIVE_PLUGIN")
            .env_remove("AHU_PLUGIN_PATH")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    fn install(&self) {
        let output = common::ahu()
            .current_dir(&self.root)
            .args(["plugins", "install", "fixture"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!self.root.join("started").exists());
    }
    fn invoke(&self, input: &[u8], cancelled: &AtomicBool) -> plugin_exec::Result<Value> {
        let repo = ahu::git::discover(&self.root).unwrap();
        let grant =
            HostGrant::new(&repo, "fixture", br#"{"invocation_id":"synthetic-1"}"#).unwrap();
        plugin_exec::invoke(&repo, &grant, input, cancelled)
    }
    fn stopped(&self) {
        let data: Value =
            serde_json::from_slice(&fs::read(self.root.join("started")).unwrap()).unwrap();
        assert_eq!(data["cwd_mode"].as_u64().unwrap() & 0o077, 0);
        let pid = data["pid"].as_i64().unwrap() as i32;
        // SAFETY: signal zero only observes the PID of our synthetic child.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "child was not reaped");
        assert!(!Path::new(data["cwd"].as_str().unwrap()).exists());
    }
}

#[test]
fn installed_entrypoint_is_inert_until_host_authorized_invocation() {
    for mode in ["normal", "stderr", "exact_limit"] {
        let fixture = Fixture::new(mode);
        fixture.install();
        let inventory = fixture.cli(&["plugins", "--output", "json"]);
        let entry = inventory["plugins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == "fixture")
            .unwrap();
        assert!(
            !entry["selected"].as_bool().unwrap(),
            "generic entrypoint became a session host"
        );
        assert_eq!(entry["entrypoint"]["protocol_version"], 1);
        assert!(entry["executable"].is_null());
        assert!(!fixture.root.join("started").exists());
        let input =
            br#"{"literal":"$(touch never-executed); `echo no`","grant":{"operation":"all"}}"#;
        let value = fixture.invoke(input, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            value["input"],
            serde_json::from_slice::<Value>(input).unwrap()
        );
        assert_eq!(value["context"], json!({"invocation_id":"synthetic-1"}));
        fixture.stopped();
        assert!(!fixture.root.join("never-executed").exists());
    }
}

#[test]
fn disabled_missing_grant_uncommitted_policy_and_tampering_never_start() {
    let mut fixture = Fixture::new("normal");
    fixture.install();
    fixture.binding.enabled = false;
    fixture.save(true);
    // Prove disabled admission doesn't inspect a package: even absent data is Denied.
    fs::rename(
        fixture.root.join(".ahu/state/plugins"),
        fixture.root.join("held-installation"),
    )
    .unwrap();
    let err = fixture
        .invoke(b"null", &AtomicBool::new(false))
        .unwrap_err();
    assert_eq!(err.code, Code::Denied);
    assert!(!err.outcome_may_be_unknown);
    fixture.binding.enabled = true;
    fixture.save(false);
    assert_eq!(
        fixture
            .invoke(b"null", &AtomicBool::new(false))
            .unwrap_err()
            .code,
        Code::Denied
    );
    fixture.binding.grants = vec![Grant::InteractiveBackend];
    fixture.save(true);
    assert_eq!(
        fixture
            .invoke(b"null", &AtomicBool::new(false))
            .unwrap_err()
            .code,
        Code::Denied
    );
    fixture.binding.grants = vec![Grant::Execute];
    fixture.save(true);
    assert_eq!(
        fixture
            .invoke(b"null", &AtomicBool::new(false))
            .unwrap_err()
            .code,
        Code::Unavailable
    );
    fs::rename(
        fixture.root.join("held-installation"),
        fixture.root.join(".ahu/state/plugins"),
    )
    .unwrap();
    let versions = fixture.root.join(".ahu/state/plugins/fixture");
    let installed = fs::read_dir(versions)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::write(
        installed.join("package/entry ; $literal"),
        "untrusted replacement",
    )
    .unwrap();
    assert_eq!(
        fixture
            .invoke(b"null", &AtomicBool::new(false))
            .unwrap_err()
            .code,
        Code::Unavailable
    );
    assert!(!fixture.root.join("started").exists());
}

#[test]
fn input_context_and_repository_scope_are_checked_before_spawn() {
    let fixture = Fixture::new("normal");
    fixture.install();
    for input in [
        vec![b'x'; plugin_exec::MAX_BYTES + 1],
        br#"{"key":1,"key":2}"#.to_vec(),
        b"{} {}".to_vec(),
        serde_json::to_vec(&"x".repeat(plugin_exec::MAX_BYTES - 2)).unwrap(),
    ] {
        let failure = fixture.invoke(&input, &AtomicBool::new(false)).unwrap_err();
        assert_eq!(failure.code, Code::InvalidInput);
        assert!(!failure.outcome_may_be_unknown);
    }
    let repo = ahu::git::discover(&fixture.root).unwrap();
    for context in [
        b"null".to_vec(),
        b"{}".to_vec(),
        br#"{"invocation_id":"one","invocation_id":"two"}"#.to_vec(),
        br#"{"invocation_id":"one","credentials":"synthetic-secret"}"#.to_vec(),
        br#"{"invocation_id":"one","path":"/synthetic/private"}"#.to_vec(),
        br#"{"invocation_id":"one","metadata":{"arbitrary":"data"}}"#.to_vec(),
        br#"{"invocation_id":"/synthetic/private"}"#.to_vec(),
        br#"{"invocation_id":""}"#.to_vec(),
        serde_json::to_vec(&json!({"invocation_id":"x".repeat(65)})).unwrap(),
        vec![b' '; plugin_exec::MAX_CONTEXT_BYTES + 1],
    ] {
        assert!(HostGrant::new(&repo, "fixture", &context).is_err());
    }
    let other = Fixture::new("normal");
    let other_repo = ahu::git::discover(&other.root).unwrap();
    let grant = HostGrant::new(&other_repo, "fixture", br#"{"invocation_id":"other-1"}"#).unwrap();
    assert_eq!(
        plugin_exec::invoke(&repo, &grant, b"null", &AtomicBool::new(false))
            .unwrap_err()
            .code,
        Code::Denied
    );
    let failure = fixture.invoke(b"null", &AtomicBool::new(true)).unwrap_err();
    assert_eq!(failure.code, Code::Cancelled);
    assert!(!failure.outcome_may_be_unknown);
    assert!(!fixture.root.join("started").exists());
}

#[test]
fn bounded_response_errors_and_timeout_are_safe_and_reap_children() {
    for (mode, expected) in [
        ("malformed", Code::Protocol),
        ("duplicate", Code::Protocol),
        ("version", Code::Protocol),
        ("extra", Code::Protocol),
        ("trailing", Code::Protocol),
        ("partial", Code::Protocol),
        ("remote_error", Code::Failed),
        ("nonzero", Code::Failed),
        ("overflow", Code::OutputLimit),
        ("early_output", Code::OutputLimit),
        ("hang", Code::TimedOut),
        ("never_read", Code::TimedOut),
        ("hang_after_response", Code::TimedOut),
        ("inherited_pipe", Code::TimedOut),
    ] {
        let fixture = Fixture::new(mode);
        fixture.install();
        let input = serde_json::to_vec(&"x".repeat(60_000)).unwrap();
        let started = Instant::now();
        let failure = fixture.invoke(&input, &AtomicBool::new(false)).unwrap_err();
        assert_eq!(failure.code, expected, "{mode}: {failure}");
        assert!(failure.outcome_may_be_unknown);
        assert!(!failure.to_string().contains("SECRET"));
        assert!(started.elapsed() < Duration::from_secs(5));
        fixture.stopped();
    }
}

#[test]
fn cancellation_interrupts_in_flight_io() {
    let fixture = Fixture::new("hang");
    fixture.install();
    let cancelled = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let join = scope.spawn(|| fixture.invoke(b"null", &cancelled));
        let until = Instant::now() + Duration::from_secs(5);
        while !fixture.root.join("started").exists() {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
        }
        cancelled.store(true, Ordering::Relaxed);
        let failure = join.join().unwrap().unwrap_err();
        assert_eq!(failure.code, Code::Cancelled);
        assert!(failure.outcome_may_be_unknown);
    });
    fixture.stopped();
}

#[test]
fn invalid_entrypoint_declarations_are_inert() {
    for replacement in [
        "protocol_version: 2",
        "timeout_ms: 99",
        "path: '../escape'",
        "path: '/bin/sh'",
    ] {
        let fixture = Fixture::new("normal");
        let manifest = fixture.package.join("ahu-plugin.md");
        let field = replacement.split(':').next().unwrap();
        let original = fs::read_to_string(&manifest).unwrap();
        let changed = original
            .lines()
            .map(|line| {
                if line.trim_start().starts_with(&format!("{field}:")) {
                    format!("  {replacement}")
                } else {
                    line.into()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(manifest, changed).unwrap();
        let result = common::ahu()
            .current_dir(&fixture.root)
            .env("AHU_PLUGIN_PATH", &fixture.package)
            .env("AHU_INTERACTIVE_PLUGIN", "none")
            .args(["plugins", "--output", "json"])
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(!fixture.root.join("started").exists());
    }
}

fn task_record(repo: &ahu::git::Repo, task_id: &str) -> ahu::task::TaskRecord {
    let discovered = repo;
    let worktree = Path::new("/synthetic/private/worktree");
    let adapter = ahu::harness::adapter_for("claude-code").unwrap();
    let (delivered, delivery) = ahu::orchestration::deliver(
        Some("synthetic private instructions"),
        "synthetic private task prompt",
    )
    .unwrap();
    let command = adapter
        .launch_command(&ahu::harness::LaunchRequest {
            model: "claude-opus-5",
            prompt: &delivered,
            cwd: worktree,
            permissions: Default::default(),
        })
        .unwrap();
    ahu::task::TaskRecord {
        schema_version: ahu::task::TASK_SCHEMA_VERSION,
        task_id: task_id.to_string(),
        title: format!("task {task_id}"),
        summary: String::new(),
        created_at: format!("2026-09-12T00:00:{:02}Z", task_id.len()),
        repo_identity: discovered.identity(),
        repo_root: discovered.root.clone(),
        branch: format!("ahu/chris/{task_id}"),
        worktree: worktree.to_path_buf(),
        base_commit: discovered.head.clone(),
        identity: ahu::task::LaunchIdentity {
            release_id: None,
            mode: ahu::task::LaunchMode::Named,
            agent: "chris".to_string(),
            agent_version: Some("1.0.0".to_string()),
            permissions: Default::default(),
            harness: "claude-code".to_string(),
            model: "claude-opus-5".to_string(),
            instructions_source: Some("/synthetic/private/instructions.md".to_string()),
            source_digest: Some("2".repeat(64)),
            instructions_digest: Some("3".repeat(64)),
            identity_digest: Some("4".repeat(64)),
            selection_basis: None,
        },
        policy_digest: "0".repeat(64),
        catalog_version: ahu::catalog::CATALOG_VERSION.to_string(),
        config_snapshot: Default::default(),
        config_snapshot_digest: "0".repeat(64),
        hooks: Default::default(),
        hooks_digest: String::new(),
        materialize: Default::default(),
        launch_command: command.redacted(),
        delivery,
        prompt_digest: ahu::util::digest_bytes(b"synthetic private task prompt"),
        harness_executable: PathBuf::from("/usr/local/bin/claude"),
        reliability_warning: None,
        enforcement: adapter
            .enforcement("claude-opus-5", Default::default())
            .unwrap(),
        session_host: None,
        cmux_group_id: None,
        cmux_workspace_id: None,
        cmux_window_id: None,
        state: ahu::task::TaskState::Starting,
    }
}

const TASK_A: &str = "00000000-0000-7000-8000-000000000001";
const TASK_B: &str = "00000000-0000-7000-8000-000000000002";
const TASK_HIDDEN: &str = "00000000-0000-7000-8000-000000000003";
const TASK_MISSING: &str = "00000000-0000-7000-8000-000000000004";

fn task_grant(repo: &ahu::git::Repo, core: &Controller, tasks: &[&str]) -> TaskReadGrant {
    let host = HostGrant::new(
        repo,
        "fixture",
        br#"{"invocation_id":"task-observations-1"}"#,
    )
    .unwrap();
    TaskReadGrant::new(host, core, tasks.iter().map(|id| (*id).into()).collect()).unwrap()
}
fn observe(
    repo: &ahu::git::Repo,
    grant: &TaskReadGrant,
    observations: Value,
) -> plugin_exec::Result<Value> {
    task_service::invoke(
        repo,
        grant,
        &serde_json::to_vec(&json!({
            "version": 1, "observations": observations
        }))
        .unwrap(),
        &AtomicBool::new(false),
    )
}
fn task_core(repo: &ahu::git::Repo, local: bool) -> Controller {
    let core = if local {
        Controller::local(repo).unwrap()
    } else {
        Controller::memory(&repo.identity())
    };
    for id in [TASK_A, TASK_B, TASK_HIDDEN] {
        core.register_admitted(
            &task_record(repo, id),
            "synthetic-provider",
            v1::Principal::Operator("synthetic-owner".into()),
            true,
        )
        .unwrap();
    }
    core
}

#[test]
fn task_observations_are_exact_scoped_projected_and_read_only() {
    for local in [false, true] {
        let fixture = Fixture::new("normal");
        fixture.install();
        let repo = ahu::git::discover(&fixture.root).unwrap();
        let core = task_core(&repo, local);
        if local {
            // Ungranted state is never read, even when corrupt.
            let path = ahu::state::coordination_dir(&repo)
                .unwrap()
                .join("agent-tasks-v1")
                .join(format!("{TASK_HIDDEN}.json"));
            fs::write(path, b"corrupt ungranted state").unwrap();
        }
        let grant = task_grant(&repo, &core, &[TASK_B, TASK_A]);
        let result = observe(
            &repo,
            &grant,
            json!([
                {"operation":"list", "limit":1},
                {"operation":"list", "after":TASK_A, "limit":128},
                {"operation":"read", "task":TASK_B},
                {"operation":"replay", "task":TASK_A, "after":0}
            ]),
        )
        .unwrap();
        let input = &result["input"];
        assert_eq!(input["kind"], "ahu.task-observations");
        assert_eq!(input["version"], 1);
        assert_eq!(
            input["capabilities"],
            json!({
                "version":1, "operations":["list","read","replay"],
                "max_input_bytes":65536, "replay_window":128
            })
        );
        let snapshot = |id: &str| {
            json!({"version":1,"task":id,"revision":1,
            "lifecycle":"prepared","cancellation_requested":false})
        };
        assert_eq!(
            input["observations"],
            json!([
                {"operation":"list", "tasks":[snapshot(TASK_A)], "next_after":TASK_A},
                {"operation":"list", "tasks":[snapshot(TASK_B)], "next_after":null},
                {"operation":"read", "snapshot":snapshot(TASK_B)},
                {"operation":"replay", "snapshot":snapshot(TASK_A), "changes":[{
                    "revision":1,"kind":"admitted","lifecycle":"prepared","cancellation_requested":false
                }]}
            ])
        );
        assert!(!input.to_string().contains(TASK_HIDDEN));
        for excluded in [
            "synthetic private",
            "/synthetic",
            "instructions_source",
            "identity",
            "owner",
            "prompt_digest",
            "launch_command",
            "supports_input",
            "controller",
            "credentials",
        ] {
            assert!(!input.to_string().contains(excluded), "leaked {excluded}");
        }
        let reader = core
            .plugin_read_session("fixture", [TASK_A.into()].into())
            .unwrap();
        assert_eq!(reader.get(1, TASK_A).unwrap().revision, 1);
        assert_eq!(
            reader.effects(1, TASK_A).unwrap().start,
            v1::DeliveryState::Pending
        );
        assert!(!fixture.root.join(".worktrees").exists());
        fixture.stopped();
    }
}

#[test]
fn task_observations_deny_ungranted_ids_cursors_and_authority_injection() {
    let fixture = Fixture::new("normal");
    fixture.install();
    let repo = ahu::git::discover(&fixture.root).unwrap();
    let core = task_core(&repo, false);
    let grant = task_grant(&repo, &core, &[TASK_A]);
    for task in [
        TASK_HIDDEN,
        TASK_MISSING,
        "00000000",
        "ahu:task:00000000-0000-7000-8000-000000000001",
        "../task",
    ] {
        for request in [
            json!({"operation":"read","task":task}),
            json!({"operation":"replay","task":task,"after":0}),
            json!({"operation":"list","after":task,"limit":1}),
        ] {
            let error = observe(&repo, &grant, json!([request])).unwrap_err();
            assert_eq!(error.code, Code::Denied);
            assert!(!error.outcome_may_be_unknown);
        }
    }
    for invalid in [
        json!({"operation":"submit","task":TASK_A}),
        json!({"operation":"cancel","task":TASK_A}),
        json!({"operation":"input","task":TASK_A,"text":"data"}),
        json!({"operation":"effects","task":TASK_A}),
        json!({"operation":"read","task":TASK_A,"principal":{"kind":"operator","id":"owner"}}),
        json!({"operation":"read","task":TASK_A,"grants":[TASK_HIDDEN]}),
        json!({"operation":"list","limit":0}),
        json!({"operation":"list","limit":129}),
        json!({"operation":"replay","task":TASK_A,"after":2}),
    ] {
        assert_eq!(
            observe(&repo, &grant, json!([invalid])).unwrap_err().code,
            Code::InvalidInput
        );
    }
    for request in [
        br#"{"version":2,"observations":[]}"#.as_slice(),
        br#"{"version":1,"observations":[],"plugin_id":"another"}"#,
        br#"{"version":1,"observations":[],"tasks":[]}"#,
        br#"{"version":1,"observations":[],"version":1}"#,
        br#"{"version":1,"observations":[{"operation":"list","limit":1,"limit":2}]}"#,
        br#"{"version":1,"observations":[]} {}"#,
    ] {
        assert_eq!(
            task_service::invoke(&repo, &grant, request, &AtomicBool::new(false))
                .unwrap_err()
                .code,
            Code::InvalidInput
        );
    }
    let missing = task_grant(&repo, &core, &[TASK_MISSING]);
    assert_eq!(
        observe(
            &repo,
            &missing,
            json!([{"operation":"read","task":TASK_MISSING}])
        )
        .unwrap_err()
        .code,
        Code::Unavailable
    );
    assert!(!fixture.root.join("started").exists());
}

#[test]
fn task_observations_bound_scope_requests_and_serialized_payloads() {
    let fixture = Fixture::new("normal");
    fixture.install();
    let repo = ahu::git::discover(&fixture.root).unwrap();
    let core = task_core(&repo, false);
    let host = || HostGrant::new(&repo, "fixture", br#"{"invocation_id":"bounded"}"#).unwrap();
    assert!(
        TaskReadGrant::new(
            host(),
            &core,
            (0..129)
                .map(|n| format!("00000000-0000-7000-8000-{n:012}"))
                .collect()
        )
        .is_err()
    );
    assert!(TaskReadGrant::new(host(), &core, ["prefix".into()].into()).is_err());
    core.register_admitted(
        &task_record(&repo, TASK_MISSING),
        "synthetic-provider",
        v1::Principal::Operator("synthetic-owner".into()),
        true,
    )
    .unwrap();
    let grant = task_grant(&repo, &core, &[TASK_A, TASK_B, TASK_HIDDEN, TASK_MISSING]);
    let requests = vec![json!({"operation":"list","limit":128}); 128];
    assert_eq!(
        observe(&repo, &grant, json!(requests))
            .expect_err("oversized projected input must fail")
            .code,
        Code::InvalidInput
    );
    assert_eq!(
        observe(
            &repo,
            &grant,
            json!(vec![json!({"operation":"list","limit":1}); 129])
        )
        .unwrap_err()
        .code,
        Code::InvalidInput
    );
    assert_eq!(
        task_service::invoke(
            &repo,
            &grant,
            &vec![b' '; plugin_exec::MAX_BYTES + 1],
            &AtomicBool::new(false)
        )
        .unwrap_err()
        .code,
        Code::InvalidInput
    );
    assert!(!fixture.root.join("started").exists());
    let empty = task_grant(&repo, &core, &[]);
    let result = observe(&repo, &empty, json!([{"operation":"list","limit":128}])).unwrap();
    assert_eq!(result["input"]["observations"][0]["tasks"], json!([]));
    assert_eq!(
        result["input"]["observations"][0]["next_after"],
        Value::Null
    );
    fixture.stopped();
}

#[test]
fn task_observations_reauthorize_binding_repository_and_scope() {
    let mut fixture = Fixture::new("normal");
    fixture.install();
    let repo = ahu::git::discover(&fixture.root).unwrap();
    let core = task_core(&repo, false);
    let grant = task_grant(&repo, &core, &[TASK_A]);
    let other = Controller::memory("different-repository");
    let host = HostGrant::new(&repo, "fixture", br#"{"invocation_id":"repository"}"#).unwrap();
    assert!(matches!(
        TaskReadGrant::new(host, &other, [TASK_A.into()].into()),
        Err(plugin_exec::Failure {
            code: Code::Denied,
            ..
        })
    ));
    let other_fixture = Fixture::new("normal");
    let other_repo = ahu::git::discover(&other_fixture.root).unwrap();
    assert_eq!(
        observe(&other_repo, &grant, json!([])).unwrap_err().code,
        Code::Denied
    );
    assert_eq!(
        task_service::invoke(&repo, &grant, b"{}", &AtomicBool::new(true))
            .unwrap_err()
            .code,
        Code::Cancelled
    );
    for (enabled, grants) in [
        (false, vec![Grant::Execute]),
        (true, vec![Grant::InteractiveBackend]),
    ] {
        fixture.binding.enabled = enabled;
        fixture.binding.grants = grants;
        fixture.save(true);
        assert_eq!(
            observe(&repo, &grant, json!([])).unwrap_err().code,
            Code::Denied
        );
    }
    fixture.binding.enabled = true;
    fixture.binding.grants = vec![Grant::Execute];
    fixture.save(false);
    assert_eq!(
        observe(&repo, &grant, json!([])).unwrap_err().code,
        Code::Denied
    );
    fixture.save(true);
    let revoked = task_grant(&repo, &core, &[]);
    assert_eq!(
        observe(
            &repo,
            &revoked,
            json!([{"operation":"list","after":TASK_A,"limit":1}])
        )
        .unwrap_err()
        .code,
        Code::Denied
    );
    fixture.binding.compatibility.ahu = "0.0.0".into();
    fixture.save(true);
    assert_eq!(
        observe(&repo, &grant, json!([])).unwrap_err().code,
        Code::Denied
    );
    assert!(!fixture.root.join("started").exists());
}

#[test]
fn task_replay_expiry_and_plugin_results_never_dispatch_effects() {
    let fixture = Fixture::new("mutation_result");
    fixture.install();
    let repo = ahu::git::discover(&fixture.root).unwrap();
    let core = task_core(&repo, false);
    let grant = task_grant(&repo, &core, &[TASK_A]);
    // Seed bounded coordination history through the trusted owner only. No
    // execution adapter is created or dispatched by this fixture.
    let owner = core.session(
        v1::Principal::Operator("synthetic-owner".into()),
        [TASK_A.into()].into(),
        [v1::Operation::Cancel].into(),
    );
    for revision in 1..=128 {
        owner
            .mutate(v1::Mutation {
                version: 1,
                task: TASK_A.into(),
                operation_id: format!("seed-{revision}"),
                expected_revision: revision,
                action: v1::Action::Cancel,
            })
            .unwrap();
    }
    assert_eq!(
        observe(
            &repo,
            &grant,
            json!([{"operation":"replay","task":TASK_A,"after":0}])
        )
        .unwrap_err()
        .code,
        Code::InvalidInput
    );
    assert!(!fixture.root.join("started").exists());
    let result = observe(
        &repo,
        &grant,
        json!([{"operation":"replay","task":TASK_A,"after":128}]),
    )
    .unwrap();
    assert_eq!(result, json!({"operation":"submit","task":TASK_A}));
    let reader = core
        .plugin_read_session("fixture", [TASK_A.into()].into())
        .unwrap();
    assert_eq!(reader.get(1, TASK_A).unwrap().revision, 129);
    assert_eq!(
        reader.effects(1, TASK_A).unwrap().start,
        v1::DeliveryState::Pending
    );
    fixture.stopped();
}

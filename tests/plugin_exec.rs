//! Synthetic installed packages and temporary Git repositories only. No worktrees.
mod common;
use ahu::plugin_binding::{Binding, Compatibility, Grant, Source, SourceKind};
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

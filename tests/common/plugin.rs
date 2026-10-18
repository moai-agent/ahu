//! Reusable synthetic executable package; no provider calls or worktrees.
use ahu::plugin_binding::{Binding, Compatibility, Grant, Source, SourceKind};
use ahu::plugin_exec::{self, HostGrant};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
pub struct Fixture {
    _temp: tempfile::TempDir,
    pub root: PathBuf,
    pub package: PathBuf,
    pub binding: Binding,
}
impl Fixture {
    pub fn new(mode: &str) -> Self {
        // Python startup can exceed 500 ms when the full integration suite is
        // running in parallel. Keep the production timeout behavior covered by
        // the explicit timeout tests below, while giving normal fixtures enough
        // room to start reliably on loaded CI workers.
        Self::with_timeout(mode, 2_000)
    }
    pub fn with_timeout(mode: &str, timeout_ms: u64) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let package = root.join("package ; $literal");
        fs::create_dir(&package).unwrap();
        fs::write(
            package.join("Cargo.toml"),
            "[package]\nname = 'fixture-plugin'\nversion = '1.0.0'\n",
        )
        .unwrap();
        fs::write(package.join("ahu-plugin.md"), "---\nokf_version: '0.2'\ntype: ahu:plugin\nschema_version: 1\nid: fixture\npackage: fixture-plugin\nhost_api: 1\ncargo_manifest: Cargo.toml\nentrypoint:\n  path: 'entry ; $literal'\n  protocol_version: 1\n  timeout_ms: 500\n---\n".replace("timeout_ms: 500", &format!("timeout_ms: {timeout_ms}"))).unwrap();
        let script = include_str!("../fixtures/plugin-entrypoint.py")
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
        super::git(&root, &["init", "-q"]);
        let fixture = Self {
            _temp: temp,
            root,
            package,
            binding,
        };
        fixture.save(true);
        fixture
    }
    pub fn save(&self, commit: bool) {
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
            super::git(&self.root, &["add", ".agents"]);
            super::git(
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
    pub fn cli(&self, args: &[&str]) -> Value {
        let output = super::ahu()
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
    pub fn install(&self) {
        let output = super::ahu()
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
    pub fn invoke(&self, input: &[u8], cancelled: &AtomicBool) -> plugin_exec::Result<Value> {
        let repo = ahu::git::discover(&self.root).unwrap();
        let grant =
            HostGrant::new(&repo, "fixture", br#"{"invocation_id":"synthetic-1"}"#).unwrap();
        plugin_exec::invoke(&repo, &grant, input, cancelled)
    }
    pub fn stopped(&self) {
        let data: Value =
            serde_json::from_slice(&fs::read(self.root.join("started")).unwrap()).unwrap();
        assert_eq!(data["cwd_mode"].as_u64().unwrap() & 0o077, 0);
        let pid = data["pid"].as_i64().unwrap() as i32;
        // SAFETY: signal zero only observes the PID of our synthetic child.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "child was not reaped");
        assert!(!Path::new(data["cwd"].as_str().unwrap()).exists());
    }
}

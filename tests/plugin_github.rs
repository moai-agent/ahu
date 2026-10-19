//! Offline synthetic GitHub provider package. No network, credentials, or GitHub access.

mod common;

use ahu::plugin_binding::{Binding, Compatibility, Grant, Source, SourceKind};
use ahu::plugin_exec::{self, Code, HostGrant};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::AtomicBool,
};

struct Github {
    _temp: tempfile::TempDir,
    root: PathBuf,
    binding: Binding,
}

impl Github {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let package = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins")
            .join("github");
        let digest = ahu::plugin_install::digest(&package).unwrap();
        let binding = Binding {
            schema_version: 1,
            id: "github".into(),
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
        let github = Self {
            _temp: temp,
            root,
            binding,
        };
        github.save(true);
        github
    }

    fn save(&self, commit: bool) {
        let directory = self.root.join(".agents/ahu/plugins");
        fs::create_dir_all(&directory).unwrap();
        let document = ahu::okf::Document::new("ahu:plugin-binding", &self.binding, "")
            .unwrap()
            .render()
            .unwrap();
        fs::write(directory.join("github.md"), document).unwrap();
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
            .args(["plugins", "install", "github"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn invoke(&self, input: &[u8]) -> plugin_exec::Result<Value> {
        let repo = ahu::git::discover(&self.root).unwrap();
        let grant = HostGrant::new(&repo, "github", br#"{"invocation_id":"github-1"}"#).unwrap();
        plugin_exec::invoke(&repo, &grant, input, &AtomicBool::new(false))
    }
}

fn base_input(action: &str, operation_id: &str) -> Value {
    let body = if action == "close" {
        Value::Null
    } else {
        json!("first comment")
    };
    json!({
        "version": 1,
        "action": action,
        "operation_id": operation_id,
        "issue": {"project": "example/project", "number": 7, "visibility": "private"},
        "tracker_visibility": "private",
        "scopes": ["issues:write"],
        "expected_revision": 0,
        "state": {"revision": 0, "applied": {}},
        "body": body,
    })
}

fn run_entrypoint(request: &Value) -> Value {
    let entrypoint = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("plugins")
        .join("github")
        .join("entrypoint");
    let mut child = Command::new(entrypoint)
        .arg("--ahu-plugin-protocol=1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    serde_json::to_writer(child.stdin.take().unwrap(), request).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "entrypoint failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn protocol_versions_must_be_json_integers() {
    let mut request = json!({
        "protocol_version": 1.0,
        "plugin_id": "github",
        "grant": {"operation": "invoke"},
        "context": {"invocation_id": "github-version-test"},
        "input": base_input("comment", "op-1"),
    });
    let response = run_entrypoint(&request);
    assert_eq!(response["status"], "error");
    assert_eq!(response["code"], "invalid_input");

    request["protocol_version"] = json!(1);
    request["input"]["version"] = json!(1.0);
    let response = run_entrypoint(&request);
    assert_eq!(response["status"], "error");
    assert_eq!(response["code"], "invalid_input");
}

#[test]
fn comment_applies_and_advances_revision() {
    let github = Github::new();
    github.install();
    let inventory = github.cli(&["plugins", "--output", "json"]);
    let entry = inventory["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|plugin| plugin["id"] == "github")
        .unwrap();
    assert!(
        !entry["selected"].as_bool().unwrap(),
        "provider entrypoint became a session host"
    );
    assert_eq!(entry["entrypoint"]["protocol_version"], 1);
    assert!(entry["executable"].is_null());
    let data = base_input("comment", "op-1");
    let value = github.invoke(&serde_json::to_vec(&data).unwrap()).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["provider"], "github");
    assert_eq!(value["operation_id"], "op-1");
    assert_eq!(value["outcome"], "applied");
    assert!(value["reason"].is_null());
    assert_eq!(value["reference"]["project"], "example/project");
    assert_eq!(value["reference"]["issue"], 7);
    assert_eq!(value["state"]["revision"], 1);
    let digest = value["state"]["applied"]["op-1"].as_str().unwrap();
    assert_eq!(digest.len(), 64);
    assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

#[test]
fn repeat_operation_is_idempotent_even_with_stale_expected_revision() {
    let github = Github::new();
    github.install();
    let mut data = base_input("comment", "op-1");
    let first = github.invoke(&serde_json::to_vec(&data).unwrap()).unwrap();
    data["state"] = first["state"].clone();
    let value = github.invoke(&serde_json::to_vec(&data).unwrap()).unwrap();
    assert_eq!(value["outcome"], "idempotent");
    assert!(value["reason"].is_null());
    assert_eq!(value["state"]["revision"], 1);
    assert_eq!(value["reference"]["issue"], 7);
}

#[test]
fn replay_revalidates_write_scope_before_returning_a_receipt() {
    let github = Github::new();
    github.install();
    for action in ["comment", "close"] {
        let mut data = base_input(action, "op-1");
        let first = github.invoke(&serde_json::to_vec(&data).unwrap()).unwrap();
        assert_eq!(first["outcome"], "applied");
        data["state"] = first["state"].clone();
        data["scopes"] = json!([]);
        let value = github.invoke(&serde_json::to_vec(&data).unwrap()).unwrap();
        assert_eq!(value["outcome"], "refused", "{action}");
        assert_eq!(value["reason"], "missing-scope");
        assert!(value["reference"].is_null());
        assert_eq!(value["state"], first["state"]);
    }
}

#[test]
fn replay_revalidates_visibility_before_returning_a_receipt() {
    let github = Github::new();
    github.install();
    for action in ["comment", "close"] {
        let mut data = base_input(action, "op-1");
        data["tracker_visibility"] = json!("public");
        data["issue"]["visibility"] = json!("public");
        let first = github.invoke(&serde_json::to_vec(&data).unwrap()).unwrap();
        assert_eq!(first["outcome"], "applied");
        data["state"] = first["state"].clone();
        data["tracker_visibility"] = json!("private");
        let value = github.invoke(&serde_json::to_vec(&data).unwrap()).unwrap();
        assert_eq!(value["outcome"], "refused", "{action}");
        assert_eq!(value["reason"], "visibility-mismatch");
        assert!(value["reference"].is_null());
        assert_eq!(value["state"], first["state"]);
    }
}

#[test]
fn duplicate_operation_with_different_body_is_refused() {
    let github = Github::new();
    github.install();
    let mut data = base_input("comment", "op-1");
    let first = github.invoke(&serde_json::to_vec(&data).unwrap()).unwrap();
    data["state"] = first["state"].clone();
    data["expected_revision"] = json!(1);
    data["body"] = json!("different body");
    let value = github.invoke(&serde_json::to_vec(&data).unwrap()).unwrap();
    assert_eq!(value["outcome"], "refused");
    assert_eq!(value["reason"], "duplicate-ambiguous");
    assert!(value["reference"].is_null());
    assert_eq!(value["state"]["revision"], 1);
}

#[test]
fn missing_write_scope_is_refused() {
    let github = Github::new();
    github.install();
    let mut data = base_input("comment", "op-1");
    data["scopes"] = json!([]);
    let value = github.invoke(&serde_json::to_vec(&data).unwrap()).unwrap();
    assert_eq!(value["outcome"], "refused");
    assert_eq!(value["reason"], "missing-scope");
    assert!(value["reference"].is_null());
    assert_eq!(value["state"]["revision"], 0);
}

#[test]
fn public_issue_in_private_tracker_is_refused() {
    let github = Github::new();
    github.install();
    let mut data = base_input("comment", "op-1");
    data["issue"]["visibility"] = json!("public");
    let value = github.invoke(&serde_json::to_vec(&data).unwrap()).unwrap();
    assert_eq!(value["outcome"], "refused");
    assert_eq!(value["reason"], "visibility-mismatch");
    assert!(value["reference"].is_null());
    assert_eq!(value["state"]["revision"], 0);
}

#[test]
fn stale_revision_is_refused() {
    let github = Github::new();
    github.install();
    let data = base_input("comment", "op-1");
    let first = github.invoke(&serde_json::to_vec(&data).unwrap()).unwrap();
    let mut data = base_input("close", "op-2");
    data["state"] = first["state"].clone();
    let value = github.invoke(&serde_json::to_vec(&data).unwrap()).unwrap();
    assert_eq!(value["outcome"], "refused");
    assert_eq!(value["reason"], "stale-revision");
    assert_eq!(value["state"]["revision"], 1);
}

#[test]
fn close_applies_without_body() {
    let github = Github::new();
    github.install();
    let data = base_input("close", "op-1");
    let value = github.invoke(&serde_json::to_vec(&data).unwrap()).unwrap();
    assert_eq!(value["outcome"], "applied");
    assert!(value["reason"].is_null());
    assert!(value["reference"].is_object());
    assert_eq!(value["state"]["revision"], 1);
}

#[test]
fn close_with_body_fails_the_invocation() {
    let github = Github::new();
    github.install();
    let mut data = base_input("close", "op-1");
    data["body"] = json!("close comment");
    let failure = github
        .invoke(&serde_json::to_vec(&data).unwrap())
        .unwrap_err();
    assert_eq!(failure.code, Code::Failed);
    assert!(failure.outcome_may_be_unknown);
}

#[test]
fn invalid_json_input_is_rejected_before_execution() {
    let github = Github::new();
    github.install();
    let failure = github.invoke(b"{").unwrap_err();
    assert_eq!(failure.code, Code::InvalidInput);
    assert!(!failure.outcome_may_be_unknown);
}

#[test]
fn structurally_invalid_input_fails_the_entrypoint() {
    let github = Github::new();
    github.install();
    let failure = github.invoke(b"{}").unwrap_err();
    assert_eq!(failure.code, Code::Failed);
    assert!(failure.outcome_may_be_unknown);
}

#[test]
fn disabled_binding_denies_without_unknown_outcome() {
    let mut github = Github::new();
    github.binding.enabled = false;
    github.save(true);
    let data = base_input("comment", "op-1");
    let failure = github
        .invoke(&serde_json::to_vec(&data).unwrap())
        .unwrap_err();
    assert_eq!(failure.code, Code::Denied);
    assert!(!failure.outcome_may_be_unknown);
}

#[test]
fn uninstalled_package_is_unavailable() {
    let github = Github::new();
    github.install();
    let held = github.root.join("held-installation");
    fs::rename(github.root.join(".ahu/state/plugins"), &held).unwrap();
    let data = base_input("comment", "op-1");
    let failure = github
        .invoke(&serde_json::to_vec(&data).unwrap())
        .unwrap_err();
    fs::rename(&held, github.root.join(".ahu/state/plugins")).unwrap();
    assert_eq!(failure.code, Code::Unavailable);
    assert!(!failure.outcome_may_be_unknown);
}

#[test]
fn repo_without_binding_has_no_github_dependency() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    common::git(&root, &["init", "-q"]);
    fs::write(root.join("README.md"), "# fixture\n").unwrap();
    common::git(&root, &["add", "."]);
    common::git(
        &root,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "initial",
        ],
    );
    let output = common::ahu()
        .current_dir(&root)
        .env_remove("AHU_INTERACTIVE_PLUGIN")
        .env_remove("AHU_PLUGIN_PATH")
        .args(["plugins", "--output", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let inventory: Value = serde_json::from_slice(&output.stdout).unwrap();
    for plugin in inventory["plugins"].as_array().unwrap() {
        assert_ne!(plugin["id"], "github");
    }
    let repo = ahu::git::discover(&root).unwrap();
    let grant = HostGrant::new(&repo, "github", br#"{"invocation_id":"github-1"}"#).unwrap();
    let data = base_input("comment", "op-13");
    let failure = plugin_exec::invoke(
        &repo,
        &grant,
        &serde_json::to_vec(&data).unwrap(),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert_eq!(failure.code, Code::Denied);
    assert!(!failure.outcome_may_be_unknown);
}

#[test]
fn cli_invokes_the_installed_provider() {
    let github = Github::new();
    github.install();
    fs::write(
        github.root.join("input.json"),
        serde_json::to_vec(&base_input("comment", "op-14")).unwrap(),
    )
    .unwrap();
    let value = github.cli(&["plugins", "invoke", "github", "--input-file", "input.json"]);
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["status"], "ok");
    assert_eq!(value["result"]["outcome"], "applied");
    assert_eq!(value["result"]["state"]["revision"], 1);
}

#[test]
fn cli_denied_reports_exit_code_four() {
    let mut github = Github::new();
    github.binding.enabled = false;
    github.save(true);
    fs::write(
        github.root.join("input.json"),
        serde_json::to_vec(&base_input("comment", "op-15")).unwrap(),
    )
    .unwrap();
    let output = common::ahu()
        .current_dir(&github.root)
        .env_remove("AHU_INTERACTIVE_PLUGIN")
        .env_remove("AHU_PLUGIN_PATH")
        .args(["plugins", "invoke", "github", "--input-file", "input.json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).unwrap(),
        json!({"schema_version": 1, "status": "error", "code": "denied", "outcome_may_be_unknown": false})
    );
    assert_eq!(
        output.stderr.iter().filter(|byte| **byte == b'\n').count(),
        1
    );
}

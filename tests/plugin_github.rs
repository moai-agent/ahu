//! Synthetic V1 and fake-gh V2 coverage. No network, credentials, or GitHub access.

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
            version: "1.1.0".into(),
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

fn inspect_input() -> Value {
    json!({"version": 2, "action": "inspect", "repository": "example/project",
           "issue_number": 7, "expected_visibility": "private"})
}

struct FakeGh {
    temp: tempfile::TempDir,
}

impl FakeGh {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("gh");
        fs::write(
            &executable,
            r##"#!/usr/bin/python3
import json, os, sys, time
from pathlib import Path
root = Path(__file__).parent
with (root / 'calls').open('a') as log:
    log.write(json.dumps(sys.argv[1:]) + '\n')
# The fixture is self-contained and never reads credentials or uses a network.
assert os.environ['GH_TOKEN'] == 'synthetic-token'
assert os.environ['GITHUB_TOKEN'] == 'synthetic-fallback'
assert os.environ['GH_PROMPT_DISABLED'] == '1'
assert os.environ['GH_PAGER'] == 'cat'
assert 'GH_HOST' not in os.environ
assert 'UNRELATED_SECRET' not in os.environ
assert 'AHU_GITHUB_PATH' not in os.environ
assert sys.stdin.read() == ''
mode = (root / 'mode').read_text()
if mode == 'failure' or (mode == 'issue_failure' and '/issues/' in sys.argv[6]):
    sys.stderr.write('synthetic-stderr-secret')
    sys.stdout.write('synthetic-stdout-secret')
    sys.exit(4)
if mode == 'large' or (mode == 'issue_large' and '/issues/' in sys.argv[6]):
    sys.stdout.write(' ' * 65537)
    sys.exit(0)
if mode == 'timeout':
    time.sleep(4)
    sys.exit(0)
if mode == 'malformed':
    sys.stdout.write('{not JSON: synthetic-secret}')
    sys.exit(0)
if mode == 'duplicate':
    sys.stdout.write('{"full_name":"example/project","full_name":"other/project"}')
    sys.exit(0)
if mode == 'deep':
    sys.stdout.write('[' * 2000 + '0' + ']' * 2000)
    sys.exit(0)
if mode == 'trailing':
    sys.stdout.write('{} {}')
    sys.exit(0)
if mode == 'invalid_utf8':
    sys.stdout.buffer.write(b'\xff')
    sys.exit(0)
name = 'example/project'
if mode == 'case':
    name = 'Example/Project'
repo = {'full_name': name, 'html_url': 'https://github.com/' + name,
        'visibility': 'private', 'private': True}
issue = {'number': 7, 'html_url': 'https://github.com/' + name + '/issues/7',
         'repository_url': 'https://api.github.com/repos/' + name,
         'state': 'open', 'is_pull_request': False}
if mode == 'public':
    repo.update(visibility='public', private=False)
if mode == 'repo_mismatch':
    repo['full_name'] = 'other/project'
if mode == 'repo_url':
    repo['html_url'] = 'https://example.invalid/example/project'
if mode == 'inconsistent_visibility':
    repo['private'] = False
if mode == 'issue_number':
    issue['number'] = 8
if mode == 'issue_repo':
    issue['repository_url'] = 'https://api.github.com/repos/other/project'
if mode == 'issue_url':
    issue['html_url'] += '?secret'
if mode == 'pull_request':
    issue['is_pull_request'] = True
if mode == 'bad_state':
    issue['state'] = 'synthetic-secret'
if mode == 'closed':
    issue['state'] = 'closed'
if mode == 'float_number':
    issue['number'] = 7.0
if mode == 'extra':
    issue['body'] = 'synthetic-secret'
if mode == 'missing':
    del issue['number']
if mode == 'max_output':
    value = json.dumps(issue if '/issues/' in sys.argv[6] else repo)
    sys.stdout.write(value + ' ' * (65536 - len(value)))
else:
    print(json.dumps(issue if '/issues/' in sys.argv[6] else repo))
if mode == 'nonzero_valid':
    sys.exit(4)
"##,
        )
        .unwrap();
        fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
        let fake = Self { temp };
        fake.mode("valid");
        fake
    }

    fn mode(&self, mode: &str) {
        fs::write(self.temp.path().join("mode"), mode).unwrap();
        let _ = fs::remove_file(self.temp.path().join("calls"));
    }

    fn calls(&self) -> Vec<Value> {
        fs::read_to_string(self.temp.path().join("calls"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn invoke(&self, github: &Github, input: &Value) -> std::process::Output {
        fs::write(
            github.root.join("input.json"),
            serde_json::to_vec(input).unwrap(),
        )
        .unwrap();
        common::ahu()
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.temp.path().display()),
            )
            .env("HOME", self.temp.path())
            .env("GH_CONFIG_DIR", self.temp.path())
            .env("XDG_CONFIG_HOME", self.temp.path())
            .env("GH_TOKEN", "synthetic-token")
            .env("GITHUB_TOKEN", "synthetic-fallback")
            .env("GH_HOST", "example.invalid")
            .env("UNRELATED_SECRET", "synthetic-secret")
            .env("AHU_GITHUB_CLI", "1")
            .env("AHU_GITHUB_PATH", "/untrusted/ambient/path")
            .current_dir(&github.root)
            .args(["plugins", "invoke", "github", "--input-file", "input.json"])
            .output()
            .unwrap()
    }

    fn result(&self, github: &Github, input: &Value) -> Value {
        let output = self.invoke(github, input);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        assert!(output.stdout.len() < 1024);
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["result"].clone()
    }
}

fn real_github_fixture() -> Github {
    let mut github = Github::new();
    github.binding.grants.push(Grant::GithubCli);
    github.save(true);
    github.install();
    github
}

#[test]
fn inspection_reads_only_bounded_state_with_fixed_get_argv() {
    let github = real_github_fixture();
    let fake = FakeGh::new();
    for mode in ["valid", "closed", "case", "public", "max_output"] {
        fake.mode(mode);
        let mut input = inspect_input();
        if mode == "public" {
            input["expected_visibility"] = json!("public");
        }
        let result = fake.result(&github, &input);
        let repository = if mode == "case" {
            "Example/Project"
        } else {
            "example/project"
        };
        assert_eq!(
            result,
            json!({"version": 2, "provider": "github", "outcome": "read",
            "code": null, "reference": {"repository": repository, "issue_number": 7,
            "url": format!("https://github.com/{repository}/issues/7")},
            "state": {"status": if mode == "closed" { "closed" } else { "open" },
            "visibility": input["expected_visibility"]}}),
            "{mode}"
        );
        assert_eq!(
            fake.calls(),
            vec![
                json!([
                    "api",
                    "--hostname",
                    "github.com",
                    "--method",
                    "GET",
                    "repos/example/project",
                    "--jq",
                    "{full_name, html_url, private, visibility}"
                ]),
                json!([
                    "api",
                    "--hostname",
                    "github.com",
                    "--method",
                    "GET",
                    format!("repos/{repository}/issues/7"),
                    "--jq",
                    "{number, html_url, repository_url, state, is_pull_request: has(\"pull_request\")}"
                ])
            ]
        );
    }
}

#[test]
fn inspection_refuses_mismatches_and_provider_failures_without_disclosing_data() {
    let github = real_github_fixture();
    let fake = FakeGh::new();
    for (mode, code, calls) in [
        ("repo_mismatch", "repository_mismatch", 1),
        ("repo_url", "repository_mismatch", 1),
        ("public", "visibility_mismatch", 1),
        ("inconsistent_visibility", "visibility_mismatch", 1),
        ("issue_number", "issue_mismatch", 2),
        ("issue_repo", "issue_mismatch", 2),
        ("issue_url", "issue_mismatch", 2),
        ("pull_request", "issue_mismatch", 2),
        ("failure", "provider_failed", 1),
        ("nonzero_valid", "provider_failed", 1),
        ("issue_failure", "provider_failed", 2),
        ("issue_large", "provider_output_limit", 2),
        ("large", "provider_output_limit", 1),
        ("timeout", "provider_timeout", 1),
        ("malformed", "provider_malformed", 1),
        ("duplicate", "provider_malformed", 1),
        ("deep", "provider_malformed", 1),
        ("trailing", "provider_malformed", 1),
        ("invalid_utf8", "provider_malformed", 1),
        ("bad_state", "provider_malformed", 2),
        ("float_number", "provider_malformed", 2),
        ("extra", "provider_malformed", 2),
        ("missing", "provider_malformed", 2),
    ] {
        fake.mode(mode);
        assert_eq!(
            fake.result(&github, &inspect_input()),
            json!({"version": 2,
            "provider": "github", "outcome": "error", "code": code,
            "reference": null, "state": null}),
            "{mode}"
        );
        assert_eq!(fake.calls().len(), calls, "{mode}");
    }
    fake.mode("valid");
    let mut input = inspect_input();
    input["expected_visibility"] = json!("public");
    assert_eq!(fake.result(&github, &input)["code"], "visibility_mismatch");
    assert_eq!(fake.calls().len(), 1);
}

#[test]
fn inspection_validates_input_before_any_gh_invocation() {
    let github = real_github_fixture();
    let fake = FakeGh::new();
    for (key, value) in [
        ("repository", json!("")),
        ("repository", json!("owner")),
        ("repository", json!("https://github.com/example/project")),
        ("repository", json!("example/../project")),
        ("repository", json!("example/..")),
        ("repository", json!("-owner/project")),
        ("repository", json!("owner/repo;echo secret")),
        ("repository", json!("owner/repo\n")),
        ("repository", json!("a".repeat(141))),
        ("issue_number", json!(0)),
        ("issue_number", json!(-1)),
        ("issue_number", json!(true)),
        ("issue_number", json!(7.0)),
        ("issue_number", json!(2147483648_u64)),
        ("issue_number", json!("7")),
        ("expected_visibility", json!("any")),
        ("action", json!("close")),
        ("unexpected", json!("secret")),
    ] {
        let mut input = inspect_input();
        input[key] = value;
        assert_eq!(
            fake.result(&github, &input)["code"],
            "invalid_input",
            "{input}"
        );
    }
    for key in ["repository", "issue_number", "expected_visibility"] {
        let mut input = inspect_input();
        input.as_object_mut().unwrap().remove(key);
        assert_eq!(fake.result(&github, &input)["code"], "invalid_input");
    }
    assert!(fake.calls().is_empty());
}

#[test]
fn inspection_is_inert_without_grant_enablement_installation_or_binding() {
    let mut github = real_github_fixture();
    let fake = FakeGh::new();
    github.binding.grants = vec![Grant::Execute];
    github.save(true);
    assert_eq!(
        fake.result(&github, &inspect_input())["code"],
        "github_cli_denied"
    );
    github.binding.grants.push(Grant::GithubCli);
    github.binding.enabled = false;
    github.save(true);
    let output = fake.invoke(&github, &inspect_input());
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
        "denied"
    );
    github.binding.enabled = true;
    github.save(true);
    fs::remove_dir_all(github.root.join(".ahu/state/plugins")).unwrap();
    let output = fake.invoke(&github, &inspect_input());
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
        "unavailable"
    );
    fs::remove_file(github.root.join(".agents/ahu/plugins/github.md")).unwrap();
    common::git(&github.root, &["add", ".agents/ahu/plugins/github.md"]);
    common::git(
        &github.root,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "remove fixture binding",
        ],
    );
    let output = fake.invoke(&github, &inspect_input());
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stderr).unwrap()["code"],
        "denied"
    );
    assert!(fake.calls().is_empty());
}

#[test]
fn inspection_missing_gh_and_unsafe_lookup_fail_closed() {
    // Invoke the real entrypoint with an explicit fixture-only search path; no
    // machine gh can be selected, even if it is installed in a system directory.
    let fake = FakeGh::new();
    for path in [
        "".to_owned(),
        ".".to_owned(),
        fake.temp.path().display().to_string(),
    ] {
        fs::write(fake.temp.path().join(".git"), "synthetic checkout marker").unwrap();
        let entrypoint = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/github/entrypoint");
        let mut child = Command::new(entrypoint)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("AHU_GITHUB_CLI", "1")
            .env("AHU_GITHUB_PATH", path)
            .current_dir(fake.temp.path())
            .arg("--ahu-plugin-protocol=1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        serde_json::to_writer(
            child.stdin.take().unwrap(),
            &json!({"protocol_version": 1,
            "plugin_id": "github", "grant": {"operation": "invoke"}, "context": {},
            "input": inspect_input()}),
        )
        .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap()["result"]["code"],
            "gh_unavailable"
        );
    }
    assert!(fake.calls().is_empty());
}

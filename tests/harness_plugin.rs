//! Independent executable and pinned installation; no native Codex or account required.
mod common;
use ahu::agent::Permissions;
use ahu::harness::{Adapter, LaunchRequest};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

// Build explicitly in the test harness, never through plugin installation.
fn planner_executable() -> &'static Path {
    static EXECUTABLE: OnceLock<PathBuf> = OnceLock::new();
    EXECUTABLE.get_or_init(|| {
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("ahu-rust-codex-agent-target"));
        let target = std::path::absolute(target).unwrap();
        let output = Command::new(env!("CARGO"))
            .args(["build", "--locked", "--offline", "--manifest-path"])
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("examples/plugins/harness-codex/Cargo.toml"),
            )
            .env("CARGO_TARGET_DIR", &target)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        target.join("debug/ahu-experiment-harness-codex")
    })
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    marker: PathBuf,
    binding: PathBuf,
}
impl Fixture {
    fn new(script: Option<&str>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("repo");
        let package = base.join("package");
        let marker = base.join("started");
        fs::create_dir_all(root.join(".agents/ahu/plugins")).unwrap();
        fs::create_dir(&package).unwrap();
        common::git(&root, &["init", "-q"]);
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/plugins/harness-codex");
        for name in ["Cargo.toml", "Cargo.lock", "ahu-plugin.md"] {
            fs::copy(source.join(name), package.join(name)).unwrap();
        }
        fs::create_dir(package.join("bin")).unwrap();
        let executable = package.join("bin/ahu-experiment-harness-codex");
        if let Some(script) = script {
            let body = script.split_once('\n').unwrap().1;
            fs::write(
                &executable,
                format!(
                    "#!/usr/bin/python3\nwith open({}, 'a') as f: f.write('start\\n')\n{}",
                    json!(marker),
                    body
                ),
            )
            .unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        } else {
            fs::copy(planner_executable(), executable).unwrap();
        }
        let digest = ahu::plugin_install::digest(&package).unwrap();
        let binding = root.join(".agents/ahu/plugins/harness-codex.md");
        fs::write(&binding, format!("---\nokf_version: '0.2'\ntype: ahu:plugin-binding\nahu:\n  schema_version: 1\n  id: harness-codex\n  version: '0.1.0'\n  source:\n    kind: local\n    repository: {}\n    revision: '{digest}'\n    sha256: '{digest}'\n  compatibility:\n    ahu: '{}'\n    host_api: 1\n  enabled: true\n  grants: [harness-launch]\n---\nSynthetic policy.\n", json!(package),env!("CARGO_PKG_VERSION"))).unwrap();
        let fixture = Self {
            _temp: temp,
            root,
            marker,
            binding,
        };
        fixture.commit();
        let output = common::ahu()
            .args([
                "--repo",
                fixture.root.to_str().unwrap(),
                "plugins",
                "install",
                "harness-codex",
            ])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(!fixture.marker.exists());
        fs::remove_dir_all(package).unwrap(); // use only the installed package
        fixture
    }
    fn commit(&self) {
        common::git(
            &self.root,
            &["add", "--", ".agents/ahu/plugins/harness-codex.md"],
        );
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
                "Synthetic policy",
            ],
        );
    }
    fn plan(
        &self,
        permissions: Permissions,
        spec: Option<&ahu::headless::Spec>,
    ) -> ahu::util::Result<ahu::harness::LaunchCommand> {
        ahu::harness_plugin::plan(
            &ahu::git::discover(&self.root).unwrap(),
            "codex",
            &LaunchRequest {
                model: "gpt-6-astra",
                prompt: "--private literal task",
                cwd: &self.root,
                permissions,
            },
            spec,
        )
    }
}
fn spec(session: Option<&str>) -> ahu::headless::Spec {
    serde_json::from_value(json!({"schema_version":2,"options":{"work_mode":true,"background":false,"timeout_seconds":30,"native_helpers":"disabled"},"harness_version":"codex-cli 0.163.0","executable_digest":"synthetic","parent_task":null,"root_task":null,"broker_request":null,"depth":0,"attempt":1,"session":session,"native_controls":[],"gaps":[]})).unwrap()
}

#[test]
fn installed_planner_matches_native_commands_without_harness_or_prompt_access() {
    let f = Fixture::new(None);
    for permissions in [
        Permissions::Prompt,
        Permissions::AcceptEdits,
        Permissions::Auto,
    ] {
        let expected = ahu::harness::codex::Codex
            .launch_command(&LaunchRequest {
                model: "gpt-6-astra",
                prompt: "--private literal task",
                cwd: &f.root,
                permissions,
            })
            .unwrap();
        let mut actual = f.plan(permissions, None).unwrap();
        assert!(actual.planner_binding_digest.take().is_some());
        assert_eq!(actual, expected);
    }
    for session in [None, Some("synthetic-session")] {
        let spec = spec(session);
        let expected = ahu::headless::batch_command(
            "codex",
            &LaunchRequest {
                model: "gpt-6-astra",
                prompt: "--private literal task",
                cwd: &f.root,
                permissions: Permissions::Auto,
            },
            &spec,
        )
        .unwrap();
        let mut actual = f.plan(Permissions::Auto, Some(&spec)).unwrap();
        assert!(actual.planner_binding_digest.take().is_some());
        assert_eq!(actual, expected);
    }
    let mut guarded = spec(None);
    guarded.options.work_mode = false;
    assert!(f.plan(Permissions::Auto, Some(&guarded)).is_err());
}

#[test]
fn policy_changes_refuse_before_execution_and_no_generic_grant_substitutes() {
    let f = Fixture::new(Some(
        "#!/usr/bin/python3\nraise AssertionError('must not execute')\n",
    ));
    let mut guarded = spec(None);
    guarded.options.work_mode = false;
    assert!(f.plan(Permissions::Auto, Some(&guarded)).is_err());
    assert!(!f.marker.exists());
    let original = fs::read_to_string(&f.binding).unwrap();
    for changed in [
        original.replace("enabled: true", "enabled: false"),
        original.replace("[harness-launch]", "[execute]"),
        original.replace("[harness-launch]", "[]"),
    ] {
        fs::write(&f.binding, changed).unwrap();
        f.commit();
        assert!(f.plan(Permissions::Auto, None).is_err());
        assert!(!f.marker.exists());
    }
    fs::remove_file(&f.binding).unwrap();
    f.commit();
    assert!(
        f.plan(Permissions::Auto, None)
            .unwrap_err()
            .to_string()
            .contains("no built-in fallback")
    );
    assert!(!f.marker.exists());
}

#[test]
fn malformed_and_failed_planners_do_not_yield_launch_commands() {
    for script in [
        "#!/usr/bin/python3\nprint('not JSON')\n",
        r#"#!/usr/bin/python3
print('{"protocol_version":1,"protocol_version":1,"harness":"codex","program":"codex","args":["<prompt: see prompt.txt>"],"prompt_arg":0}')
"#,
        r#"#!/usr/bin/python3
print('{"protocol_version":1,"harness":"codex","program":"codex","args":["<prompt: see prompt.txt>"],"prompt_arg":0,"unknown":true}')
"#,
        r#"#!/usr/bin/python3
print('{"protocol_version":1,"harness":"codex","program":"sh","args":["<prompt: see prompt.txt>"],"prompt_arg":0}')
"#,
        r#"#!/usr/bin/python3
print('{"protocol_version":1,"harness":"codex","program":"codex","args":["<prompt: see prompt.txt>","extra"],"prompt_arg":0}')
"#,
        "#!/usr/bin/python3\nimport sys\nsys.exit(1)\n",
        "#!/usr/bin/python3\nprint('x'*65537)\n",
    ] {
        let f = Fixture::new(Some(script));
        assert!(f.plan(Permissions::Auto, None).is_err());
        assert!(f.marker.exists());
    }
}

#[test]
fn independent_plugin_rejects_unknown_request_fields_and_never_receives_prompt() {
    let f = Fixture::new(Some(
        "#!/usr/bin/python3\nimport json,sys\nr=json.load(sys.stdin)\nassert set(r)=={'protocol_version','harness','mode','model','permissions','work_mode','session'}\nassert 'private literal' not in json.dumps(r)\nprint(json.dumps({'protocol_version':1,'harness':'codex','program':'codex','args':['--','<prompt: see prompt.txt>'],'prompt_arg':1}))\n",
    ));
    assert_eq!(
        f.plan(Permissions::Auto, None).unwrap().args[1],
        "--private literal task"
    );
    let mut child = Command::new(planner_executable())
        .arg("--ahu-harness-launch-protocol=1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"protocol_version":1,"unexpected":true}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}

#[test]
fn changing_committed_planner_policy_changes_frozen_command_identity() {
    let f = Fixture::new(None);
    let before = f.plan(Permissions::Auto, None).unwrap();
    let original = fs::read_to_string(&f.binding).unwrap();
    fs::write(
        &f.binding,
        original.replace("[harness-launch]", "[harness-launch, read-project-context]"),
    )
    .unwrap();
    f.commit();
    let after = f.plan(Permissions::Auto, None).unwrap();
    assert_eq!(before.args, after.args);
    assert_ne!(before.redacted(), after.redacted());
    let encoded = serde_json::to_vec(&before.redacted()).unwrap();
    let restored: ahu::harness::LaunchCommand = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(
        restored.planner_binding_digest,
        before.planner_binding_digest
    );
    let mut built_in = restored.clone();
    built_in.planner_binding_digest = None;
    assert_ne!(restored, built_in);
}

#[test]
fn planner_timeout_returns_no_command() {
    let f = Fixture::new(Some("#!/usr/bin/python3\nimport time\ntime.sleep(30)\n"));
    let error = f.plan(Permissions::Auto, None).unwrap_err();
    assert!(error.to_string().contains("TimedOut"), "{error}");
    assert!(f.marker.exists());
}

fn exchange(raw: &[u8]) -> std::process::Output {
    use std::io::Write;
    let mut child = Command::new(planner_executable())
        .arg("--ahu-harness-launch-protocol=1")
        .env_clear()
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(raw).unwrap();
    child.wait_with_output().unwrap()
}

fn valid_request() -> serde_json::Value {
    json!({"protocol_version":1,"harness":"codex","mode":"interactive",
        "model":"example-model","permissions":"prompt","work_mode":false,"session":null})
}

#[test]
fn rust_planner_rejects_invalid_requests_without_output() {
    let valid = valid_request();
    let mut invalid = vec![];
    for field in valid.as_object().unwrap().keys() {
        let mut request = valid.clone();
        request.as_object_mut().unwrap().remove(field);
        invalid.push(serde_json::to_vec(&request).unwrap());
    }
    for (field, values) in [
        (
            "protocol_version",
            vec![json!(true), json!(1.0), json!(2), json!("1")],
        ),
        ("harness", vec![json!("other"), json!(null)]),
        (
            "model",
            vec![json!(""), json!("-flag"), json!("a\0b"), json!(0)],
        ),
        (
            "session",
            vec![json!(""), json!("a\0b"), json!(0), json!("resume")],
        ),
        ("mode", vec![json!("other"), json!(null), json!("headless")]),
        ("permissions", vec![json!("other"), json!(null)]),
        ("work_mode", vec![json!(true), json!(0), json!(null)]),
        ("unknown", vec![json!(true)]),
    ] {
        for value in values {
            let mut request = valid.clone();
            request[field] = value;
            invalid.push(serde_json::to_vec(&request).unwrap());
        }
    }
    let encoded = serde_json::to_string(&valid).unwrap();
    for field in valid.as_object().unwrap().keys() {
        invalid.push(format!("{{{field:?}:{},{}", valid[field], &encoded[1..]).into_bytes());
    }
    invalid.extend([
        b"null".to_vec(),
        b"[]".to_vec(),
        b"{".to_vec(),
        format!("{encoded}{{}}").into_bytes(),
        vec![b' '; 65_537],
        vec![0xff],
    ]);
    let mut headless = valid.clone();
    headless["mode"] = json!("headless");
    headless["work_mode"] = json!(true);
    for permission in ["prompt", "accept-edits"] {
        headless["permissions"] = json!(permission);
        invalid.push(serde_json::to_vec(&headless).unwrap());
    }
    for raw in invalid {
        let output = exchange(&raw);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
    }
    for args in [
        vec![],
        vec!["--ahu-harness-launch-protocol=2"],
        vec!["--ahu-harness-launch-protocol=1", "extra"],
    ] {
        let output = Command::new(planner_executable())
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn rust_planner_preserves_ascii_framing_and_exact_limits() {
    let mut request = valid_request();
    request["model"] = json!("é😀\u{7f}");
    let output = exchange(&serde_json::to_vec(&request).unwrap());
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        concat!(
            "{\"protocol_version\":1,\"harness\":\"codex\",\"program\":\"codex\",",
            "\"args\":[\"-m\",\"\\u00e9\\ud83d\\ude00\\u007f\",\"--\",\"<prompt: see prompt.txt>\"],\"prompt_arg\":3}\n"
        )
    );
    let mut raw = serde_json::to_vec(&valid_request()).unwrap();
    raw.resize(65_536, b' ');
    assert!(exchange(&raw).status.success());
    raw.push(b' ');
    assert_eq!(exchange(&raw).status.code(), Some(2));

    request["model"] = json!("a");
    let baseline = exchange(&serde_json::to_vec(&request).unwrap())
        .stdout
        .len();
    let model_bytes = 65_536 - baseline + 1;
    let model = "é".repeat(model_bytes / 6) + &"a".repeat(model_bytes % 6);
    request["model"] = json!(model);
    let output = exchange(&serde_json::to_vec(&request).unwrap());
    assert!(output.status.success());
    assert_eq!(output.stdout.len(), 65_536);
    request["model"] = json!(model + "a");
    let output = exchange(&serde_json::to_vec(&request).unwrap());
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    request["model"] = json!("é".repeat(11_000));
    let raw = serde_json::to_vec(&request).unwrap();
    assert!(raw.len() < 65_536);
    let output = exchange(&raw);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}

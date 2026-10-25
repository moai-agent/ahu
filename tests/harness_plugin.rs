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
fn planner_executable_for(harness: &str) -> PathBuf {
    static EXECUTABLES: OnceLock<std::sync::Mutex<std::collections::HashMap<String, PathBuf>>> =
        OnceLock::new();
    let mut executables = EXECUTABLES.get_or_init(Default::default).lock().unwrap();
    executables
        .entry(harness.into())
        .or_insert_with(|| {
            let target = std::env::var_os("CARGO_TARGET_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::temp_dir().join("ahu-rust-codex-agent-target"));
            let target = std::path::absolute(target).unwrap();
            let output = Command::new(env!("CARGO"))
                .args(["build", "--locked", "--offline", "--manifest-path"])
                .arg(
                    Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join(format!("plugins/harness-{harness}/Cargo.toml")),
                )
                .env("CARGO_TARGET_DIR", &target)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            target.join(format!("debug/ahu-plugin-{harness}"))
        })
        .clone()
}

fn planner_executable() -> PathBuf {
    planner_executable_for("codex")
}

fn native_fixture(name: &str) -> bool {
    if common::is_child_case(name) {
        return true;
    }
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    for (program, version) in [
        ("codex", "0.163.0"),
        ("claude", "2.1.310"),
        ("agy", "1.4.7"),
        ("opencode", "2.0.32"),
    ] {
        let binary = dir.path().join(program);
        fs::write(&binary, format!("#!/bin/sh\nprintf '{version}\\n'\n")).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = common::run_child_case(name, |command| {
        command.env("PATH", path);
    });
    common::assert_child_passed(name, &output);
    false
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    marker: PathBuf,
    binding: PathBuf,
    harness: String,
}
impl Fixture {
    fn new(script: Option<&str>) -> Self {
        Self::for_harness("codex", script)
    }
    fn for_harness(harness: &str, script: Option<&str>) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("repo");
        let package = base.join("package");
        let marker = base.join("started");
        fs::create_dir_all(root.join(".agents/ahu/plugins")).unwrap();
        fs::create_dir(&package).unwrap();
        common::git(&root, &["init", "-q"]);
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("plugins/harness-{harness}"));
        for name in ["Cargo.toml", "Cargo.lock", "ahu-plugin.md"] {
            fs::copy(source.join(name), package.join(name)).unwrap();
        }
        fs::create_dir(package.join("bin")).unwrap();
        let executable = package.join(format!("bin/ahu-plugin-{harness}"));
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
            fs::copy(planner_executable_for(harness), executable).unwrap();
        }
        let digest = ahu::plugin_install::digest(&package).unwrap();
        let binding = root.join(format!(".agents/ahu/plugins/harness-{harness}.md"));
        fs::write(&binding, format!("---\nokf_version: '0.2'\ntype: ahu:plugin-binding\nahu:\n  schema_version: 1\n  id: harness-{harness}\n  version: '0.1.0'\n  source:\n    kind: local\n    repository: {}\n    revision: '{digest}'\n    sha256: '{digest}'\n  compatibility:\n    ahu: '{}'\n    host_api: 1\n  enabled: true\n  grants: [harness-launch]\n---\nSynthetic policy.\n", json!(package),env!("CARGO_PKG_VERSION"))).unwrap();
        let fixture = Self {
            _temp: temp,
            root,
            marker,
            binding,
            harness: harness.into(),
        };
        fixture.commit();
        let output = common::ahu()
            .args([
                "--repo",
                fixture.root.to_str().unwrap(),
                "plugins",
                "install",
                &format!("harness-{harness}"),
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
            &[
                "add",
                "--",
                &format!(".agents/ahu/plugins/harness-{}.md", self.harness),
            ],
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
fn installed_planner_preserves_permissions_and_adds_interactive_server_ownership() {
    if !native_fixture(
        "installed_planner_preserves_permissions_and_adds_interactive_server_ownership",
    ) {
        return;
    }
    let f = Fixture::new(None);
    for permissions in [
        Permissions::Prompt,
        Permissions::AcceptEdits,
        Permissions::Auto,
    ] {
        let mut expected = ahu::harness::codex::Codex
            .launch_command(&LaunchRequest {
                model: "gpt-6-astra",
                prompt: "--private literal task",
                cwd: &f.root,
                permissions,
            })
            .unwrap();
        // The pilot deliberately differs from the built-in adapter only in
        // native server ownership. Keep model, approval and prompt parity.
        expected.args.insert(2, "--no-daemon".into());
        expected.prompt_arg = expected.prompt_arg.map(|index| index + 1);
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
    if !native_fixture("policy_changes_refuse_before_execution_and_no_generic_grant_substitutes") {
        return;
    }
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
    if !native_fixture("malformed_and_failed_planners_do_not_yield_launch_commands") {
        return;
    }
    for script in [
        "#!/usr/bin/python3\nprint('not JSON')\n",
        r#"#!/usr/bin/python3
print('{"protocol_version":2,"protocol_version":2,"harness":"codex","program":"codex","args":["<prompt: see prompt.txt>"],"prompt_arg":0}')
"#,
        r#"#!/usr/bin/python3
print('{"protocol_version":2,"harness":"codex","program":"codex","args":["<prompt: see prompt.txt>"],"prompt_arg":0,"unknown":true}')
"#,
        r#"#!/usr/bin/python3
print('{"protocol_version":2,"harness":"codex","program":"sh","args":["<prompt: see prompt.txt>"],"prompt_arg":0}')
"#,
        r#"#!/usr/bin/python3
print('{"protocol_version":2,"harness":"codex","program":"codex","args":["<prompt: see prompt.txt>","extra"],"prompt_arg":0}')
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
    if !native_fixture(
        "independent_plugin_rejects_unknown_request_fields_and_never_receives_prompt",
    ) {
        return;
    }
    let f = Fixture::new(Some(
        "#!/usr/bin/python3\nimport json,sys\nr=json.load(sys.stdin)\nassert set(r)=={'protocol_version','harness','mode','model','permissions','work_mode','session','native_version','timeout_seconds'}\nassert 'private literal' not in json.dumps(r)\nprint(json.dumps({'protocol_version':2,'harness':'codex','program':'codex','args':['--','<prompt: see prompt.txt>'],'prompt_arg':1}))\n",
    ));
    assert_eq!(
        f.plan(Permissions::Auto, None).unwrap().args[1],
        "--private literal task"
    );
    let mut child = Command::new(planner_executable())
        .arg("--ahu-harness-launch-protocol=2")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"protocol_version":2,"unexpected":true}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}

#[test]
fn changing_committed_planner_policy_changes_frozen_command_identity() {
    if !native_fixture("changing_committed_planner_policy_changes_frozen_command_identity") {
        return;
    }
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
    if !native_fixture("planner_timeout_returns_no_command") {
        return;
    }
    let f = Fixture::new(Some("#!/usr/bin/python3\nimport time\ntime.sleep(30)\n"));
    let error = f.plan(Permissions::Auto, None).unwrap_err();
    assert!(error.to_string().contains("TimedOut"), "{error}");
    assert!(f.marker.exists());
}

fn exchange(raw: &[u8]) -> std::process::Output {
    use std::io::Write;
    let mut child = Command::new(planner_executable())
        .arg("--ahu-harness-launch-protocol=2")
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
    json!({"protocol_version":2,"harness":"codex","mode":"interactive",
        "model":"example-model","permissions":"prompt","work_mode":false,"session":null,"native_version":"0.163.0","timeout_seconds":1800})
}

#[test]
fn rust_planner_rejects_invalid_requests_without_output() {
    if !native_fixture("rust_planner_rejects_invalid_requests_without_output") {
        return;
    }
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
            vec![json!(true), json!(2.0), json!(1), json!("2")],
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
        assert!(output.stderr.len() < 128);
    }
    for args in [
        vec![],
        vec!["--ahu-harness-launch-protocol=1"],
        vec!["--ahu-harness-launch-protocol=2", "extra"],
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
    if !native_fixture("rust_planner_preserves_ascii_framing_and_exact_limits") {
        return;
    }
    let mut request = valid_request();
    request["model"] = json!("example-model");
    let output = exchange(&serde_json::to_vec(&request).unwrap());
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        concat!(
            "{\"protocol_version\":2,\"harness\":\"codex\",\"program\":\"codex\",",
            "\"args\":[\"-m\",\"example-model\",\"--no-daemon\",\"--\",\"<prompt: see prompt.txt>\"],\"prompt_arg\":4}\n"
        )
    );
    let mut raw = serde_json::to_vec(&request).unwrap();
    raw.resize(65_536, b' ');
    assert!(exchange(&raw).status.success());
    raw.push(b' ');
    assert_eq!(exchange(&raw).status.code(), Some(2));
    request["model"] = json!("a".repeat(256));
    assert!(
        exchange(&serde_json::to_vec(&request).unwrap())
            .status
            .success()
    );
    for model in ["a".repeat(257), "é😀".into(), "a\u{7f}".into()] {
        request["model"] = json!(model);
        let output = exchange(&serde_json::to_vec(&request).unwrap());
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, b"invalid model\n");
    }
}

#[cfg(feature = "harness-plugin-pilot")]
#[test]
fn headless_resume_keeps_installed_planner_identity() {
    if !native_fixture("headless_resume_keeps_installed_planner_identity") {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new(None);
    let external = tempfile::tempdir().unwrap();
    let bin = external.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let native = bin.join("codex");
    fs::write(
        &native,
        r#"#!/usr/bin/python3
import sys,json
if '--version' in sys.argv:
 print('codex-cli 0.163.0'); sys.exit(0)
a=sys.argv[1:]
assert a[0]=='exec'
if a[1]=='resume': assert a[-2]=='synthetic-session'
print(json.dumps({'type':'thread.started','thread_id':'synthetic-session'}),flush=True)
print(json.dumps({'type':'turn.completed'}),flush=True)
"#,
    )
    .unwrap();
    fs::set_permissions(&native, fs::Permissions::from_mode(0o755)).unwrap();
    fs::create_dir_all(f.root.join(".agents/ahu/agents")).unwrap();
    fs::write(f.root.join(".agents/ahu/agents/probe.md"), "---\nokf_version: '0.2'\ntype: ahu:agent\ntitle: probe\ndescription: Synthetic probe\nharness: codex\nmodel: gpt-6-astra\npermissions: auto\nversion: 1.0.0\n---\nPerform synthetic work.\n").unwrap();
    fs::write(f.root.join(".agents/ahu/config.md"), format!("---\nokf_version: '0.2'\ntype: ahu:project\nahu:\n  schema_version: 1\n  catalog_version: '{}'\n  harness_preferences: [codex]\n  model_selection: project-ranked\n  model_rankings:\n    codex: [gpt-6-astra]\n---\nSynthetic configuration.\n", ahu::catalog::CATALOG_VERSION)).unwrap();
    fs::write(f.root.join(".gitignore"), ".ahu/\n.worktrees/\n").unwrap();
    let command = || {
        let mut command = common::ahu();
        command
            .current_dir(&f.root)
            .env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("HOME", external.path())
            .env("XDG_STATE_HOME", external.path().join("state"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null");
        command
    };
    let success = |output: std::process::Output| {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };
    success(command().args(["lock", "--update"]).output().unwrap());
    common::git(&f.root, &["add", ".agents", ".gitignore"]);
    f.commit();
    let initial: serde_json::Value = serde_json::from_slice(&success(
        command()
            .args([
                "@probe",
                "--headless",
                "--work-mode",
                "--allow-widened-approvals",
                "--prompt",
                "synthetic first attempt",
                "--output",
                "json",
            ])
            .output()
            .unwrap(),
    ))
    .unwrap();
    assert_eq!(initial["outcome"], "succeeded");
    let id = initial["task_id"].as_str().unwrap();
    let initial_result_path = PathBuf::from(initial["review"]["result_path"].as_str().unwrap());
    let initial_task =
        ahu::task::load(initial_result_path.parent().unwrap().parent().unwrap()).unwrap();
    let initial_planner = initial_task.launch_command.planner_binding_digest;
    assert!(initial_planner.is_some());
    let prompt = external.path().join("followup.txt");
    fs::write(&prompt, "synthetic followup").unwrap();
    // A primary-checkout policy update must not silently select a new planner
    // identity for a task whose worktree still carries the original binding.
    let original_binding = fs::read_to_string(&f.binding).unwrap();
    fs::write(
        &f.binding,
        original_binding.replace("[harness-launch]", "[harness-launch, execute]"),
    )
    .unwrap();
    f.commit();
    let refused = command()
        .args(["resume", id, "--prompt-file"])
        .arg(&prompt)
        .args(["--output", "json"])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("changed identity"),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    let unchanged =
        ahu::task::load(initial_result_path.parent().unwrap().parent().unwrap()).unwrap();
    assert_eq!(
        unchanged.launch_command.planner_binding_digest,
        initial_planner
    );
    fs::write(&f.binding, original_binding).unwrap();
    f.commit();
    success(
        command()
            .args(["resume", id, "--prompt-file"])
            .arg(&prompt)
            .args(["--output", "json"])
            .output()
            .unwrap(),
    );
    let resumed: serde_json::Value = serde_json::from_slice(&success(
        command()
            .args(["wait", id, "--output", "json"])
            .output()
            .unwrap(),
    ))
    .unwrap();
    assert_eq!(resumed["outcome"], "succeeded");
    assert_eq!(resumed["attempt"], 2);
    assert_eq!(resumed["harness"]["session"], "synthetic-session");
    let result_path = PathBuf::from(resumed["review"]["result_path"].as_str().unwrap());
    let task = ahu::task::load(result_path.parent().unwrap().parent().unwrap()).unwrap();
    assert_eq!(task.launch_command.planner_binding_digest, initial_planner);
    success(command().args(["cleanup", id]).output().unwrap());
    success(command().args(["remove", id]).output().unwrap());
}

#[test]
fn four_installed_packages_plan_native_programs_and_refuse_unknown_versions() {
    if !native_fixture("four_installed_packages_plan_native_programs_and_refuse_unknown_versions") {
        return;
    }
    for (harness, program, version, model) in [
        ("codex", "codex", "0.163.0", "gpt-6-astra"),
        ("claude-code", "claude", "2.1.310", "sonnet"),
        ("antigravity", "agy", "1.4.7", "gemini-3-pro"),
        ("opencode", "opencode", "2.0.32", "example/model"),
    ] {
        let fixture = Fixture::for_harness(harness, None);
        let repo = ahu::git::discover(&fixture.root).unwrap();
        let request = LaunchRequest {
            model,
            prompt: "--literal private prompt",
            cwd: &fixture.root,
            permissions: Permissions::Auto,
        };
        for session in [None, Some("synthetic-session")] {
            let mut batch = spec(session);
            batch.harness_version = version.into();
            let plan = ahu::harness_plugin::plan(&repo, harness, &request, Some(&batch)).unwrap();
            assert_eq!(plan.program, program);
            assert_eq!(plan.args.last().unwrap(), request.prompt);
            assert!(plan.planner_binding_digest.is_some());
            batch.harness_version = "999.0.0".into();
            assert!(ahu::harness_plugin::plan(&repo, harness, &request, Some(&batch)).is_err());
        }
        let interactive = LaunchRequest {
            permissions: Permissions::Prompt,
            ..request
        };
        let plan = ahu::harness_plugin::plan(&repo, harness, &interactive, None).unwrap();
        assert_eq!(plan.program, program);
        assert_eq!(plan.args.last().unwrap(), interactive.prompt);
        if harness == "opencode" {
            assert_eq!(plan.args[0], "mini");
            let auto = LaunchRequest {
                permissions: Permissions::Auto,
                ..interactive
            };
            assert!(ahu::harness_plugin::plan(&repo, harness, &auto, None).is_err());
        }
    }
}

#[cfg(feature = "harness-plugin-pilot")]
#[test]
fn cli_headless_planning_does_not_require_interactive_auto_support() {
    if !native_fixture("cli_headless_planning_does_not_require_interactive_auto_support") {
        return;
    }
    for (harness, model, first_arg) in [
        ("codex", "gpt-6-astra", "exec"),
        ("claude-code", "claude-sonnet-5", "--print"),
        ("antigravity", "gemini-3.1-pro-high", "--model"),
        ("opencode", "ollama/glm-5.3:cloud", "run"),
    ] {
        let f = Fixture::for_harness(harness, None);
        fs::create_dir_all(f.root.join(".agents/ahu/agents")).unwrap();
        fs::write(f.root.join(".agents/ahu/agents/probe.md"), format!("---\nokf_version: '0.2'\ntype: ahu:agent\ntitle: probe\ndescription: Synthetic probe\nharness: {harness}\nmodel: {model}\npermissions: auto\nversion: 1.0.0\n---\nPerform synthetic work.\n")).unwrap();
        fs::write(f.root.join(".agents/ahu/config.md"), format!("---\nokf_version: '0.2'\ntype: ahu:project\nahu:\n  schema_version: 1\n  catalog_version: '{}'\n  harness_preferences: [{harness}]\n  model_selection: project-ranked\n  model_rankings:\n    {harness}: [{model}]\n---\nSynthetic configuration.\n", ahu::catalog::CATALOG_VERSION)).unwrap();
        fs::write(f.root.join(".gitignore"), ".ahu/\n.worktrees/\n").unwrap();
        let home = tempfile::tempdir().unwrap();
        let invoke = |args: &[&str]| {
            let output = common::ahu()
                .current_dir(&f.root)
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap())
                .env("HOME", home.path())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{harness}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        };
        invoke(&["lock", "--update"]);
        common::git(&f.root, &["add", ".agents", ".gitignore"]);
        f.commit();
        let preview: serde_json::Value = serde_json::from_slice(&invoke(&[
            "@probe",
            "--headless",
            "--work-mode",
            "--allow-widened-approvals",
            "--dry-run",
            "--prompt",
            "synthetic work",
            "--output",
            "json",
        ]))
        .unwrap();
        assert_eq!(preview["command"]["args"][0], first_arg);
        assert!(preview["command"]["planner_binding_digest"].is_string());
        assert!(!Path::new(preview["worktree"].as_str().unwrap()).exists());
    }
}

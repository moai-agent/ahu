//! Independent sixth-harness fixture: no native provider or shared wire serializer.
//! Python is development-only test tooling, never a shipped runtime implementation.
mod common;
use ahu::harness_runtime::{self, Runtime};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

const PEER: &str = r#"
import json, os, sys, time, subprocess
assert sys.argv[1:] == ['--ahu-harness-runtime-protocol=1']
assert 'HOME' not in os.environ and 'PATH' not in os.environ
sequence = 0
for line in sys.stdin:
    request = json.loads(line)
    assert set(request) == {'protocol_version', 'id', 'operation', 'params'}
    sequence += 1
    assert request['id'] == sequence and request['protocol_version'] == 1
    params = request['params'] or {}
    mode = params.get('mode', 'ok')
    if mode == 'block': time.sleep(30)
    if mode == 'signal_block':
        with open(params['ready'], 'w') as marker: marker.write('ready')
        time.sleep(30)
    if mode == 'stderr': os.write(2, b'x' * 2000000)
    if mode == 'large':
        os.write(1, b'x' * 1048577); time.sleep(30)
    if mode == 'partial':
        os.write(1, b'{'); sys.exit(0)
    if mode == 'malformed':
        print('native-secret', flush=True); continue
    if mode == 'duplicate':
        print('{"protocol_version":1,"id":1,"id":1,"result":null}', flush=True); continue
    if mode == 'nested_duplicate':
        print('{"protocol_version":1,"id":1,"result":{"x":1,"x":2}}', flush=True); continue
    response = {'protocol_version':1, 'id':sequence, 'result':{'count':sequence,'operation':request['operation']}}
    if 'native_executable' in params:
        assert set(params) == {'project','home','native_executable','environment'}
        assert set(params['environment']) == {'PATH'}
        assert os.path.isabs(params['project']) and os.path.isabs(params['home'])
        if request['operation'] == 'probe':
            response['result'] = {'version':'6.2.0','compatibility':None,'reviewed_version':False,'enforcement':{}}
        if request['operation'] == 'models':
            response['result'] = {'source':'native','models':['sixth/new-exact-model'],'entitlement_checked':False}
    if mode == 'child':
        child = subprocess.Popen(['/bin/sleep', '30'])
        response['result'] = {'child':child.pid, 'parent':os.getpid()}
    if mode == 'cwd': response['result'] = os.getcwd()
    if mode == 'mismatch': response['id'] += 1
    if mode == 'version': response['protocol_version'] = 2
    if mode == 'unknown': response['extra'] = True
    if mode == 'both': response['error'] = 'failed'
    if mode == 'error': response = {'protocol_version':1,'id':sequence,'error':'unsupported'}
    if mode == 'private_error': response = {'protocol_version':1,'id':sequence,'error':'native-secret'}
    if mode == 'missing': del response['result']
    if mode == 'null': response['result'] = None
    if mode in ['boundary', 'over_boundary']:
        response['result'] = ''
        size = 1048576 + (mode == 'over_boundary')
        response['result'] = 'x' * (size - len(json.dumps(response)) - 1)
    raw = json.dumps(response) + '\n'
    if mode == 'extra': raw += raw
    sys.stdout.write(raw); sys.stdout.flush()
    if mode == 'exit_failure': sys.exit(7)
    if mode == 'late_extra':
        sys.stdin.read(); print('native-secret', flush=True)
"#;
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    binding: PathBuf,
    marker: PathBuf,
}
impl Fixture {
    fn new(prefix: &str, mutate: impl FnOnce(&mut Value)) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("repo");
        let package = base.join("package");
        let marker = base.join("started");
        fs::create_dir_all(root.join(".agents/ahu/plugins")).unwrap();
        fs::create_dir(&package).unwrap();
        common::git(&root, &["init", "-q"]);
        fs::write(
            package.join("Cargo.toml"),
            "[package]\nname='sixth-package'\nversion='1.0.0'\nedition='2024'\n",
        )
        .unwrap();
        let mut declaration = json!({
            "protocol_version":1,"path":"peer","timeout_ms":300,
            "descriptor":{"id":"sixth","display_name":"Independent Sixth","program":"sixth-native",
                "aliases":["six"],"models":["invented/exact-v6"],"modes":["interactive","headless","direct"],
                "operations":["probe","models","launch","admission","setup","native_sources","auth","budget","trust_prepare","trust_cleanup","decode_start","decode_event","decode_finish","recover"],
                "capabilities":["synthetic"],"tested_versions":["6.0.0"],"context_paths":[".sixth/"],
                "skill_paths":[".sixth/skills"],"native_source_paths":[".sixth/agents"]}
        });
        mutate(&mut declaration);
        let valid_declaration =
            serde_json::from_value::<ahu_harness_api::Declaration>(declaration.clone())
                .ok()
                .filter(|d| d.validate().is_ok())
                .is_some_and(|d| {
                    ahu::catalog::validate_runtime_descriptors([&d.descriptor]).is_ok()
                });
        fs::write(package.join("ahu-plugin.md"), format!("---\nokf_version: '0.2'\ntype: ahu:plugin\nschema_version: 1\nid: independent-package\npackage: sixth-package\nhost_api: 1\ncargo_manifest: Cargo.toml\nharness_runtime: {}\n---\nSynthetic runtime.\n", declaration)).unwrap();
        fs::write(package.join("peer"), format!("#!/usr/bin/python3\nwith open({}, 'a') as marker: marker.write('start\\n')\n{prefix}\n{PEER}", json!(marker))).unwrap();
        fs::set_permissions(package.join("peer"), fs::Permissions::from_mode(0o755)).unwrap();
        let digest = ahu::plugin_install::digest(&package).unwrap();
        let binding = root.join(".agents/ahu/plugins/independent-package.md");
        fs::write(&binding, format!("---\nokf_version: '0.2'\ntype: ahu:plugin-binding\nahu:\n  schema_version: 1\n  id: independent-package\n  version: '1.0.0'\n  source:\n    kind: local\n    repository: {}\n    revision: '{digest}'\n    sha256: '{digest}'\n  compatibility:\n    ahu: '{}'\n    host_api: 1\n  enabled: true\n  grants: [harness-runtime]\n---\nSynthetic policy.\n", json!(package), env!("CARGO_PKG_VERSION"))).unwrap();
        let fixture = Self {
            _temp: temp,
            root,
            binding,
            marker,
        };
        fixture.commit();
        let output = common::ahu()
            .args(["--repo"])
            .arg(&fixture.root)
            .args(["plugins", "install", "independent-package"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success() || !valid_declaration,
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!fixture.marker.exists());
        fs::remove_dir_all(package).unwrap();
        fixture
    }
    fn repo(&self) -> ahu::git::Repo {
        ahu::git::discover(&self.root).unwrap()
    }
    fn load(&self) -> Runtime {
        harness_runtime::load(&self.repo(), "sixth").unwrap()
    }
    fn commit(&self) {
        common::git(&self.root, &["add", "--", ".agents/ahu/plugins"]);
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
                "Synthetic binding",
            ],
        );
    }
    fn policy(&self, from: &str, to: &str) {
        fs::write(
            &self.binding,
            fs::read_to_string(&self.binding).unwrap().replace(from, to),
        )
        .unwrap();
        self.commit();
    }
    fn installation(&self) -> PathBuf {
        let store = ahu::state::checkout_root(&self.repo().primary_root().unwrap())
            .unwrap()
            .join("plugins/independent-package");
        fs::read_dir(store).unwrap().next().unwrap().unwrap().path()
    }
}

#[test]
fn invented_harness_is_inert_owned_and_provider_independent() {
    let fixture = Fixture::new("", |_| {});
    let descriptors = harness_runtime::discover(&fixture.repo()).unwrap();
    assert_eq!(descriptors.len(), 1);
    assert_eq!(descriptors[0].id, "sixth");
    assert_eq!(descriptors[0].models, ["invented/exact-v6"]);
    let runtime = harness_runtime::load(&fixture.repo(), "six").unwrap();
    assert_eq!(runtime.descriptor(), &descriptors[0]);
    assert!(!fixture.marker.exists());
    assert_eq!(runtime.call("probe", json!({})).unwrap()["count"], 1);
    assert_eq!(
        runtime.call("models", json!({"mode":"null"})).unwrap(),
        Value::Null
    );
    let cwd = runtime.call("probe", json!({"mode":"cwd"})).unwrap();
    let snapshot = PathBuf::from(cwd.as_str().unwrap());
    assert!(!snapshot.starts_with(&fixture.root));
    assert_eq!(
        fs::metadata(&snapshot).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let mut session = runtime.open_stream().unwrap();
    drop(runtime); // Session retains the frozen executable snapshot.
    assert!(snapshot.exists());
    assert_eq!(session.call("decode_start", json!({})).unwrap()["count"], 1);
    assert_eq!(session.call("decode_event", json!({})).unwrap()["count"], 2);
    assert_eq!(
        session.call("decode_finish", json!({})).unwrap()["count"],
        3
    );
    session.finish().unwrap();
    assert!(!snapshot.exists());
}

#[test]
fn policy_refuses_missing_disabled_revoked_incompatible_and_changed_bindings() {
    let fixture = Fixture::new("", |_| {});
    assert!(harness_runtime::load(&fixture.repo(), "missing").is_err());
    let runtime = fixture.load();
    let original = fs::read_to_string(&fixture.binding).unwrap();
    for (from, to) in [
        ("enabled: true", "enabled: false"),
        ("[harness-runtime]", "[harness-launch]"),
        (env!("CARGO_PKG_VERSION"), "999.0.0"),
    ] {
        fs::write(&fixture.binding, original.replace(from, to)).unwrap();
        fixture.commit();
        assert!(harness_runtime::load(&fixture.repo(), "sixth").is_err());
        assert!(runtime.open_stream().is_err());
    }
    fs::write(&fixture.binding, &original).unwrap();
    fixture.commit();
    // Working-tree policy cannot grant or revoke authority.
    fs::write(
        &fixture.binding,
        original.replace("enabled: true", "enabled: false"),
    )
    .unwrap();
    assert!(harness_runtime::load(&fixture.repo(), "sixth").is_ok());
    fs::write(&fixture.binding, &original).unwrap();
    fixture.policy(
        "[harness-runtime]",
        "[harness-runtime, read-project-context]",
    );
    assert_ne!(runtime.binding_digest(), fixture.load().binding_digest());
    assert!(runtime.open_stream().is_err());
    assert!(!fixture.marker.exists());
}

#[test]
fn missing_and_tampered_installs_refuse_execution_even_after_load() {
    for tamper in [true, false] {
        let fixture = Fixture::new("", |_| {});
        let runtime = fixture.load();
        let installation = fixture.installation();
        if tamper {
            fs::write(installation.join("package/peer"), "tampered").unwrap();
        } else {
            fs::remove_dir_all(installation).unwrap();
        }
        assert!(harness_runtime::discover(&fixture.repo()).is_err());
        assert!(harness_runtime::load(&fixture.repo(), "sixth").is_err());
        assert!(runtime.open_stream().is_err());
        assert!(!fixture.marker.exists());
    }
}

#[test]
fn descriptor_rejects_missing_duplicate_reserved_and_unknown_fields() {
    for (field, value) in [
        ("id", json!("")),
        ("aliases", json!(["six", "six"])),
        ("aliases", json!(["setup"])),
        ("aliases", json!(["sixth"])),
        ("operations", json!(["shell"])),
        ("extra", json!(true)),
    ] {
        let fixture = Fixture::new("", |d| d["descriptor"][field] = value);
        assert!(harness_runtime::discover(&fixture.repo()).is_err());
        assert!(!fixture.marker.exists());
    }
    let fixture = Fixture::new("", |d| {
        d["descriptor"].as_object_mut().unwrap().remove("id");
    });
    assert!(harness_runtime::discover(&fixture.repo()).is_err());
    let descriptor = Fixture::new("", |_| {}).load().descriptor().clone();
    assert!(ahu::catalog::validate_runtime_descriptors([&descriptor, &descriptor]).is_err());
    let mut other = descriptor.clone();
    other.id = "seventh".into();
    assert!(ahu::catalog::validate_runtime_descriptors([&descriptor, &other]).is_err());
}

#[test]
fn malformed_frames_are_bounded_private_and_poison_sessions() {
    let fixture = Fixture::new("", |_| {});
    let runtime = fixture.load();
    for mode in [
        "malformed",
        "duplicate",
        "nested_duplicate",
        "mismatch",
        "version",
        "unknown",
        "both",
        "error",
        "private_error",
        "missing",
        "extra",
        "partial",
        "large",
        "block",
    ] {
        let start = Instant::now();
        let mut session = runtime.open_stream().unwrap();
        let error = session.call("probe", json!({"mode":mode})).unwrap_err();
        assert!(!error.to_string().contains("native-secret"));
        assert!(session.call("probe", json!({})).is_err());
        assert!(start.elapsed() < Duration::from_secs(5), "{mode}");
    }
    assert!(runtime.call("probe", json!({"mode":"stderr"})).is_ok());
    for mode in ["exit_failure", "late_extra"] {
        assert!(runtime.call("probe", json!({"mode":mode})).is_err());
    }
}

#[test]
fn blocked_stdin_cancellation_and_decoder_sequence_are_bounded() {
    let blocked = Fixture::new("import time; time.sleep(30)", |_| {});
    let start = Instant::now();
    assert!(
        blocked
            .load()
            .call("probe", json!({"bytes":"x".repeat(900_000)}))
            .is_err()
    );
    assert!(start.elapsed() < Duration::from_secs(5));
    let fixture = Fixture::new("", |_| {});
    let runtime = fixture.load();
    assert!(runtime.call("shell", Value::Null).is_err());
    assert!(!fixture.marker.exists());
    let mut session = runtime.open_stream().unwrap();
    assert!(
        session
            .call_cancellable("probe", json!({}), &AtomicBool::new(true))
            .is_err()
    );
    let mut session = runtime.open_stream().unwrap();
    assert!(session.call("decode_event", json!({})).is_err());
    let mut session = runtime.open_stream().unwrap();
    session.call("decode_start", json!({})).unwrap();
    assert!(session.finish().is_err());
    assert!(
        runtime
            .call("probe", json!({"bytes":"x".repeat(1_048_576)}))
            .is_err()
    );
}

fn assert_gone(pid: i32) {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        // SAFETY: signal 0 only observes the synthetic child; it sends no signal.
        if unsafe { libc::kill(pid, 0) } != 0 {
            return;
        }
        // Orphan descendants may briefly remain as zombies pending system reaping.
        let output = std::process::Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        if String::from_utf8_lossy(&output.stdout)
            .trim()
            .starts_with('Z')
        {
            return;
        }
        assert!(
            Instant::now() < until,
            "synthetic child {pid} still running"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn drop_failure_and_finalization_terminate_owned_process_groups() {
    let fixture = Fixture::new("", |_| {});
    let runtime = fixture.load();
    for ending in ["drop", "failure", "finish"] {
        let mut session = runtime.open_stream().unwrap();
        let pids = session.call("probe", json!({"mode":"child"})).unwrap();
        match ending {
            "failure" => {
                assert!(session.call("probe", json!({"mode":"block"})).is_err());
                drop(session);
            }
            "finish" => {
                // Descendant inherits stdout, so finalization times out and kills the group.
                assert!(session.finish().is_err());
            }
            _ => drop(session),
        }
        assert_gone(pids["parent"].as_i64().unwrap() as i32);
        assert_gone(pids["child"].as_i64().unwrap() as i32);
    }
}

#[test]
fn setup_lists_and_prepares_runtime_metadata_without_activation() {
    let fixture = Fixture::new("", |_| {});
    let package = fixture.installation().join("package");
    let output = common::ahu()
        .arg("--repo")
        .arg(&fixture.root)
        .args(["setup", "--list-harness-plugins"])
        .env("AHU_PLUGIN_PATH", &package)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("sixth"));
    assert!(stdout.contains("metadata only"));
    let target = tempfile::tempdir().unwrap();
    common::git(target.path(), &["init", "-q"]);
    let output = common::ahu()
        .arg("--repo")
        .arg(target.path())
        .args(["setup", "--harness-plugin"])
        .arg(&package)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("harness-runtime grant"));
    let binding = ahu::plugin_binding::load(target.path(), "independent-package")
        .unwrap()
        .unwrap()
        .0;
    assert!(!binding.enabled);
    assert!(binding.grants.is_empty());
    assert!(!fixture.marker.exists());
}

#[test]
fn exact_frame_limits_include_newline_and_cancellation_interrupts_dispatch() {
    let fixture = Fixture::new("", |d| d["timeout_ms"] = json!(3000));
    let runtime = fixture.load();
    let result = runtime.call("probe", json!({"mode":"boundary"})).unwrap();
    assert!(result.as_str().unwrap().len() > 1_000_000);
    assert!(
        runtime
            .call("probe", json!({"mode":"over_boundary"}))
            .is_err()
    );
    let envelope = json!({"protocol_version":1,"id":1,"operation":"probe","params":{"bytes":""}});
    let overhead = serde_json::to_vec(&envelope).unwrap().len() + 1;
    let payload = "x".repeat(1_048_576 - overhead);
    assert!(runtime.call("probe", json!({"bytes":payload})).is_ok());
    assert!(
        runtime
            .call("probe", json!({"bytes":format!("{payload}x")}))
            .is_err()
    );
    let ready = fixture.marker.with_file_name("dispatch-ready");
    let cancelled = AtomicBool::new(false);
    let mut session = runtime.open_stream().unwrap();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !ready.exists() {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
            cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        let error = session
            .call_cancellable(
                "probe",
                json!({"mode":"signal_block","ready":ready}),
                &cancelled,
            )
            .unwrap_err();
        assert!(error.to_string().contains("cancelled"));
    });
    assert!(session.call("probe", json!({})).is_err());
}

#[test]
fn installed_registry_drives_models_and_prerequisites_without_native_core_probes() {
    const CASE: &str =
        "installed_registry_drives_models_and_prerequisites_without_native_core_probes";
    if !common::is_child_case(CASE) {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().canonicalize().unwrap();
        let executable = directory.join("sixth-native");
        // Core must never invoke --version or models directly on this program.
        fs::write(&executable, "#!/bin/sh\nexit 91\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let output = common::run_child_case(CASE, |command| {
            command
                .env("HOME", &directory)
                .env("PATH", format!("{}:/usr/bin:/bin", directory.display()));
        });
        common::assert_child_passed(CASE, &output);
        return;
    }
    let fixture = Fixture::new("", |_| {});
    let repo = fixture.repo();
    let descriptors = harness_runtime::discover(&repo).unwrap();
    let suggestions = ahu::models::suggestions(&descriptors[0]);
    assert_eq!(suggestions.models, ["invented/exact-v6"]);
    assert!(!fixture.marker.exists());
    let options = ahu::models::for_harness(&repo, "sixth").unwrap();
    assert_eq!(options.models, ["sixth/new-exact-model"]);
    assert_eq!(options.source, ahu::models::Source::Native);
    let prerequisite = ahu::selection::check_prerequisite(&repo, "sixth").unwrap();
    assert!(prerequisite.satisfied());
    assert_eq!(prerequisite.version.as_deref(), Some("6.2.0"));
}

#[test]
fn project_rankings_select_exact_runtime_identity_without_machine_fallback() {
    let fixture = Fixture::new("", |_| {});
    let repo = fixture.repo();
    let config = ahu::config::ProjectConfig {
        schema_version: ahu::config::SUPPORTED_SCHEMA_VERSION,
        harness_preferences: vec!["sixth".into()],
        model_selection: "project-ranked".into(),
        harness_version_pins: Default::default(),
        model_rankings: [("sixth".into(), vec!["unlisted/exact-model".into()])].into(),
        knowledge: Default::default(),
        headless: Default::default(),
        telemetry: Default::default(),
    };
    let mut loaded = ahu::config::LoadedConfig {
        config,
        path: fixture.root.join(".agents/ahu/config.md"),
        digest: "a".repeat(64),
    };
    let pair = ahu::selection::resolve_automatic(&repo, &loaded).unwrap();
    assert_eq!(pair.harness, "sixth");
    assert_eq!(pair.model, "unlisted/exact-model");
    assert_eq!(pair.runtime_binding_digest, fixture.load().binding_digest());
    assert!(!fixture.marker.exists());
    loaded
        .config
        .harness_preferences
        .insert(0, "missing".into());
    loaded
        .config
        .model_rankings
        .insert("missing".into(), vec!["exact".into()]);
    assert!(ahu::selection::resolve_automatic(&repo, &loaded).is_err());
    loaded.config.harness_preferences = vec!["six".into()];
    loaded
        .config
        .model_rankings
        .insert("six".into(), vec!["exact".into()]);
    assert!(ahu::selection::resolve_automatic(&repo, &loaded).is_err());
    assert!(!fixture.marker.exists());
}

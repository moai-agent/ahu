//! Independent JSONL peer tests; no native harness, provider call, or worktree.
use super::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

struct Peer {
    _temp: tempfile::TempDir,
    root: PathBuf,
    pid: PathBuf,
    ready: PathBuf,
}
impl Peer {
    fn new(mode: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let root = base.join("repo");
        let package = base.join("package");
        let pid = base.join("peer-pid");
        let ready = base.join("ready");
        fs::create_dir_all(root.join(".agents/ahu/plugins")).unwrap();
        fs::create_dir(&package).unwrap();
        let git = |args: &[&str]| {
            let result = Command::new("git")
                .args(args)
                .current_dir(&root)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .env("GIT_AUTHOR_NAME", "Fixture")
                .env("GIT_COMMITTER_NAME", "Fixture")
                .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
                .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        };
        git(&["init", "-q"]);
        fs::write(
            package.join("Cargo.toml"),
            "[package]\nname='decoder-fixture'\nversion='1.0.0'\n",
        )
        .unwrap();
        let declaration = json!({"protocol_version":1,"path":"peer","timeout_ms":500,
            "descriptor":{"id":"invented","display_name":"Synthetic decoder","program":"invented-native",
                "aliases":[],"models":["fixture"],"modes":["headless"],
                "operations":["probe","models","launch","decode_start","decode_event","decode_finish"],
                "capabilities":[],"tested_versions":[],"context_paths":[],"skill_paths":[],"native_source_paths":[]}});
        fs::write(package.join("ahu-plugin.md"), format!("---\nokf_version: '0.2'\ntype: ahu:plugin\nschema_version: 1\nid: decoder-fixture\npackage: decoder-fixture\nhost_api: 1\ncargo_manifest: Cargo.toml\nharness_runtime: {declaration}\n---\nSynthetic peer.\n")).unwrap();
        let script = format!(
            r#"#!/usr/bin/python3
import json, os, sys, time
mode = {mode}
value = json.loads({snapshot})
with open({pid}, 'w') as out: out.write(str(os.getpid()))
for line in sys.stdin:
    request = json.loads(line)
    operation = request['operation']
    if operation == 'decode_event':
        with open({ready}, 'w') as out: out.write('ready')
        if mode == 'cancel': time.sleep(30)
        if mode == 'malformed':
            print('PRIVATE MALFORMED', flush=True); continue
        if mode == 'error':
            print(json.dumps({{'protocol_version':1,'id':request['id'],'error':'failed'}}), flush=True); continue
        if mode == 'schema': value['harness'] = 'wrong'
        value['summary'] = 'PRIVATE SUMMARY'
    if operation == 'decode_finish': value['finished'] = True
    print(json.dumps({{'protocol_version':1,'id':request['id'],'result':value}}), flush=True)
if mode == 'eof_timeout': time.sleep(30)
"#,
            mode = json!(mode),
            snapshot = json!(super::tests::fixture().to_string()),
            pid = json!(pid),
            ready = json!(ready)
        );
        fs::write(package.join("peer"), script).unwrap();
        fs::set_permissions(package.join("peer"), fs::Permissions::from_mode(0o700)).unwrap();
        let digest = crate::plugin_install::digest(&package).unwrap();
        fs::write(root.join(".agents/ahu/plugins/decoder-fixture.md"), format!("---\nokf_version: '0.2'\ntype: ahu:plugin-binding\nahu:\n  schema_version: 1\n  id: decoder-fixture\n  version: '1.0.0'\n  source:\n    kind: local\n    repository: {}\n    revision: '{digest}'\n    sha256: '{digest}'\n  compatibility:\n    ahu: '{}'\n    host_api: 1\n  enabled: true\n  grants: [harness-runtime]\n---\nSynthetic binding.\n",json!(package),env!("CARGO_PKG_VERSION"))).unwrap();
        git(&["add", ".agents"]);
        git(&[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "fixture",
        ]);
        crate::plugin_install::run(&root, "install", "decoder-fixture").unwrap();
        Self {
            _temp: temp,
            root,
            pid,
            ready,
        }
    }
    fn worker(&self) -> (Worker, Arc<Mutex<Events>>) {
        let repo = crate::git::discover(&self.root).unwrap();
        let runtime = crate::harness_runtime::load(&repo, "invented").unwrap();
        let events = Arc::new(Mutex::new(Events::default()));
        let worker = start(
            &runtime,
            SessionOwner {
                task_id: "synthetic".into(),
                attempt: 1,
                harness: "invented".into(),
            },
            self.root.join("checkpoint"),
            self.root.clone(),
            Arc::clone(&events),
        )
        .unwrap();
        (worker, events)
    }
    fn assert_reaped(&self) {
        let pid: i32 = fs::read_to_string(&self.pid).unwrap().parse().unwrap();
        // SAFETY: signal zero only inspects the disposable peer process.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }
}

#[test]
fn independent_peer_errors_and_eof_timeout_fail_incomplete_and_reap() {
    for mode in ["malformed", "error", "schema", "eof_timeout"] {
        let peer = Peer::new(mode);
        let (worker, events) = peer.worker();
        worker
            .input
            .send(Input::Line {
                stream: "stdout",
                line: "synthetic".into(),
            })
            .unwrap();
        worker.input.send(Input::End("stdout")).unwrap();
        worker.input.send(Input::End("stderr")).unwrap();
        let result = worker.outcome.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(result.is_err(), "{mode}");
        assert!(events.lock().unwrap().failed);
        assert!(!format!("{result:?}").contains("PRIVATE"));
        drop(worker);
        peer.assert_reaped();
    }
}

#[test]
fn cancelling_owned_decoder_joins_and_reaps_without_accepting_late_result() {
    let peer = Peer::new("cancel");
    let (worker, events) = peer.worker();
    worker
        .input
        .send(Input::Line {
            stream: "stderr",
            line: "synthetic".into(),
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !peer.ready.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    drop(worker);
    peer.assert_reaped();
    let events = events.lock().unwrap();
    assert!(events.failed);
    assert!(!events.terminal);
}

#[test]
fn mixed_streams_finalize_only_after_both_drains_and_keep_summary_ephemeral() {
    let peer = Peer::new("success");
    let (worker, events) = peer.worker();
    worker
        .input
        .send(Input::Line {
            stream: "stdout",
            line: "synthetic".into(),
        })
        .unwrap();
    worker.input.send(Input::End("stdout")).unwrap();
    assert!(
        worker
            .outcome
            .recv_timeout(Duration::from_millis(20))
            .is_err()
    );
    worker
        .input
        .send(Input::Line {
            stream: "stderr",
            line: "synthetic".into(),
        })
        .unwrap();
    worker.input.send(Input::End("stderr")).unwrap();
    worker
        .outcome
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    drop(worker);
    peer.assert_reaped();
    let events = events.lock().unwrap();
    assert_eq!(events.summary, "PRIVATE SUMMARY");
    assert!(!serde_json::to_string(&*events).unwrap().contains("PRIVATE"));
    assert!(!events.terminal); // EOF is not native completion evidence.
}

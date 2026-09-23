//! Synthetic batch protocol/lifecycle tests. No provider or terminal is contacted.
mod common;
use common::TestRepo;
use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};
use tempfile::TempDir;

struct Fixture {
    repo: TestRepo,
    external: TempDir,
    bin: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let repo = TestRepo::new();
        repo.init_config();
        repo.add_agent_on("worker", "1.0.0", "claude-code", "claude-opus-5");
        repo.commit("fixture configuration");
        let external = tempfile::tempdir().unwrap();
        let bin = external.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let script = r#"#!/usr/bin/env python3
import sys,os,json,time
if '--version' in sys.argv:
 print('2.1.270'); sys.exit(0)
a=sys.argv[1:]
assert '--print' in a and '--permission-prompts' in a and 'none' in a
assert a[a.index('--model')+1]=='claude-opus-5'
assert os.environ['AHU_EXECUTION_BACKEND']=='headless'
assert not any(k.startswith('CMUX_') for k in os.environ)
assert 'ahu delegation contract (v3, headless)' in a[-1]
assert sys.stdin.read()==''
scenario=os.environ.get('SCENARIO','success')
if scenario in ('child','mailbox') and '\ndelegate synthetic task</ahu-request-' not in a[-1]: scenario='success'
session=a[a.index('--resume')+1] if '--resume' in a else os.environ['AHU_PARENT_TASK']
print(json.dumps({'type':'system','subtype':'init','session_id':session}),flush=True)
if scenario=='sleep': time.sleep(60)
if scenario=='flood': sys.stderr.write('diagnostic\n'*100000); sys.stderr.flush()
if scenario=='oversized': print('x'*(1024*1024+1),flush=True); sys.exit(0)
if scenario=='malformed': print('{broken',flush=True); sys.exit(0)
if scenario=='missing': sys.exit(0)
if scenario in ('child','mailbox'):
 import subprocess
 if scenario=='mailbox':
  from pathlib import Path
  inbox=Path(os.environ['AHU_TASK_DIR'])/'requests'
  bad=inbox/'1111111111111111.request.json'; bad.write_text('{'); bad.chmod(0o600)
  bad=inbox/'2222222222222222.request.json'; bad.symlink_to('/dev/null')
  bad=inbox/'3333333333333333.request.json'; bad.write_text('{"executable":"forbidden"}'); bad.chmod(0o600)
  time.sleep(0.3)
 child=os.environ['CHILD_PROMPT']
 env=dict(os.environ,SCENARIO='success')
 extra=['--name',os.environ['CHILD_TASK_NAME']] if 'CHILD_TASK_NAME' in os.environ else []
 r=subprocess.run([os.environ['AHU_BIN'],'launch','@worker','--headless','--background','--output','json','--prompt-file',child]+extra,env=env,capture_output=True,text=True)
 assert r.returncode==0, r.stderr
 child_id=json.loads(r.stdout)['task_id']
 r=subprocess.run([os.environ['AHU_BIN'],'wait',child_id,'--output','json'],env=env,capture_output=True,text=True)
 assert r.returncode==0,r.stderr
 if scenario=='mailbox':
  valid=[p for p in inbox.glob('*.request.json') if p.name not in ['1111111111111111.request.json','2222222222222222.request.json','3333333333333333.request.json']][0]
  data=valid.read_bytes(); valid.unlink(); valid.write_bytes(data);valid.chmod(0o600)
  time.sleep(0.4)
  listing=subprocess.run([os.environ['AHU_BIN'],'tasks','--output','json'],env=env,capture_output=True,text=True)
  assert len(json.loads(listing.stdout)['tasks'])==2,listing.stdout
if scenario.startswith('native_'):
 assert a[a.index('--tools')+1]=='Read,Grep,Glob,Task'
 assert os.environ['CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH']=='1'
 assert os.environ['CLAUDE_CODE_SUBAGENT_MODEL']=='claude-opus-5'
 assert 'ENTIRE assignment' in a[-1]
 print(json.dumps({'type':'system','subtype':'task_started','task_id':'helper','subagent_type':'ahu-reader','spawn_depth':1,'is_backgrounded':False}),flush=True)
 if scenario!='native_unjoined':
  print(json.dumps({'type':'system','subtype':'task_notification','task_id':'helper','status':('failed' if scenario=='native_failed' else 'completed'),'summary':'synthetic read evidence'}),flush=True)
elif scenario=='outside_write':
 target=os.path.join(os.path.dirname(os.getcwd()),'escape.txt')
 print(json.dumps({'type':'tool_use','name':'Write','input':{'file_path':target}}),flush=True)
 open(target,'w').write('escaped\n')
elif scenario=='outside_event':
 target=os.environ['OUTSIDE_PATH']
 print(json.dumps({'type':'assistant','message':{'content':[{'type':'tool_use','name':'Edit','input':{'file_path':target}}]}}),flush=True)
elif scenario=='inside_write':
 print(json.dumps({'type':'tool_use','name':'Write','input':{'file_path':'inside.txt'}}),flush=True)
 print(json.dumps({'type':'tool_use','name':'Write','input':{'file_path':os.path.join(os.getcwd(),'kept.txt')}}),flush=True)
elif scenario=='outside_bad_input':
 print(json.dumps({'type':'tool_use','name':'Write','input':'not json{'}),flush=True)
else: open('proof.txt','w').write('synthetic proof\n')
print(json.dumps({'type':'result','subtype':'success','result':'validated synthetic proof','session_id':session,'is_error':False,'permission_denials':([{'tool':'Bash'}] if scenario=='denied' else [])}),flush=True)
if scenario=='nonzero': sys.exit(7)
"#;
        std::fs::write(bin.join("claude"), script).unwrap();
        std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        std::fs::write(bin.join("cmux"), "#!/bin/sh\nexit 97\n").unwrap();
        std::fs::set_permissions(bin.join("cmux"), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir(external.path().join("home")).unwrap();
        Self {
            repo,
            external,
            bin,
        }
    }
    fn command(&self) -> Command {
        let mut c = common::ahu();
        c.current_dir(self.repo.path())
            .env("AHU_RUNTIME_DIR", self.external.path().join("runtime"))
            .env(
                "AHU_TASK_INDEX_DIR",
                self.external.path().join("retired-index"),
            )
            .env(
                "HOME",
                self.external.path().join("home").canonicalize().unwrap(),
            )
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin:/opt/homebrew/bin", self.bin.display()),
            )
            .env("CMUX_WORKSPACE_ID", "sentinel")
            .env_remove("AHU_PARENT_TASK")
            .env_remove("AHU_EXECUTION_BACKEND")
            .env_remove("CODEX_HOME")
            .env_remove("CLAUDE_CONFIG_DIR");
        c
    }
    fn launch(&self, scenario: &str, extra: &[&str]) -> Output {
        self.command()
            .env("SCENARIO", scenario)
            .args([
                "launch",
                "@worker",
                "--headless",
                "--output",
                "json",
                "--prompt",
                "perform synthetic task",
            ])
            .args(extra)
            .output()
            .unwrap()
    }
    fn value(output: &Output) -> Value {
        serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
            panic!(
                "{e}; stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        })
    }
}

#[test]
fn foreground_keeps_primary_coordination_and_identity_without_native_copies() {
    let f = Fixture::new();
    let out = f.launch("success", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = Fixture::value(&out);
    assert_eq!(v["outcome"], "succeeded");
    assert_eq!(v["completion_verified"], false);
    assert_eq!(v["identity"]["agent"], "worker");
    let worktree = PathBuf::from(v["worktree"].as_str().unwrap());
    assert!(worktree.join("proof.txt").exists());
    assert!(!worktree.join(".ahu/state").exists());
    let artifacts = PathBuf::from(v["review"]["result_path"].as_str().unwrap());
    assert!(artifacts.exists());
    assert!(artifacts.starts_with(f.repo.path().canonicalize().unwrap()));
    for name in [
        "events.jsonl",
        "stderr.log",
        "final.txt",
        "supervisor.log",
        "native.log",
    ] {
        assert!(!artifacts.parent().unwrap().join(name).exists());
    }
    let id = v["task_id"].as_str().unwrap();
    let listed = f
        .command()
        .args(["tasks", "--output", "json"])
        .output()
        .unwrap();
    assert!(listed.status.success());
    assert_eq!(Fixture::value(&listed)["tasks"][0]["task_id"], id);
    let inspect = f
        .command()
        .args(["task", id, "--output", "json"])
        .output()
        .unwrap();
    assert!(
        inspect.status.success(),
        "{}",
        String::from_utf8_lossy(&inspect.stderr)
    );
}

#[test]
fn task_handles_round_trip_through_controls_siblings_and_resume_without_reassignment() {
    let f = Fixture::new();
    let output = f.launch("success", &["--name", "@storage-cleanup"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = Fixture::value(&output);
    let id = value["task_id"].as_str().unwrap();
    assert_eq!(value["task_handle"], "@storage-cleanup");
    assert_eq!(value["review"]["task_handle"], "@storage-cleanup");
    let linked = f.external.path().join("linked");
    common::git(
        f.repo.path(),
        &["worktree", "add", "--detach", linked.to_str().unwrap()],
    );
    for command in ["task", "result", "wait"] {
        let output = f
            .command()
            .current_dir(&linked)
            .args([command, "@STORAGE-CLEANUP", "--output", "json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let inspected = Fixture::value(&output);
        assert_eq!(inspected["task_id"], id);
        assert_eq!(inspected["task_handle"], "@storage-cleanup");
    }
    for args in [
        vec!["diff", "@storage-cleanup"],
        vec!["message", "@storage-cleanup", "hello"],
        vec!["cleanup", "@storage-cleanup"],
        vec!["cancel", "@storage-cleanup"],
    ] {
        let output = f.command().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let prompt = f.external.path().join("resume.txt");
    std::fs::write(&prompt, "continue synthetic task").unwrap();
    let output = f
        .command()
        .args([
            "resume",
            "@storage-cleanup",
            "--prompt-file",
            prompt.to_str().unwrap(),
            "--output",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let resumed = Fixture::value(&output);
    assert_eq!(resumed["task_id"], id);
    let output = f
        .command()
        .args(["wait", "@storage-cleanup", "--output", "json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(Fixture::value(&output)["task_handle"], "@storage-cleanup");
    let rejected = f.launch("success", &["--name", "storage-cleanup"]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("already reserved"));
    assert_eq!(
        ahu::task_handles::resolve(
            &ahu::git::discover(f.repo.path()).unwrap(),
            "@storage-cleanup"
        )
        .unwrap(),
        id
    );
}

#[test]
fn generated_handles_are_short_unique_and_dry_run_does_not_reserve_them() {
    let f = Fixture::new();
    let dry = f.launch("success", &["--title", "Parser cleanup", "--dry-run"]);
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    let preview = Fixture::value(&dry);
    assert_eq!(preview["task_handle_candidate"], "@parser-cleanup");
    assert_eq!(preview["task_handle_reserved"], false);
    let repo = ahu::git::discover(f.repo.path()).unwrap();
    assert!(
        !ahu::state::coordination_dir(&repo)
            .unwrap()
            .join("task-handles")
            .exists()
    );
    for expected in ["@parser-cleanup", "@parser-cleanup-2"] {
        let out = f.launch("success", &["--title", "Parser cleanup"]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(Fixture::value(&out)["task_handle"], expected);
    }
}

#[test]
fn zero_exit_does_not_override_malformed_missing_or_denied_results() {
    let f = Fixture::new();
    for scenario in ["malformed", "missing", "denied", "nonzero"] {
        let out = f.launch(scenario, &[]);
        assert!(!out.status.success(), "{scenario}");
        let v = Fixture::value(&out);
        assert_eq!(v["outcome"], "failed", "{scenario}");
    }
}
#[test]
fn concurrent_background_tasks_survive_submission_and_capture_stderr() {
    let f = Fixture::new();
    let first = f.launch("flood", &["--background"]);
    let second = f.launch("success", &["--background"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(second.status.success());
    let first = Fixture::value(&first);
    let second = Fixture::value(&second);
    assert_ne!(first["task_id"], second["task_id"]);
    for v in [&first, &second] {
        let out = f
            .command()
            .args(["wait", v["task_id"].as_str().unwrap(), "--output", "json"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(Fixture::value(&out)["outcome"], "succeeded");
    }
}
#[test]
fn timeout_and_cancel_are_distinct_terminal_outcomes() {
    let f = Fixture::new();
    let timeout = f.launch("sleep", &["--timeout", "1"]);
    assert_eq!(Fixture::value(&timeout)["outcome"], "timed_out");
    let launch = f.launch("sleep", &["--background"]);
    assert!(launch.status.success());
    let v = Fixture::value(&launch);
    let id = v["task_id"].as_str().unwrap();
    assert!(
        f.command()
            .args(["cancel", id, "--output", "json"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let out = f
        .command()
        .args(["wait", id, "--output", "json"])
        .output()
        .unwrap();
    assert_eq!(Fixture::value(&out)["outcome"], "cancelled");
}
#[test]
fn explicit_resume_keeps_session_and_creates_a_new_attempt() {
    let f = Fixture::new();
    let out = f.launch("success", &["--allow-child", "@worker"]);
    assert!(out.status.success());
    let v = Fixture::value(&out);
    let id = v["task_id"].as_str().unwrap();
    let prompt = f.external.path().join("followup.txt");
    std::fs::write(&prompt, "continue synthetic task").unwrap();
    let resumed = f
        .command()
        .args(["resume", id, "--prompt-file"])
        .arg(prompt)
        .args(["--output", "json"])
        .output()
        .unwrap();
    assert!(
        resumed.status.success(),
        "{}",
        String::from_utf8_lossy(&resumed.stderr)
    );
    let result = f
        .command()
        .args(["wait", id, "--output", "json"])
        .output()
        .unwrap();
    let next = Fixture::value(&result);
    assert_eq!(next["attempt"], 2);
    assert_eq!(next["harness"]["session"], v["harness"]["session"]);
    let events = PathBuf::from(next["review"]["result_path"].as_str().unwrap());
    let dir = events.parent().unwrap().parent().unwrap();
    let record = ahu::task::load(dir).unwrap();
    let spec: ahu::headless::Spec =
        serde_json::from_slice(&std::fs::read(dir.join("headless.json")).unwrap()).unwrap();
    let composition = record.delivery.composition.as_ref().unwrap();
    let state = composition.state.as_ref().unwrap();
    assert_eq!(state, &spec.coordination_state());
    assert_eq!(state.attempt, 2);
    assert_eq!(
        state.native_session.as_deref(),
        v["harness"]["session"].as_str()
    );
    assert_eq!(state.child_grants.len(), 1);
    assert_eq!(state.child_grants[0].agent, "worker");
    assert_eq!(composition.metadata.as_ref().unwrap().task_id, id);
    let delivered = ahu::orchestration::redeliver_headless_policy(
        &record.delivery,
        "continue synthetic task",
        &spec.options.native_helpers,
    )
    .unwrap();
    assert_eq!(
        ahu::orchestration::fence_body(&delivered, "request", &record.delivery.nonce),
        Some("continue synthetic task")
    );
}
#[test]
fn wrapper_is_refused_before_even_version_probe() {
    let f = Fixture::new();
    let marker = f.external.path().join("invoked");
    std::fs::write(
        f.bin.join("claude"),
        format!(
            "#!/bin/sh\n# cmux wrapper\ntouch '{}'\necho 2.1.270\n",
            marker.display()
        ),
    )
    .unwrap();
    let out = f.launch("success", &[]);
    assert!(!out.status.success());
    assert!(!marker.exists());
    assert_eq!(
        common::git(f.repo.path(), &["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        1
    );
}
#[test]
fn recursive_registered_child_inherits_headless_and_is_discoverable() {
    let f = Fixture::new();
    let prompt = f.external.path().join("child.txt");
    std::fs::write(&prompt, "perform synthetic task").unwrap();
    let out = f
        .command()
        .env("SCENARIO", "child")
        .env("CHILD_TASK_NAME", "@nested-worker")
        .env("CHILD_PROMPT", prompt)
        .args([
            "launch",
            "@worker",
            "--headless",
            "--output",
            "json",
            "--prompt",
            "delegate synthetic task",
            "--allow-child",
            "@worker",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let listed = f
        .command()
        .args(["tasks", "--output", "json"])
        .output()
        .unwrap();
    assert_eq!(
        Fixture::value(&listed)["tasks"].as_array().unwrap().len(),
        2
    );
    assert!(
        Fixture::value(&listed)["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|task| task["task_handle"] == "@nested-worker")
    );
}
#[test]
fn protocol_parser_rejects_uppercase_error_duplicate_and_session_drift() {
    let mut agy = ahu::headless::Events::default();
    agy.observe(
        "antigravity",
        br#"{"type":"result","status":"ERROR","response":"quota exhausted"}"#,
    );
    assert!(agy.failed && agy.terminal);
    let mut claude = ahu::headless::Events::default();
    claude.observe("claude-code", br#"{"type":"system","session_id":"first"}"#);
    claude.observe(
        "claude-code",
        br#"{"type":"result","session_id":"second","result":"ok"}"#,
    );
    assert!(claude.failed);
    let mut codex = ahu::headless::Events::default();
    codex.observe("codex", br#"{"type":"turn.completed"}"#);
    codex.observe("codex", br#"{"type":"turn.completed"}"#);
    assert!(codex.failed);
}

#[test]
fn cli_values_never_become_batch_options_and_duplicates_are_refused() {
    for field in ["--prompt", "--title", "--summary"] {
        let mut args = vec!["launch", "@worker", "--headless", field, "--dry-run"];
        if field != "--prompt" {
            args.extend(["--prompt", "task"]);
        }
        let ahu::cli::Command::HeadlessLaunch { launch, .. } = ahu::cli::parse(args).unwrap()
        else {
            panic!()
        };
        let ahu::cli::Command::Launch { dry_run, .. } = *launch else {
            panic!()
        };
        assert!(!dry_run);
    }
    for extra in [
        vec!["--headless", "--headless"],
        vec!["--headless", "--background", "--background"],
        vec!["--headless", "--timeout", "1", "--timeout", "1"],
    ] {
        let mut args = vec!["launch", "@worker", "--prompt", "task"];
        args.extend(extra);
        assert!(ahu::cli::parse(args).is_err());
    }
}
#[test]
fn incomplete_terminal_objects_are_protocol_failures() {
    for harness in ["claude-code", "antigravity"] {
        for event in [
            br#"{"type":"result"}"#.as_slice(),
            br#"{"type":"result","status":true,"result":17}"#,
        ] {
            let mut events = ahu::headless::Events::default();
            events.observe(harness, event);
            assert!(events.failed && events.terminal, "{harness}");
        }
    }
}
#[test]
fn missing_native_home_beneath_checkout_alias_is_refused() {
    let f = Fixture::new();
    std::fs::create_dir(f.repo.path().join("subdir")).unwrap();
    let alias = f.external.path().join("alias");
    std::os::unix::fs::symlink(f.repo.path().join("subdir"), &alias).unwrap();
    let out = f
        .command()
        .env("CODEX_HOME", alias.join("missing/home"))
        .args(["launch", "@worker", "--headless", "--prompt", "task"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("inside a checkout"));
    assert!(!f.repo.path().join("subdir/missing").exists());
}
#[test]
fn execution_rechecks_snapshot_and_user_hooks_before_spawning() {
    for user_hook in [false, true] {
        let f = Fixture::new();
        let out = f.launch("success", &[]);
        assert!(out.status.success());
        let v = Fixture::value(&out);
        let worktree = PathBuf::from(v["worktree"].as_str().unwrap());
        let events = PathBuf::from(v["review"]["result_path"].as_str().unwrap());
        let dir = events.parent().unwrap().parent().unwrap();
        let mut spec: Value =
            serde_json::from_slice(&std::fs::read(dir.join("headless.json")).unwrap()).unwrap();
        spec["attempt"] = 2.into();
        // Prepare a coherent next attempt before changing its configuration.
        // The delivery also freezes the attempt number now.
        let mut record = ahu::task::load(dir).unwrap();
        let prompt = ahu::task::load_prompt(dir).unwrap();
        let mut composition = record.delivery.composition.clone().unwrap();
        composition.state.as_mut().unwrap().attempt = 2;
        record.delivery = ahu::orchestration::deliver_composed(
            record.delivery.agent_instructions.as_deref(),
            &prompt,
            composition,
        )
        .unwrap()
        .1;
        ahu::task::save(dir, &record, &prompt).unwrap();
        std::fs::write(
            dir.join("headless.json"),
            serde_json::to_vec(&spec).unwrap(),
        )
        .unwrap();
        ahu::state::create_private_dir_all(&dir.join("attempt-2")).unwrap();
        std::fs::remove_file(worktree.join("proof.txt")).unwrap();
        if user_hook {
            let settings = f.external.path().join("home/.claude");
            std::fs::create_dir(&settings).unwrap();
            std::fs::write(settings.join("settings.json"),r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"echo synthetic"}]}]}}"#).unwrap();
        } else {
            std::fs::write(
                worktree.join(".agents/ahu/agents/worker.md"),
                "Changed instructions\n",
            )
            .unwrap();
        }
        let mut bytes = std::fs::read(dir.join("task.json")).unwrap();
        bytes.extend(std::fs::read(dir.join("headless.json")).unwrap());
        let out = f
            .command()
            .env("AHU_EXPECTED_DIGEST", ahu::util::digest_bytes(&bytes))
            .args(["supervise", "--task-dir"])
            .arg(dir)
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(!worktree.join("proof.txt").exists());
        assert!(String::from_utf8_lossy(&out.stderr).contains("changed since submission"));
    }
}
#[test]
fn failed_resume_prevalidation_preserves_previous_result_and_attempt() {
    let f = Fixture::new();
    let out = f.launch("success", &[]);
    assert!(out.status.success());
    let v = Fixture::value(&out);
    let id = v["task_id"].as_str().unwrap();
    let prompt = f.external.path().join("followup.txt");
    std::fs::write(&prompt, "followup").unwrap();
    std::fs::write(f.bin.join("claude"), "#!/bin/sh\necho changed\n").unwrap();
    let out = f
        .command()
        .args(["resume", id, "--prompt-file"])
        .arg(prompt)
        .output()
        .unwrap();
    assert!(!out.status.success());
    let result = f
        .command()
        .args(["result", id, "--output", "json"])
        .output()
        .unwrap();
    let value = Fixture::value(&result);
    assert_eq!(value["attempt"], 1);
    assert_eq!(value["outcome"], "succeeded");
}
#[test]
fn stalled_version_probe_is_bounded_before_worktree_creation() {
    let f = Fixture::new();
    std::fs::write(f.bin.join("claude"), "#!/bin/sh\nsleep 60\n").unwrap();
    let at = std::time::Instant::now();
    let out = f.launch("success", &[]);
    assert!(!out.status.success());
    assert!(at.elapsed() < std::time::Duration::from_secs(8));
    assert_eq!(
        common::git(f.repo.path(), &["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        1
    );
}

#[test]
fn oversized_event_stops_capture_and_preserves_evidence() {
    let f = Fixture::new();
    let out = f.launch("oversized", &[]);
    assert!(!out.status.success());
    let v = Fixture::value(&out);
    assert!(matches!(
        v["outcome"].as_str(),
        Some("failed" | "capture_failed")
    ));
    assert!(
        v["harness"]["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("stream evaluation"))
    );
}
#[test]
fn explicit_resume_recovers_a_pre_spawn_interrupted_metadata_transition() {
    let f = Fixture::new();
    let out = f.launch("success", &[]);
    assert!(out.status.success());
    let v = Fixture::value(&out);
    let id = v["task_id"].as_str().unwrap();
    let events = PathBuf::from(v["review"]["result_path"].as_str().unwrap());
    let dir = events.parent().unwrap().parent().unwrap();
    let record: Value =
        serde_json::from_slice(&std::fs::read(dir.join("task.json")).unwrap()).unwrap();
    let spec: Value =
        serde_json::from_slice(&std::fs::read(dir.join("headless.json")).unwrap()).unwrap();
    let prompt = std::fs::read_to_string(dir.join("prompt.txt")).unwrap();
    ahu::state::write_json(
        &dir.join("resume-journal.json"),
        &serde_json::json!({"record":record,"spec":spec,"prompt":prompt,"next_attempt":2}),
    )
    .unwrap();
    std::fs::write(dir.join("prompt.txt"), "partially transitioned prompt").unwrap();
    let followup = f.external.path().join("followup.txt");
    std::fs::write(&followup, "continue").unwrap();
    let out = f
        .command()
        .args(["resume", id, "--prompt-file"])
        .arg(followup)
        .args(["--output", "json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = f
        .command()
        .args(["wait", id, "--output", "json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(Fixture::value(&out)["attempt"], 2);
}
#[test]
fn cancelled_parent_refuses_later_child_admission() {
    let f = Fixture::new();
    let out = f.launch("success", &[]);
    assert!(out.status.success());
    let v = Fixture::value(&out);
    let id = v["task_id"].as_str().unwrap();
    let out = f
        .command()
        .args(["cancel", id, "--output", "json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    // The child launch is a registered child of the cancelled parent: the
    // headless profile routes it to the broker, which refuses admission
    // because the parent is already cancelled.
    let out = f
        .command()
        .current_dir(v["worktree"].as_str().unwrap())
        .env("AHU_PARENT_TASK", id)
        .env("AHU_EXECUTION_BACKEND", "headless")
        .args([
            "launch",
            "@worker",
            "--headless",
            "--prompt",
            "child",
            "--output",
            "json",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("cancelling"));
    let out = f
        .command()
        .args(["tasks", "--output", "json"])
        .output()
        .unwrap();
    assert_eq!(Fixture::value(&out)["tasks"].as_array().unwrap().len(), 1);
}

#[test]
fn batch_options_without_headless_refuse_ambient_execution_backend() {
    // AHU_EXECUTION_BACKEND records how this process was launched; it must
    // not silently select the headless profile for a new launch that omits
    // the explicit --headless flag.
    let f = Fixture::new();
    let out = f
        .command()
        .env("AHU_EXECUTION_BACKEND", "headless")
        .args([
            "launch",
            "@worker",
            "--background",
            "--prompt",
            "x",
            "--output",
            "json",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("batch options require --headless"));
}

#[test]
fn ambient_broker_dispatch_admits_batch_options() {
    // A broker-dispatched child inherits AHU_BROKER_DISPATCH, so batch
    // options parse without the explicit --headless flag: the refusal
    // below comes from identity resolution, not the batch-option gate.
    let f = Fixture::new();
    let out = f
        .command()
        .env("AHU_BROKER_DISPATCH", "1111111111111111")
        .args([
            "launch",
            "@missing",
            "--background",
            "--prompt",
            "x",
            "--output",
            "json",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("batch options require --headless"));
    assert!(stderr.contains("no agent named"));
}

#[test]
fn malformed_mailbox_requests_are_isolated_and_consumed_ids_do_not_replay() {
    let f = Fixture::new();
    let prompt = f.external.path().join("child.txt");
    std::fs::write(&prompt, "perform synthetic task").unwrap();
    let out = f
        .command()
        .env("SCENARIO", "mailbox")
        .env("CHILD_PROMPT", prompt)
        .args([
            "launch",
            "@worker",
            "--headless",
            "--prompt",
            "delegate synthetic task",
            "--allow-child",
            "@worker",
            "--output",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let value = Fixture::value(&out);
    assert_eq!(value["ahu_children"].as_array().unwrap().len(), 1);
    let events = PathBuf::from(value["review"]["result_path"].as_str().unwrap());
    let attempt = events.parent().unwrap();
    assert!(attempt.join("broker/1111111111111111.claim.json").exists());
    assert!(
        !attempt
            .parent()
            .unwrap()
            .join("requests/1111111111111111.claim.json")
            .exists()
    );
}

#[test]
fn host_grants_reject_implicit_approval_widening_before_execution() {
    let f = Fixture::new();
    f.repo
        .add_agent_on("writer", "1.0.0", "claude-code", "claude-opus-5");
    let path = f.repo.path().join(".agents/ahu/agents/writer.md");
    let source = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        source.replace("permissions: prompt", "permissions: auto"),
    )
    .unwrap();
    let out = f.launch("success", &["--allow-child", "@writer"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--allow-child-widened"));
    assert_eq!(
        common::git(f.repo.path(), &["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        1
    );
}

#[test]
fn a_dispatch_from_an_old_parent_attempt_is_refused_before_child_creation() {
    let f = Fixture::new();
    let launch = f.launch("sleep", &["--background"]);
    assert!(launch.status.success());
    let value = Fixture::value(&launch);
    let id = value["task_id"].as_str().unwrap();
    let before = common::git(f.repo.path(), &["worktree", "list", "--porcelain"]);
    let out = f
        .command()
        .current_dir(value["worktree"].as_str().unwrap())
        .env("AHU_PARENT_TASK", id)
        .env("AHU_PARENT_ATTEMPT", "2")
        .env("AHU_BROKER_DISPATCH", "1111111111111111")
        .args([
            "launch",
            "@worker",
            "--headless",
            "--prompt",
            "stale child",
            "--output",
            "json",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("stale parent attempt"));
    assert_eq!(
        common::git(f.repo.path(), &["worktree", "list", "--porcelain"]),
        before
    );
    assert!(
        f.command()
            .args(["cancel", id])
            .output()
            .unwrap()
            .status
            .success()
    );
    let _ = f
        .command()
        .args(["wait", id, "--output", "json"])
        .output()
        .unwrap();
}
#[test]
fn cleanup_removes_capture_but_preserves_result_and_worktree() {
    let f = Fixture::new();
    let launch = f.launch("success", &[]);
    assert!(launch.status.success());
    let value = Fixture::value(&launch);
    let id = value["task_id"].as_str().unwrap();
    let output = f
        .command()
        .args(["cleanup", id, "--output", "json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(PathBuf::from(value["review"]["result_path"].as_str().unwrap()).exists());
    assert!(
        PathBuf::from(value["worktree"].as_str().unwrap())
            .join("proof.txt")
            .exists()
    );
    let result = f
        .command()
        .args(["result", id, "--output", "json"])
        .output()
        .unwrap();
    assert_eq!(Fixture::value(&result)["outcome"], "succeeded");
}

#[test]
fn bounded_native_profile_is_applied_and_unjoined_helpers_block_the_result() {
    let f = Fixture::new();
    for (scenario, expected) in [
        ("native_joined", "succeeded"),
        ("native_unjoined", "failed"),
        ("native_failed", "failed"),
    ] {
        let output = f.launch(scenario, &["--native-helpers", "bounded"]);
        let value = Fixture::value(&output);
        assert_eq!(
            value["outcome"],
            expected,
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !PathBuf::from(value["worktree"].as_str().unwrap())
                .join("proof.txt")
                .exists()
        );
        assert_eq!(
            value["capabilities"]["native_profile"]["helper_model"],
            "claude-opus-5"
        );
    }
}

#[test]
fn ownership_inspection_never_confuses_invalid_locks_with_live_owners() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    let out = f.launch("success", &[]);
    let value = Fixture::value(&out);
    let id = value["task_id"].as_str().unwrap();
    let events = PathBuf::from(value["review"]["result_path"].as_str().unwrap());
    let attempt = events.parent().unwrap();
    let lock = attempt.parent().unwrap().join("owner.lock");
    std::fs::remove_file(attempt.join("result.json")).unwrap();
    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o400)).unwrap();
    let out = f
        .command()
        .args(["result", id, "--output", "json"])
        .output()
        .unwrap();
    assert_eq!(Fixture::value(&out)["outcome"], "interrupted");
    std::fs::remove_file(&lock).unwrap();
    for invalid in ["missing", "symlink", "directory"] {
        if invalid == "symlink" {
            symlink("/dev/null", &lock).unwrap();
        }
        if invalid == "directory" {
            std::fs::create_dir(&lock).unwrap();
        }
        for action in ["result", "wait"] {
            let at = std::time::Instant::now();
            let out = f
                .command()
                .args([action, id, "--output", "json"])
                .output()
                .unwrap();
            assert!(!out.status.success());
            assert!(String::from_utf8_lossy(&out.stderr).contains("liveness unknown"));
            assert!(at.elapsed() < std::time::Duration::from_secs(2));
        }
        if invalid == "symlink" {
            std::fs::remove_file(&lock).unwrap();
        }
        if invalid == "directory" {
            std::fs::remove_dir(&lock).unwrap();
        }
    }
}

#[test]
fn stderr_failures_override_zero_exit_success_and_unknown_diagnostics_are_exposed() {
    let f = Fixture::new();
    let original = std::fs::read_to_string(f.bin.join("claude")).unwrap();
    for (diagnostic, expected) in [
        ("Permission denied: tool execution", "failed"),
        ("RESOURCE_EXHAUSTED: quota reached", "failed"),
        ("authentication failed", "failed"),
        ("provider diagnostic with unknown semantics", "succeeded"),
    ] {
        let script = original.replace(
            "scenario=os.environ",
            &format!(
                "sys.stderr.write({});sys.stderr.flush()\nscenario=os.environ",
                serde_json::to_string(diagnostic).unwrap()
            ),
        );
        std::fs::write(f.bin.join("claude"), script).unwrap();
        let out = f.launch("success", &[]);
        let value = Fixture::value(&out);
        assert_eq!(value["outcome"], expected, "{value}");
        assert_eq!(value["process"]["exit_code"], 0);
        assert!(!value["harness"]["blockers"].as_array().unwrap().is_empty());
        if expected == "failed" {
            assert!(
                !value["harness"]["stderr_diagnostics"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
        } else {
            assert_eq!(value["harness"]["stderr_unclassified_lines"], 1);
        }
    }
}

#[test]
fn automatic_shutdown_cancels_live_children_and_prevents_late_writes() {
    for mode in ["timed_out", "capture_failed"] {
        let f = Fixture::new();
        let script = r#"#!/usr/bin/env python3
import sys,os,json,time,subprocess
if '--version' in sys.argv: print('2.1.270');sys.exit(0)
print(json.dumps({'type':'system','subtype':'init','session_id':os.environ['AHU_PARENT_TASK']}),flush=True)
if '\nparent-shutdown</ahu-request-' in sys.argv[-1]:
 time.sleep(1)
 r=subprocess.run([os.environ['AHU_BIN'],'launch','@worker','--headless','--background','--timeout','30','--prompt','slow-child','--output','json'],capture_output=True,text=True)
 assert r.returncode==0,r.stderr
 open(os.environ['CHILD_ID_FILE'],'w').write(json.loads(r.stdout)['task_id'])
 if os.environ['STOP_MODE']=='capture_failed': print('x'*(1024*1024+1),flush=True)
 time.sleep(30)
else:
 time.sleep(30)
 open(os.environ['LATE_FILE'],'w').write('unexpected child continuation')
"#;
        std::fs::write(f.bin.join("claude"), script).unwrap();
        let child_file = f.external.path().join("child-id");
        let late_file = f.external.path().join("late-write");
        let out = f
            .command()
            .env("CHILD_ID_FILE", &child_file)
            .env("LATE_FILE", &late_file)
            .env("STOP_MODE", mode)
            .args([
                "launch",
                "@worker",
                "--headless",
                "--timeout",
                "6",
                "--prompt",
                "parent-shutdown",
                "--allow-child",
                "@worker",
                "--output",
                "json",
            ])
            .output()
            .unwrap();
        let value = Fixture::value(&out);
        assert!(
            matches!(
                value["outcome"].as_str(),
                Some("timed_out" | "capture_failed")
            ),
            "parent must terminate through timeout or capture failure: {value}"
        );
        let child = std::fs::read_to_string(&child_file).unwrap();
        let out = f
            .command()
            .args(["result", &child, "--output", "json"])
            .output()
            .unwrap();
        let child_value = Fixture::value(&out);
        assert!(
            matches!(
                child_value["outcome"].as_str(),
                Some("cancelled" | "failed")
            ),
            "child must be terminal after parent shutdown: {child_value}"
        );
        assert!(!late_file.exists());
        assert_eq!(value["descendant_cancellation"][0]["outcome"], "cancelled");
    }
}

#[test]
fn child_and_worker_resume_refuse_before_mutating_attempts() {
    let f = Fixture::new();
    let prompt = f.external.path().join("child.txt");
    std::fs::write(&prompt, "perform synthetic task").unwrap();
    let out = f
        .command()
        .env("SCENARIO", "child")
        .env("CHILD_PROMPT", &prompt)
        .args([
            "launch",
            "@worker",
            "--headless",
            "--prompt",
            "delegate synthetic task",
            "--allow-child",
            "@worker",
            "--output",
            "json",
        ])
        .output()
        .unwrap();
    let value = Fixture::value(&out);
    assert_eq!(value["outcome"], "succeeded");
    let child = value["ahu_children"][0]["task_id"].as_str().unwrap();
    let parent = value["task_id"].as_str().unwrap();
    for id in [child, parent] {
        let before = f
            .command()
            .args(["result", id, "--output", "json"])
            .output()
            .unwrap();
        let mut command = f.command();
        if id == parent {
            command.env("AHU_BROKER_TOKEN", "synthetic-worker");
        }
        let out = command
            .args(["resume", id, "--prompt-file"])
            .arg(&prompt)
            .output()
            .unwrap();
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("previous attempt preserved"));
        let after = f
            .command()
            .args(["result", id, "--output", "json"])
            .output()
            .unwrap();
        assert_eq!(before.stdout, after.stdout);
    }
}

#[test]
fn mailbox_noise_and_slow_dispatch_do_not_starve_timeout() {
    for mode in ["noise", "slow"] {
        let f = Fixture::new();
        let script = r#"#!/usr/bin/env python3
import sys,os,json,time,subprocess
from pathlib import Path
if '--version' in sys.argv:
 if os.environ.get('SLOW_VERSION')=='1': time.sleep(30)
 print('2.1.270');sys.exit(0)
print(json.dumps({'type':'system','subtype':'init','session_id':os.environ['AHU_PARENT_TASK']}),flush=True)
if os.environ['MAILBOX_MODE']=='noise':
 inbox=Path(os.environ['AHU_TASK_DIR'])/'requests'
 for i in range(3000): (inbox/('invalid-'+str(i))).touch()
else:
 # The host dispatch inherits the supervisor environment; the parent version
 # probe must complete first, so delay only probes in a task worktree.
 subprocess.Popen([os.environ['AHU_BIN'],'launch','@worker','--background','--prompt','child','--output','json'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
time.sleep(30)
"#;
        let script = script.replace(
            "if os.environ.get('SLOW_VERSION')=='1':",
            "if 'worktrees' in os.getcwd():",
        );
        std::fs::write(f.bin.join("claude"), script).unwrap();
        let at = std::time::Instant::now();
        let out = f
            .command()
            .env("MAILBOX_MODE", mode)
            .args([
                "launch",
                "@worker",
                "--headless",
                "--timeout",
                "2",
                "--prompt",
                "parent",
                "--allow-child",
                "@worker",
                "--output",
                "json",
            ])
            .output()
            .unwrap();
        assert_eq!(
            Fixture::value(&out)["outcome"],
            "timed_out",
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            at.elapsed() < std::time::Duration::from_secs(10),
            "{mode}: {:?}",
            at.elapsed()
        );
    }
}

#[test]
fn agy_stderr_denial_is_not_hidden_by_a_success_terminal() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.repo
        .add_agent_on("agy-worker", "1.0.0", "antigravity", "gemini-3.1-pro-high");
    f.repo.commit("synthetic agy configuration");
    let script = r#"#!/usr/bin/env python3
import sys,json
if '--version' in sys.argv: print('1.2.2');sys.exit(0)
print(json.dumps({'type':'init','session_id':'synthetic-agy-session'}),flush=True)
sys.stderr.write('Permission denied: required tool execution\n');sys.stderr.flush()
print(json.dumps({'type':'result','status':'SUCCESS','response':'plausible success'}),flush=True)
"#;
    std::fs::write(f.bin.join("agy"), script).unwrap();
    std::fs::set_permissions(f.bin.join("agy"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = f
        .command()
        .args([
            "launch",
            "@agy-worker",
            "--headless",
            "--prompt",
            "synthetic task",
            "--output",
            "json",
        ])
        .output()
        .unwrap();
    let value = Fixture::value(&out);
    assert_eq!(value["outcome"], "failed");
    assert_eq!(value["process"]["exit_code"], 0);
    assert!(
        value["harness"]["stderr_diagnostics"][0]
            .as_str()
            .unwrap()
            .contains("permission denial")
    );
}

#[test]
fn terminal_only_native_work_fails_and_a_refused_helper_is_not_started_work() {
    let f = Fixture::new();
    let original = std::fs::read_to_string(f.bin.join("claude")).unwrap();
    for policy in ["disabled", "bounded"] {
        for (stats, expected) in [
            (
                serde_json::json!({"spawned":1,"completed":1,"failed":0,"max_depth":1}),
                "failed",
            ),
            (
                serde_json::json!({"spawned":0,"completed":0,"failed":0,"max_depth":0,"refused":{"depth_limit":1}}),
                "succeeded",
            ),
        ] {
            let script = original.replace(
                "'permission_denials':",
                &format!("'subagent_stats':{},'permission_denials':", stats),
            );
            std::fs::write(f.bin.join("claude"), script).unwrap();
            let out = f.launch("success", &["--native-helpers", policy]);
            let value = Fixture::value(&out);
            assert_eq!(value["outcome"], expected, "{value}");
            assert_eq!(
                value["native_completeness"]["complete"],
                expected == "succeeded"
            );
            if expected == "failed" {
                assert_eq!(value["native_completeness"]["evidence_complete"], false);
            } else {
                assert!(
                    !value["native_completeness"]["unknown"]
                        .as_array()
                        .unwrap()
                        .is_empty()
                );
            }
        }
    }
}

#[test]
fn mailbox_request_limit_bounds_private_retention_and_cleanup_removes_inbox() {
    let f = Fixture::new();
    let original = std::fs::read_to_string(f.bin.join("claude")).unwrap();
    let script = original.replace(
        "if scenario=='sleep': time.sleep(60)",
        r#"if scenario=='sleep':
 from pathlib import Path
 inbox=Path(os.environ['AHU_TASK_DIR'])/'requests'
 for i in range(270):
  p=inbox/(format(i,'016x')+'.request.json');p.write_text('{');p.chmod(0o600)
 time.sleep(60)"#,
    );
    std::fs::write(f.bin.join("claude"), script).unwrap();
    // This checks retention admission, not the attempt timeout. Hundreds of
    // durable claim writes need headroom under a concurrent full-suite load.
    let out = f.launch("sleep", &["--timeout", "30"]);
    let value = Fixture::value(&out);
    assert_eq!(value["outcome"], "capture_failed", "{value}");
    assert!(
        value["harness"]["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("broker dispatch failed"))
    );
    let events = PathBuf::from(value["review"]["result_path"].as_str().unwrap());
    let attempt = events.parent().unwrap();
    let claims = std::fs::read_dir(attempt.join("broker"))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".claim.json")
        })
        .count();
    assert_eq!(claims, 256);
    let out = f
        .command()
        .args([
            "cleanup",
            value["task_id"].as_str().unwrap(),
            "--output",
            "json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        std::fs::read_dir(attempt.parent().unwrap().join("requests"))
            .unwrap()
            .count(),
        0
    );
    assert!(attempt.join("broker/0000000000000000.claim.json").exists());
}

#[test]
fn background_shell_tasks_are_not_native_helpers_under_disabled_policy() {
    let f = Fixture::new();
    let original = std::fs::read_to_string(f.bin.join("claude")).unwrap();
    let script = original.replace("scenario=os.environ", r#"for task_id in ['shell-one','shell-two']:
 print(json.dumps({'type':'system','subtype':'task_started','task_type':'local_bash','task_id':task_id,'is_backgrounded':False}),flush=True)
 print(json.dumps({'type':'system','subtype':'task_notification','task_id':task_id,'status':'completed','summary':'synthetic shell completed'}),flush=True)
scenario=os.environ"#);
    std::fs::write(f.bin.join("claude"), script).unwrap();
    let out = f.launch("success", &[]);
    let value = Fixture::value(&out);
    assert_eq!(value["outcome"], "succeeded", "{value}");
    assert_eq!(value["native_completeness"]["complete"], true);
    assert!(
        value["native_completeness"]["joined"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        value["native_completeness"]["violations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

/// OpenCode's batch profile is `opencode run --format json`, and every part of
/// it was read off the running CLI rather than inferred from the interactive
/// adapter.
///
/// Verified on OpenCode 1.18.30 on 2026-09-14: `run` takes the message as a
/// positional argument, `--format json` emits one JSON event per line, `--`
/// ends the option list (`run --format json -m <model> -- --version` sent
/// `--version` to the model instead of printing the version), and `--session`
/// continues a recorded session. `-i/--interactive` is what would make `run` a
/// session, and this path never passes it.
#[test]
fn the_opencode_batch_profile_pins_the_model_and_ends_the_option_list() {
    use ahu::agent::Permissions;
    use ahu::harness::LaunchRequest;
    use ahu::headless::{Options, Spec};

    let spec = |session: Option<&str>| Spec {
        schema_version: 1,
        options: Options::default(),
        harness_version: "1.18.30".into(),
        executable_digest: "0".repeat(64),
        parent_task: None,
        parent_attempt: None,
        root_task: None,
        broker_request: None,
        child_grants: Vec::new(),
        depth: 0,
        attempt: 1,
        session: session.map(str::to_owned),
        broker_dir: None,
        native_profile: None,
        native_controls: Vec::new(),
        gaps: Vec::new(),
    };
    let request = |permissions| LaunchRequest {
        model: "ollama/glm-5.3:cloud",
        prompt: "do the thing",
        cwd: std::path::Path::new("/tmp"),
        permissions,
    };

    let prompting =
        ahu::headless::batch_command("opencode", &request(Permissions::Prompt), &spec(None))
            .expect("a prompting opencode batch launch");
    assert_eq!(prompting.program, "opencode");
    assert_eq!(
        prompting.args,
        [
            "run",
            "--format",
            "json",
            "--model",
            "ollama/glm-5.3:cloud",
            "--",
            "do the thing"
        ]
    );
    // The prompt is one argv element, and the stored command redacts exactly it.
    assert_eq!(prompting.prompt_arg, Some(prompting.args.len() - 1));
    assert!(
        !prompting.args.iter().any(|a| a == "--auto" || a == "-i"),
        "a prompting launch may neither widen permissions nor ask for a session: {:?}",
        prompting.args
    );

    let auto = ahu::headless::batch_command("opencode", &request(Permissions::Auto), &spec(None))
        .expect("an auto opencode batch launch");
    assert!(
        auto.args.contains(&"--auto".to_string()),
        "permissions = auto is the manifest's explicit request: {:?}",
        auto.args
    );

    // Resume continues the recorded session rather than forking it: `--fork`
    // would branch the conversation and leave the recorded id describing a
    // session the attempt is no longer in.
    let resumed = ahu::headless::batch_command(
        "opencode",
        &request(Permissions::Prompt),
        &spec(Some("ses_f5cc153b6ffeNq1dwHoZGMECKw")),
    )
    .expect("a resumed opencode batch launch");
    let session = resumed.args.iter().position(|a| a == "--session").unwrap();
    assert_eq!(resumed.args[session + 1], "ses_f5cc153b6ffeNq1dwHoZGMECKw");
    assert!(!resumed.args.iter().any(|a| a == "--fork"));

    // The batch path refuses accept-edits for the reason the interactive
    // adapter does, rather than quietly widening it to --auto.
    let refused =
        ahu::headless::batch_command("opencode", &request(Permissions::AcceptEdits), &spec(None))
            .expect_err("accept-edits has no OpenCode mapping")
            .to_string();
    assert!(refused.contains("accept-edits"), "{refused}");
    assert!(refused.contains("--auto"), "{refused}");
}

/// The OpenCode event stream decides the outcome; the exit status never does.
///
/// Every shape asserted here was captured from OpenCode 1.18.30 on 2026-09-14.
/// The load-bearing observation is the refused `write`: with no `--auto`, the
/// tool call came back as an ordinary tool error, the turn ended on a
/// `tool-calls` step with no `stop` after it, and the process still exited 0.
/// A run scored on its exit status would have recorded that as success.
#[test]
fn the_opencode_event_stream_is_terminal_only_when_a_step_stops() {
    use ahu::headless::Events;

    const SESSION: &str = "ses_f5cc153b6ffeNq1dwHoZGMECKw";
    let observe = |lines: &[&str]| {
        let mut events = Events::default();
        for line in lines {
            events.observe("opencode", line.as_bytes());
        }
        events
    };
    let step_start = format!(
        r#"{{"type":"step_start","timestamp":1,"sessionID":"{SESSION}","part":{{"type":"step-start"}}}}"#
    );
    let tool_step = format!(
        r#"{{"type":"step_finish","timestamp":2,"sessionID":"{SESSION}","part":{{"type":"step-finish","reason":"tool-calls"}}}}"#
    );
    let text = format!(
        r#"{{"type":"text","timestamp":3,"sessionID":"{SESSION}","part":{{"type":"text","text":"the final answer"}}}}"#
    );
    let stop = format!(
        r#"{{"type":"step_finish","timestamp":4,"sessionID":"{SESSION}","part":{{"type":"step-finish","reason":"stop"}}}}"#
    );

    let complete = observe(&[&step_start, &tool_step, &text, &stop]);
    assert!(complete.terminal, "a stop step ends the turn");
    assert!(!complete.failed);
    assert_eq!(complete.session.as_deref(), Some(SESSION));
    assert_eq!(complete.summary, "the final answer");
    assert_eq!(complete.unknown_events, 0, "the stream is fully recognized");

    // A tool-call boundary is where the model pauses to run tools; another step
    // follows it. Treating it as terminal would score an assignment complete at
    // its first tool call.
    let unfinished = observe(&[&step_start, &tool_step]);
    assert!(
        !unfinished.terminal,
        "only a stop step is terminal: {unfinished:?}"
    );

    // The refused `write`, in the shape OpenCode reported it: status "error",
    // the refusal in `state.error`, and no terminal step anywhere in the run.
    let refusal = format!(
        r#"{{"type":"tool_use","timestamp":5,"sessionID":"{SESSION}","part":{{"type":"tool","tool":"write","callID":"call_fh9hm5l9","state":{{"status":"error","error":"The user rejected permission to use this specific tool call."}}}}}}"#
    );
    let denied = observe(&[&step_start, &refusal, &tool_step]);
    assert!(denied.failed, "a refused tool call is a failure");
    assert!(!denied.terminal, "and the run never reached a stop step");
    assert!(
        denied
            .blockers
            .iter()
            .any(|b| b.contains("permission denials")),
        "{:?}",
        denied.blockers
    );

    // An ordinary tool error is not a permission refusal, and an agent that
    // recovers from one has not failed.
    let tool_error = format!(
        r#"{{"type":"tool_use","timestamp":6,"sessionID":"{SESSION}","part":{{"type":"tool","tool":"read","state":{{"status":"error","error":"ENOENT: no such file"}}}}}}"#
    );
    let recovered = observe(&[&step_start, &tool_error, &tool_step, &text, &stop]);
    assert!(!recovered.failed, "{:?}", recovered.blockers);
    assert!(recovered.terminal);

    // A step that ends for any other reason ended without the model saying it
    // was done, so it is terminal and failed rather than silently incomplete.
    let truncated = format!(
        r#"{{"type":"step_finish","timestamp":7,"sessionID":"{SESSION}","part":{{"type":"step-finish","reason":"length"}}}}"#
    );
    let capped = observe(&[&step_start, &truncated]);
    assert!(capped.terminal && capped.failed);
    assert!(
        capped
            .blockers
            .iter()
            .any(|b| b.contains("without a stop reason")),
        "{:?}",
        capped.blockers
    );

    // OpenCode's own error event, which carries the session id, so a failed
    // attempt is still resumable.
    let error = format!(
        r#"{{"type":"error","timestamp":8,"sessionID":"{SESSION}","error":{{"name":"UnknownError","data":{{"message":"Unexpected server error."}}}}}}"#
    );
    let failed = observe(&[&error]);
    assert!(failed.failed);
    assert_eq!(failed.session.as_deref(), Some(SESSION));

    // Two sessions inside one attempt is an identity change, not a resume.
    let elsewhere = stop.replace(SESSION, "ses_someone_else");
    let drifted = observe(&[&step_start, &elsewhere]);
    assert!(drifted.failed);
    assert!(
        drifted
            .blockers
            .iter()
            .any(|b| b.contains("session identity changed")),
        "{:?}",
        drifted.blockers
    );
}

/// An unvalidated OpenCode refuses before anything is launched.
///
/// The batch surface is pinned to the versions whose `run` options were read
/// off the CLI. OpenCode updates itself in place — the catalog entry names two
/// versions for exactly that reason — so the version gate is what stops a
/// renamed or re-meant option from changing behaviour silently.
#[test]
fn an_unvalidated_opencode_version_is_refused_by_the_headless_path() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.repo
        .add_agent_on("oc", "1.0.0", "opencode", "ollama/glm-5.3:cloud");
    f.repo.commit("an opencode agent");
    // Its own stub, rather than whichever OpenCode the machine has installed:
    // the refusal under test is about the version, so the version has to be the
    // test's to choose.
    let stub = f.bin.join("opencode");
    std::fs::write(
        &stub,
        "#!/bin/sh\n[ \"$1\" = \"--version\" ] && echo 1.18.5 && exit 0\nexit 9\n",
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let out = f
        .command()
        .args([
            "launch",
            "@oc",
            "--headless",
            "--prompt",
            "perform synthetic task",
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "an unvalidated version must not launch"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unvalidated headless opencode version \"1.18.5\""),
        "the refusal must name the harness and the version it found: {stderr}"
    );
    assert!(
        stderr.contains("no fallback was selected"),
        "the refusal must say nothing was substituted: {stderr}"
    );
    assert!(
        !stderr.contains("claude") && !stderr.contains("antigravity"),
        "no other harness may be offered in its place: {stderr}"
    );
}

/// A registered OpenCode agent runs the whole headless path end to end.
///
/// The stub replays the event stream captured from OpenCode 1.18.30, including
/// the intermediate `tool-calls` step, and asserts the argv ahu built. Nothing
/// here contacts a provider; the point is that ahu drives `run --format json`,
/// reads the terminal step, and records the session it was told.
#[test]
fn a_registered_opencode_agent_runs_headless_and_records_its_session() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.repo
        .add_agent_on("oc", "1.0.0", "opencode", "ollama/glm-5.3:cloud");
    f.repo.commit("an opencode agent");
    let stub = f.bin.join("opencode");
    std::fs::write(
        &stub,
        r#"#!/usr/bin/env python3
import sys,os,json
if '--version' in sys.argv:
 print('1.18.30'); sys.exit(0)
a=sys.argv[1:]
assert a[0]=='run', a
assert a[a.index('--format')+1]=='json', a
assert a[a.index('--model')+1]=='ollama/glm-5.3:cloud', a
assert a[-2]=='--', a
assert '--auto' not in a, a
assert '-i' not in a and '--interactive' not in a, a
assert os.environ['AHU_EXECUTION_BACKEND']=='headless'
assert 'ahu delegation contract (v3, headless)' in a[-1]
session='ses_'+os.environ['AHU_PARENT_TASK']
def emit(kind,part): print(json.dumps({'type':kind,'timestamp':1,'sessionID':session,'part':part}),flush=True)
emit('step_start',{'type':'step-start'})
emit('tool_use',{'type':'tool','tool':'write','callID':'call_1','state':{'status':'completed'}})
emit('step_finish',{'type':'step-finish','reason':'tool-calls'})
open('proof.txt','w').write('synthetic proof\n')
emit('text',{'type':'text','text':'validated synthetic proof'})
emit('step_finish',{'type':'step-finish','reason':'stop'})
"#,
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let out = f
        .command()
        .args([
            "launch",
            "@oc",
            "--headless",
            "--output",
            "json",
            "--prompt",
            "perform synthetic task",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = Fixture::value(&out);
    assert_eq!(v["outcome"], "succeeded");
    // Still not an acceptance: a terminal event is the harness saying it
    // stopped, not the assignment being judged done.
    assert_eq!(v["completion_verified"], false);
    assert_eq!(v["identity"]["harness"], "opencode");
    let worktree = PathBuf::from(v["worktree"].as_str().unwrap());
    assert!(worktree.join("proof.txt").exists());
    assert!(v["harness"].get("summary").is_none());
    assert!(
        v["harness"]["session"]
            .as_str()
            .is_some_and(|s| s.starts_with("ses_")),
        "the recorded session is the one OpenCode reported: {}",
        v["harness"]
    );
}

/// A probe output with decoration after the version must be admitted.
///
/// `--version` output is not a stable contract: tools append build tags, commit
/// hashes, and channel suffixes after whitespace. The compatibility table
/// records plain versions, so the gate must compare the first token that
/// parses as a version rather than the whole line — otherwise every decorated
/// build of a validated version is refused as unvalidated.
#[test]
fn a_decorated_version_token_is_admitted_by_the_headless_path() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.repo.add_agent("stubbed", "1.0.0", "claude-sonnet-5");
    f.repo.commit("agent");
    // Replaces the fixture's `claude`: same launch flow, but `--version`
    // decorates the validated token instead of printing it bare.
    let stub = f.bin.join("claude");
    std::fs::write(
        &stub,
        r#"#!/usr/bin/env python3
import sys,json
if '--version' in sys.argv:
 print('2.1.270 (Claude Code, linux-x64)'); sys.exit(0)
assert '--print' in sys.argv, sys.argv
print(json.dumps({'type':'system','subtype':'init','session_id':'stub-session'}),flush=True)
print(json.dumps({'type':'result','subtype':'success','result':'validated synthetic proof','session_id':'stub-session','is_error':False,'permission_denials':[]}),flush=True)
"#,
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let out = f
        .command()
        .args(["launch", "@stubbed", "--headless", "--prompt", "ok"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "a validated version with trailing decoration must launch: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A probe output whose first token is not validated must be refused, however
/// validated a later token looks.
///
/// The inverse of the decorated-token test: `any`-token matching admitted
/// `1.0.0 (Claude Code 2.1.270)` because a supported version appeared inside
/// the parenthetical. Compatibility is a property of the first token — the
/// actual CLI the user has installed — not of any string the probe emits.
#[test]
fn a_parenthetical_version_token_is_refused_by_the_headless_path() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.repo.add_agent("stubbed", "1.0.0", "claude-sonnet-5");
    f.repo.commit("agent");
    let stub = f.bin.join("claude");
    std::fs::write(
        &stub,
        "#!/bin/sh\n[ \"$1\" = \"--version\" ] && echo '1.0.0 (Claude Code, profile 2.1.270)' && exit 0\nexit 9\n",
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let out = f
        .command()
        .args(["launch", "@stubbed", "--headless", "--prompt", "ok"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "an unvalidated first token must not launch"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unvalidated headless claude-code version"),
        "the refusal must name the harness and version: {stderr}"
    );
}

/// A probe output with a CLI-name prefix before the version must be admitted.
///
/// Codex prints `codex-cli 0.154.0` from `--version`: the first whitespace
/// token names the CLI and the second is the version. First-token matching
/// refused every installed codex as unvalidated; compatibility is a property
/// of the version the probe reports, not of the name in front of it.
#[test]
fn a_cli_name_prefixed_version_token_is_admitted_by_the_headless_path() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.repo.add_agent_on("cx", "1.0.0", "codex", "gpt-6-astra");
    f.repo.commit("a codex agent");
    let stub = f.bin.join("codex");
    std::fs::write(
        &stub,
        r#"#!/usr/bin/env python3
import sys,os,json
if '--version' in sys.argv:
 print('codex-cli 0.154.0'); sys.exit(0)
a=sys.argv[1:]
assert a[0]=='exec', a
assert a[1]!='resume', a
assert a[a.index('--model')+1]=='gpt-6-astra', a
assert '--json' in a, a
i=0
cfg={}
while i+1 < len(a):
 if a[i]=='-c': cfg[a[i+1]]=cfg.get(a[i+1],0)+1; i+=2; continue
 i+=1
assert cfg.get('agents.enabled=false')==1, cfg
assert cfg.get('approval_policy="never"')==1, cfg
assert cfg.get('sandbox_mode="read-only"')==1, cfg
assert '--approve-for-me' not in a, a
assert a[a.index('--color')+1]=='never', a
assert a[-2]=='--', a
assert 'ahu delegation contract (v3, headless)' in a[-1], a[-1][:200]
assert os.environ['AHU_EXECUTION_BACKEND']=='headless'
assert sys.stdin.read()==''
session='thr_'+os.environ['AHU_PARENT_TASK']
print(json.dumps({'type':'thread.started','thread_id':session}),flush=True)
open('proof.txt','w').write('synthetic proof\n')
print(json.dumps({'type':'item.completed','item':{'type':'agent_message','text':'validated synthetic proof'}}),flush=True)
print(json.dumps({'type':'turn.completed'}),flush=True)
"#,
    )
    .unwrap();
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

    let out = f
        .command()
        .args([
            "launch",
            "@cx",
            "--headless",
            "--output",
            "json",
            "--prompt",
            "perform synthetic task",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "a validated version behind a CLI-name prefix must launch: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let v = Fixture::value(&out);
    assert_eq!(v["outcome"], "succeeded");
    assert_eq!(v["identity"]["harness"], "codex");
    let worktree = PathBuf::from(v["worktree"].as_str().unwrap());
    assert!(worktree.join("proof.txt").exists());
    assert!(v["harness"].get("summary").is_none());
    assert!(
        v["harness"]["session"]
            .as_str()
            .is_some_and(|s| s.starts_with("thr_")),
        "the recorded session is the one Codex reported: {}",
        v["harness"]
    );
}

/// Cleanup unlinks a mailbox symlink where it stands; it never follows one.
///
/// The inbox is writable by the worker, so what cleanup finds there is not
/// necessarily what ahu wrote. Removal goes through a handle on the validated
/// directory, and unlinks the entry rather than its target, so a link planted
/// at an inbox name costs the link and nothing else.
#[test]
fn cleanup_unlinks_a_mailbox_symlink_without_following_it() {
    let f = Fixture::new();
    let value = Fixture::value(&f.launch("success", &[]));
    let id = value["task_id"].as_str().unwrap();
    let task = PathBuf::from(value["review"]["result_path"].as_str().unwrap())
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    let decoy = f.external.path().join("decoy-outside-the-runtime-root");
    std::fs::write(&decoy, "keep me").unwrap();
    let planted = task.join("requests").join("0000000000000001.json");
    std::os::unix::fs::symlink(&decoy, &planted).unwrap();

    let out = f
        .command()
        .args(["cleanup", id, "--output", "json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!planted.exists() && planted.symlink_metadata().is_err());
    assert_eq!(std::fs::read_to_string(&decoy).unwrap(), "keep me");
}

/// An artifact name occupied by something ahu did not write stops cleanup.
///
/// The five captured logs are regular files ahu created. A symlink standing at
/// one of those names is not an artifact whose removal is cleanup's business,
/// so the kind is read through the pinned directory and the run refuses rather
/// than deleting anything at that name.
#[test]
fn cleanup_refuses_an_artifact_name_that_is_not_a_regular_file() {
    let f = Fixture::new();
    let value = Fixture::value(&f.launch("success", &[]));
    let id = value["task_id"].as_str().unwrap();
    let attempt = PathBuf::from(value["review"]["result_path"].as_str().unwrap())
        .parent()
        .unwrap()
        .to_path_buf();

    let decoy = f.external.path().join("decoy-behind-an-artifact-name");
    std::fs::write(&decoy, "keep me").unwrap();
    let planted = attempt.join("final.txt");
    let _ = std::fs::remove_file(&planted);
    std::os::unix::fs::symlink(&decoy, &planted).unwrap();

    let out = f
        .command()
        .args(["cleanup", id, "--output", "json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("refusing runtime artifact") && stderr.contains("final.txt"),
        "the refusal must name the artifact: {stderr}"
    );
    assert_eq!(std::fs::read_to_string(&decoy).unwrap(), "keep me");
    assert!(planted.symlink_metadata().unwrap().file_type().is_symlink());
}

/// A write tool call that escapes the task worktree is disclosed in the
/// result envelope, next to the evidence that the worktree itself stayed
/// clean. The classification is a disclosure, not a gate: the attempt still
/// succeeds and the escaped file is left exactly where the harness put it.
#[test]
fn writes_outside_worktree_are_disclosed_in_the_result_envelope() {
    let f = Fixture::new();
    let out = f.launch("outside_write", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = Fixture::value(&out);
    assert_eq!(v["outcome"], "succeeded");
    let worktree = PathBuf::from(v["worktree"].as_str().unwrap());
    let escaped = worktree
        .parent()
        .unwrap()
        .join("escape.txt")
        .canonicalize()
        .unwrap();
    assert_eq!(std::fs::read_to_string(&escaped).unwrap(), "escaped\n");
    let recorded = v["writes_outside_worktree"].as_array().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(PathBuf::from(recorded[0].as_str().unwrap()), escaped);
}

/// A write tool call nested inside an assistant message is found the same as
/// a top-level one, and a path that never became a file is still recorded via
/// nearest-existing-ancestor resolution. The scenario never creates the file,
/// proving the disclosure comes from the event stream alone.
#[test]
fn a_planned_write_outside_the_worktree_is_disclosed_without_the_file() {
    let f = Fixture::new();
    // The planned file never exists, so resolve the real path through the
    // existing parent the same way the supervisor does.
    let planned = f
        .external
        .path()
        .canonicalize()
        .unwrap()
        .join("planned-escape.txt");
    let out = f
        .command()
        .env("SCENARIO", "outside_event")
        .env("OUTSIDE_PATH", &planned)
        .args([
            "launch",
            "@worker",
            "--headless",
            "--output",
            "json",
            "--prompt",
            "perform synthetic task",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = Fixture::value(&out);
    assert_eq!(v["outcome"], "succeeded");
    assert!(!planned.exists());
    let recorded = v["writes_outside_worktree"].as_array().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(PathBuf::from(recorded[0].as_str().unwrap()), planned);
}

/// Writes that stay inside the worktree — relative, or absolute under it —
/// are not disclosed as escapes.
#[test]
fn writes_inside_the_worktree_are_not_disclosed() {
    let f = Fixture::new();
    let out = f.launch("inside_write", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = Fixture::value(&out);
    assert_eq!(v["outcome"], "succeeded");
    assert_eq!(v["writes_outside_worktree"].as_array().unwrap().len(), 0);
}

/// A write tool call whose `input` is not a JSON object yields no path
/// candidates; the malformed input is skipped rather than fatal.
#[test]
fn unparseable_write_input_yields_no_disclosed_paths() {
    let f = Fixture::new();
    let out = f.launch("outside_bad_input", &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v = Fixture::value(&out);
    assert_eq!(v["outcome"], "succeeded");
    assert_eq!(v["writes_outside_worktree"].as_array().unwrap().len(), 0);
}

#[test]
fn composed_batch_launch_and_continuation_freeze_identity_session_and_grants() {
    use ahu::agent::Permissions;
    use ahu::headless::{Options, Spec};
    use ahu::orchestration::*;
    for (harness, model, version, policy) in [
        ("codex", "gpt-6-astra", "0.154.0", "disabled"),
        ("claude-code", "claude-opus-5", "2.1.270", "disabled"),
        ("claude-code", "claude-opus-5", "2.1.270", "bounded"),
        ("antigravity", "gemini-3.1-pro-high", "0.4.10", "disabled"),
        ("opencode", "ollama/glm-5.3:cloud", "1.18.30", "disabled"),
    ] {
        let mut spec = Spec {
            schema_version: 1,
            options: Options {
                native_helpers: policy.into(),
                ..Options::default()
            },
            harness_version: version.into(),
            executable_digest: "0".repeat(64),
            parent_task: None,
            parent_attempt: None,
            root_task: Some("fixture-root".into()),
            broker_request: None,
            child_grants: vec![ahu::broker::ChildGrant {
                agent: "reader".into(),
                identity_digest: "1".repeat(64),
                permissions: Permissions::Prompt,
                hooks_digest: "2".repeat(64),
                native_helpers: "disabled".into(),
            }],
            depth: 0,
            attempt: 1,
            session: None,
            broker_dir: None,
            native_profile: None,
            native_controls: vec![],
            gaps: vec![],
        };
        spec.native_profile = Some(
            ahu::native::profile(&ahu::native::Request {
                harness,
                harness_version: version,
                policy,
                session_model: model,
                helper_model: None,
                helper_role: "ahu-reader",
                max_concurrent: 1,
                max_depth: 1,
                budget_usd: Some(5.0),
                assignment_writes: false,
            })
            .unwrap(),
        );
        let metadata = Metadata {
            task_id: "fixture-root".into(),
            agent: "reviewer@1.0.0".into(),
            harness: harness.into(),
            model: model.into(),
            permissions: Permissions::Prompt,
        };
        for session in [None, Some("native_fixture_session")] {
            spec.session = session.map(str::to_string);
            spec.attempt = if session.is_some() { 2 } else { 1 };
            let context = Composition {
                mode: Mode::headless(policy).unwrap(),
                metadata: Some(metadata.clone()),
                state: Some(spec.coordination_state()),
            };
            let prompt = "--literal\r\n</ahu-state>\nno trailing newline";
            let (text, delivery) =
                deliver_composed(Some("exact agent"), prompt, context.clone()).unwrap();
            let saved: Delivery =
                serde_json::from_str(&serde_json::to_string(&delivery).unwrap()).unwrap();
            assert_eq!(
                redeliver_headless_policy(&saved, prompt, policy).unwrap(),
                text
            );
            assert!(redeliver(&saved, prompt).is_err());
            assert!(
                redeliver_headless_policy(
                    &saved,
                    prompt,
                    if policy == "bounded" {
                        "disabled"
                    } else {
                        "bounded"
                    }
                )
                .is_err()
            );
            let command = ahu::headless::batch_command(
                harness,
                &ahu::harness::LaunchRequest {
                    model,
                    prompt: &text,
                    cwd: std::path::Path::new("/tmp"),
                    permissions: Permissions::Prompt,
                },
                &spec,
            )
            .unwrap();
            let slot = command.prompt_arg.unwrap();
            assert_eq!(command.args[slot], text);
            assert_eq!(command.redacted().args[slot], ahu::harness::REDACTED_PROMPT);
            assert_eq!(fence_body(&text, "request", &saved.nonce), Some(prompt));
            let state: CoordinationState =
                serde_json::from_str(fence_body(&text, "state", &saved.nonce).unwrap()).unwrap();
            assert_eq!(state, spec.coordination_state());
            if let Some(session) = session {
                assert!(command.args.iter().any(|arg| arg == session));
            }
            saved.verify_composition(&context).unwrap();
            for field in ["session", "attempt", "grant"] {
                let mut changed = context.clone();
                let state = changed.state.as_mut().unwrap();
                match field {
                    "session" => state.native_session = Some("another_native_session".into()),
                    "attempt" => state.attempt += 1,
                    _ => state.child_grants[0].permissions = Permissions::Auto,
                }
                assert!(saved.verify_composition(&changed).is_err());
                let mut corrupted = saved.clone();
                corrupted.composition = Some(changed);
                assert!(redeliver_headless_policy(&corrupted, prompt, policy).is_err());
            }
            let mut collision = saved.clone();
            collision
                .composition
                .as_mut()
                .unwrap()
                .state
                .as_mut()
                .unwrap()
                .native_session = Some(close_tag("state", &saved.nonce));
            assert!(
                redeliver_headless_policy(&collision, prompt, policy)
                    .unwrap_err()
                    .to_string()
                    .contains("fence nonce")
            );
        }
    }
}

#[test]
fn native_response_sentinels_never_persist_in_primary_coordination() {
    let f = Fixture::new();
    let sentinel = "NATIVE_RESPONSE_MUST_STAY_NATIVE_6b2e9c";
    let executable = f.bin.join("claude");
    let script = std::fs::read_to_string(&executable).unwrap()
        .replace("synthetic read evidence", sentinel)
        .replace("validated synthetic proof", sentinel)
        .replace("scenario=os.environ", &format!("sys.stderr.write('permission denied: {sentinel}\\n');sys.stderr.flush()\nscenario=os.environ"));
    std::fs::write(executable, script).unwrap();
    let output = f.launch("native_joined", &["--native-helpers", "bounded"]);
    let value = Fixture::value(&output);
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["outcome"], "failed");
    assert_eq!(value["native_helpers"][0]["status"], "completed");
    assert!(value["harness"].get("summary").is_none());
    assert!(value["harness"].get("native_observations").is_none());
    assert!(value.get("agent_report").is_none());
    assert_eq!(value["native_reference"]["source"], "harness event stream");
    assert!(value["native_reference"]["data_location"].is_null());
    fn inspect(path: &std::path::Path, sentinel: &str) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                inspect(&path, sentinel);
            } else {
                let name = entry.file_name();
                let name = name.to_str().unwrap();
                assert!(
                    ![
                        "events.jsonl",
                        "stderr.log",
                        "final.txt",
                        "native.log",
                        "supervisor.log",
                        "result.md"
                    ]
                    .contains(&name),
                    "{}",
                    path.display()
                );
                assert!(!name.ends_with(".dispatch.log"));
                let bytes = std::fs::read(&path).unwrap();
                assert!(
                    !bytes
                        .windows(sentinel.len())
                        .any(|b| b == sentinel.as_bytes()),
                    "native text persisted at {}",
                    path.display()
                );
            }
        }
    }
    inspect(&f.repo.path().join(".ahu/state"), sentinel);
    assert!(!f.external.path().join("runtime").exists());
    assert!(!f.external.path().join("retired-index").exists());
}

#[test]
fn session_checkpoint_survives_stream_evaluation_failure() {
    let f = Fixture::new();
    let output = f.launch("oversized", &[]);
    let value = Fixture::value(&output);
    let result_path = PathBuf::from(value["review"]["result_path"].as_str().unwrap());
    let checkpoint: Value = serde_json::from_slice(
        &std::fs::read(result_path.parent().unwrap().join("native-session.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(checkpoint["schema_version"], 2);
    assert_eq!(checkpoint["task_id"], value["task_id"]);
    assert_eq!(checkpoint["attempt"], 1);
    assert_eq!(checkpoint["harness"], "claude-code");
    assert_eq!(checkpoint["session"], value["task_id"]);
    assert_eq!(value["harness"]["session"], value["task_id"]);
    assert!(checkpoint["native_data_location"].is_null());
    assert_ne!(value["outcome"], "succeeded");
}

#[test]
fn linked_checkout_reads_primary_headless_records_and_independent_repo_does_not() {
    let f = Fixture::new();
    let output = f.launch("success", &[]);
    let value = Fixture::value(&output);
    let id = value["task_id"].as_str().unwrap();
    let sibling = f.external.path().join("sibling");
    common::git(
        f.repo.path(),
        &["worktree", "add", "--detach", sibling.to_str().unwrap()],
    );
    for command in ["task", "result", "wait"] {
        let out = f
            .command()
            .current_dir(&sibling)
            .args([command, id, "--output", "json"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(Fixture::value(&out)["task_id"], id);
    }
    let independent = TestRepo::new();
    let out = f
        .command()
        .current_dir(independent.path())
        .args(["result", id, "--output", "json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn primary_store_redirect_is_refused_before_creating_a_task_worktree() {
    let f = Fixture::new();
    let repo = ahu::git::discover(f.repo.path()).unwrap();
    let storage = ahu::storage::RepositoryStorage::new(&repo).unwrap();
    ahu::state::create_private_dir_all(&storage.coordination_dir().unwrap()).unwrap();
    let decoy = f.external.path().join("unrelated");
    std::fs::create_dir(&decoy).unwrap();
    std::os::unix::fs::symlink(&decoy, ahu::headless::store(&repo).unwrap()).unwrap();
    let out = f.launch("success", &[]);
    assert!(!out.status.success());
    assert_eq!(std::fs::read_dir(&decoy).unwrap().count(), 0);
    assert!(!f.repo.path().join(".worktrees").exists());
}

#[test]
fn excessive_native_counters_are_rejected_before_helper_accounting() {
    let mut events = ahu::headless::Events::default();
    events.observe("claude-code", br#"{"type":"result","subtype":"success","is_error":false,"result":"synthetic","subagent_stats":{"refused":{"depth_limit":18446744073709551615,"concurrency_limit":1,"budget":1}}}"#);
    assert!(events.failed);
    assert!(
        events
            .blockers
            .iter()
            .any(|b| b.contains("evaluation bound"))
    );
}

#[test]
fn primary_records_remain_readable_after_submitting_sibling_is_removed() {
    let f = Fixture::new();
    let sibling = f.external.path().join("submitting-sibling");
    common::git(
        f.repo.path(),
        &["worktree", "add", "--detach", sibling.to_str().unwrap()],
    );
    let out = f
        .command()
        .current_dir(&sibling)
        .args([
            "launch",
            "@worker",
            "--headless",
            "--prompt",
            "synthetic",
            "--output",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value = Fixture::value(&out);
    let id = value["task_id"].as_str().unwrap();
    common::git(
        f.repo.path(),
        &["worktree", "remove", sibling.to_str().unwrap()],
    );
    for action in ["task", "wait", "result"] {
        let out = f
            .command()
            .args([action, id, "--output", "json"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(Fixture::value(&out)["task_id"], id);
    }
    let repo = ahu::git::discover(f.repo.path()).unwrap();
    let entry = ahu::task_index::lookup_in(&repo, id).unwrap().unwrap();
    assert_eq!(entry.checkout, repo.primary_root().unwrap());
}

#[test]
fn review_regression_ordinary_payloads_do_not_use_helper_bounds() {
    use ahu::headless::Events;
    use serde_json::json;
    for (harness, payload, terminal) in [
        (
            "codex",
            json!({"type":"item.completed","item":{"type":"command_execution","aggregated_output":"x".repeat(4097)}}),
            json!({"type":"turn.completed"}),
        ),
        (
            "codex",
            json!({"type":"item.completed","item":{"type":"function_call","arguments":"x".repeat(4097)}}),
            json!({"type":"turn.completed"}),
        ),
        (
            "claude-code",
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","input":{"command":"x".repeat(4097)}}]},"unused":"x".repeat(4097)}),
            json!({"type":"result","subtype":"success","is_error":false,"result":"done"}),
        ),
    ] {
        let mut events = Events::default();
        events.observe(harness, &serde_json::to_vec(&payload).unwrap());
        events.observe(harness, &serde_json::to_vec(&terminal).unwrap());
        assert!(events.terminal && !events.failed, "{harness}: {events:?}");
    }
}

#[test]
fn review_regression_helper_identifiers_still_bounded() {
    let mut events = ahu::headless::Events::default();
    events.observe("claude-code", &serde_json::to_vec(&serde_json::json!({
        "type":"system","subtype":"task_started","task_id":"x".repeat(4097),"subagent_type":"reader"
    })).unwrap());
    assert!(events.failed);
}

fn malformed_spec_domain_probe(during_execution: bool, own_domain: bool) {
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    for malformed in [false, true] {
        let f = Fixture::new();
        let first = f.launch("success", &[]);
        assert!(first.status.success(), "{first:?}");
        let discovered = ahu::git::discover(f.repo.path()).unwrap();
        let primary = ahu::storage::HeadlessStore::for_repo(&discovered)
            .unwrap()
            .directory;
        let first_id = Fixture::value(&first)["task_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let mut record: Value = serde_json::from_slice(
            &std::fs::read(primary.join(first_id).join("task.json")).unwrap(),
        )
        .unwrap();
        record["task_id"] = json!("abc123");
        let legacy = f.external.path().canonicalize().unwrap().join("legacy");
        std::fs::create_dir(&legacy).unwrap();
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o700)).unwrap();
        ahu::state::write_json(
            &f.repo.path().join(".ahu/state/legacy-lookup.json"),
            &json!({"schema_version":1,"runtime_roots":[legacy]}),
        )
        .unwrap();
        let old = if own_domain {
            primary.join("abc123")
        } else {
            legacy.join(discovered.identity()).join("abc123")
        };
        let seed = f.external.path().join("seed.json");
        std::fs::write(&seed, record.to_string()).unwrap();
        let script = format!(
            "\nfrom pathlib import Path\np=Path({})\np.mkdir(parents=True,exist_ok=True)\n(p/'task.json').write_text(Path({}).read_text())\n(p/'task.json').chmod(0o600)\n{}\n",
            json!(old),
            json!(seed),
            if malformed {
                "(p/'headless.json').write_text('{broken')\n(p/'headless.json').chmod(0o600)"
            } else {
                ""
            }
        );
        if during_execution {
            let harness = f.bin.join("claude");
            let text = std::fs::read_to_string(&harness).unwrap();
            std::fs::write(
                &harness,
                text.replace(
                    "if scenario=='nonzero':",
                    &(script + "\nif scenario=='nonzero':"),
                ),
            )
            .unwrap();
        } else {
            std::fs::create_dir_all(&old).unwrap();
            ahu::state::write_private_file(&old.join("task.json"), record.to_string().as_bytes())
                .unwrap();
            if malformed {
                ahu::state::write_private_file(&old.join("headless.json"), b"{broken").unwrap();
            }
        }
        let out = f.launch("success", &[]);
        if own_domain {
            assert!(
                !out.status.success(),
                "malformed metadata in the owning domain must remain fatal"
            );
            if during_execution {
                assert_eq!(Fixture::value(&out)["outcome"], "supervisor_error");
            }
        } else {
            assert!(out.status.success(), "{out:?}");
            assert_eq!(Fixture::value(&out)["outcome"], "succeeded");
        }
        assert_eq!(
            std::fs::read_to_string(old.join("task.json")).unwrap(),
            record.to_string()
        );
        if malformed {
            assert_eq!(
                std::fs::read_to_string(old.join("headless.json")).unwrap(),
                "{broken"
            );
        } else {
            assert!(!old.join("headless.json").exists());
        }
    }
}

#[test]
fn review_regression_admission_ignores_unrelated_legacy_specs() {
    malformed_spec_domain_probe(false, false);
}

#[test]
fn review_regression_reconciliation_ignores_unrelated_legacy_specs() {
    malformed_spec_domain_probe(true, false);
}

#[test]
fn owning_domain_malformed_specs_still_fail_closed() {
    malformed_spec_domain_probe(false, true);
    malformed_spec_domain_probe(true, true);
}

#[test]
fn ordinary_event_subtypes_do_not_consume_helper_budget() {
    let mut events = ahu::headless::Events::default();
    for _ in 0..300 {
        events.observe("codex", br#"{"type":"item.completed","subtype":"task_payload","item":{"type":"command_execution"}}"#);
    }
    events.observe("codex", br#"{"type":"turn.completed"}"#);
    assert!(events.terminal && !events.failed);
}

#[test]
fn admission_matrix_is_visible_before_launch_and_does_not_gate_interactive_cmux() {
    use std::os::unix::fs::PermissionsExt;
    for (harness, model, executable, version, source, body, reason) in [
        (
            "codex",
            "gpt-6-astra",
            "codex",
            "0.155.1",
            ".codex/plugins",
            "synthetic plugin state",
            "plugin",
        ),
        (
            "opencode",
            "ollama/glm-5.3:cloud",
            "opencode",
            "1.18.31",
            ".local/share/opencode/auth.json",
            "SYNTHETIC_OPAQUE_SENTINEL",
            "authentication store present",
        ),
        (
            "claude-code",
            "claude-opus-5",
            "claude",
            "2.1.270",
            ".claude/settings.json",
            r#"{"enabledPlugins":{"synthetic":true}}"#,
            "plugin hook behavior",
        ),
        (
            "antigravity",
            "gemini-3.1-pro-high",
            "agy",
            "1.2.3",
            "",
            "",
            "unvalidated headless antigravity version",
        ),
    ] {
        let f = Fixture::new();
        f.repo.add_agent_on("matrix", "1.0.0", harness, model);
        f.repo.commit("synthetic admission matrix");
        // Every executable is disposable. A real invocation would leave proof.
        for entry in ahu::catalog::HARNESSES {
            let stub = f.bin.join(entry.executable);
            let observed = if entry.executable == executable {
                version
            } else {
                entry.headless_verified_versions[0]
            };
            std::fs::write(&stub, format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo '{observed}'; exit 0; fi\ntouch \"$HOME/worker-started\"\nexit 97\n")).unwrap();
            std::fs::set_permissions(stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let home = f.external.path().join("home");
        if !source.is_empty() {
            let path = home.join(source);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        let status = f
            .command()
            .args(["cmux", "status", "--output", "json"])
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
        let status = Fixture::value(&status);
        let status = status["integrations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["harness"] == harness)
            .unwrap();
        assert_eq!(status["headless"]["allowed"], false, "{status}");
        assert_eq!(status["cli_version"], version);
        assert!(
            status["headless"]["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v.as_str().unwrap().contains(reason)),
            "{status}"
        );
        let human = f.command().args(["cmux", "status"]).output().unwrap();
        let human = String::from_utf8(human.stdout).unwrap();
        assert!(human.contains(reason), "{human}");
        assert!(human.contains(status["next_action"].as_str().unwrap()));

        let interactive = f
            .command()
            .args([
                "launch",
                "@matrix",
                "--dry-run",
                "--output",
                "json",
                "--prompt",
                "synthetic task",
            ])
            .output()
            .unwrap();
        assert!(
            interactive.status.success(),
            "{}",
            String::from_utf8_lossy(&interactive.stderr)
        );
        let preview = Fixture::value(&interactive);
        assert_eq!(preview["cmux_integration"]["headless"]["allowed"], false);
        assert_eq!(
            preview["cmux_integration"]["profile"]["interactive_supported"],
            true
        );
        let repo = ahu::git::discover(f.repo.path()).unwrap();
        let branches = common::git(f.repo.path(), &["branch", "--list"]);
        for extra in [vec!["--dry-run"], vec![], vec!["--allow-widened-approvals"]] {
            let refused = f
                .command()
                .args([
                    "launch",
                    "@matrix",
                    "--headless",
                    "--output",
                    "json",
                    "--prompt",
                    "synthetic task",
                ])
                .args(&extra)
                .output()
                .unwrap();
            assert!(!refused.status.success(), "{harness}: {extra:?}");
            assert!(
                String::from_utf8_lossy(&refused.stderr).contains(reason),
                "{harness}: {extra:?}: {}",
                String::from_utf8_lossy(&refused.stderr)
            );
            assert!(!home.join("worker-started").exists());
            assert!(!f.external.path().join("runtime").exists());
            assert!(!ahu::headless::store(&repo).unwrap().exists());
            // Planning may create the self-ignoring root, but no task checkout.
            assert!(
                std::fs::read_dir(f.repo.path().join(".worktrees"))
                    .unwrap()
                    .all(|entry| entry.unwrap().file_name() == ".gitignore")
            );
            assert_eq!(common::git(f.repo.path(), &["branch", "--list"]), branches);
            assert_eq!(
                common::git(f.repo.path(), &["worktree", "list", "--porcelain"])
                    .lines()
                    .filter(|line| line.starts_with("worktree "))
                    .count(),
                1
            );
            let listed = f
                .command()
                .args(["tasks", "--output", "json"])
                .output()
                .unwrap();
            assert!(
                listed.status.success(),
                "{}",
                String::from_utf8_lossy(&listed.stderr)
            );
            assert_eq!(Fixture::value(&listed)["tasks"], serde_json::json!([]));
        }
        assert_eq!(preview["executed"], false);
        assert!(!status.to_string().contains("SYNTHETIC_OPAQUE_SENTINEL"));
    }
}

#[test]
fn headless_isolation_refusal_does_not_gate_real_interactive_cmux_dispatch() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let home = f.external.path().join("home");
    std::fs::create_dir(home.join(".claude")).unwrap();
    std::fs::write(
        home.join(".claude/settings.json"),
        r#"{"enabledPlugins":{"synthetic":true}}"#,
    )
    .unwrap();
    let refused = f.launch("success", &[]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("plugin hook behavior"));
    // Planning may create the self-ignoring root, but no task checkout.
    assert!(
        std::fs::read_dir(f.repo.path().join(".worktrees"))
            .unwrap()
            .all(|entry| entry.unwrap().file_name() == ".gitignore")
    );

    // Reuse the saved-group protocol from the synthetic coordinator fixture.
    // Record the real dispatch, without executing its shell command or opening a terminal.
    let cmux = f.bin.join("cmux");
    std::fs::write(&cmux, r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
base = Path(os.environ['HOME'])
args = sys.argv[1:]
if args[0] == 'ping': print('PONG'); sys.exit(0)
if args[0] == 'capabilities':
 print(json.dumps({'capabilities':['workspace.groups.v1','workspace.group_create.v1','workspace.create_in_group.v1']})); sys.exit(0)
if args[0] == 'new-workspace':
 assert not (base/'dispatch.json').exists()
 (base/'dispatch.json').write_text(json.dumps(args)); sys.exit(0)
if args[0] == 'set-status': sys.exit(0)
assert args[0] == 'rpc', args
method, params = args[1], json.loads(args[2])
if method == 'system.identify':
 assert params == {'caller':{'workspace_id':'sentinel'}}
 print(json.dumps({'caller':{'window_id':'synthetic-window'}}))
elif method == 'workspace.group.list':
 assert params == {'window_id':'synthetic-window'}
 members = ['anchor']
 if (base/'dispatch.json').exists(): members.append('synthetic-task')
 print(json.dumps({'groups':[{'id':'saved-group','name':'synthetic group','anchor_workspace_id':'anchor','member_workspace_ids':members}]}))
elif method == 'workspace.list': print(json.dumps({'workspaces':[]}))
elif method == 'workspace.group.expand':
 assert params == {'group_id':'saved-group'}
 print('{}')
else: raise AssertionError(method)
"#).unwrap();
    std::fs::set_permissions(&cmux, std::fs::Permissions::from_mode(0o755)).unwrap();
    let repo = ahu::git::discover(f.repo.path()).unwrap();
    ahu::state::write_json(
        &ahu::state::coordination_dir(&repo)
            .unwrap()
            .join("cmux.json"),
        &ahu::launch::GroupMapping {
            group_id: Some("saved-group".into()),
            window_id: Some("synthetic-window".into()),
            anchor_workspace_id: Some("anchor".into()),
        },
    )
    .unwrap();
    let launched = f
        .command()
        .env("AHU_CMUX_BIN", &cmux)
        .args([
            "launch",
            "@worker",
            "--prompt",
            "synthetic interactive task",
        ])
        .output()
        .unwrap();
    assert!(
        launched.status.success(),
        "{}",
        String::from_utf8_lossy(&launched.stderr)
    );
    let listing = ahu::task::list(&repo).unwrap();
    assert!(listing.unreadable.is_empty());
    assert_eq!(listing.records.len(), 1);
    let (task_dir, record) = &listing.records[0];
    assert_eq!(record.cmux_workspace_id.as_deref(), Some("synthetic-task"));
    assert_eq!(record.cmux_group_id.as_deref(), Some("saved-group"));
    assert!(record.worktree.is_dir());
    assert_eq!(record.identity.agent, "worker");
    let args: Vec<String> =
        serde_json::from_slice(&std::fs::read(home.join("dispatch.json")).unwrap()).unwrap();
    let option = |name| args[args.iter().position(|arg| arg == name).unwrap() + 1].clone();
    assert_eq!(option("--cwd"), record.worktree.to_str().unwrap());
    assert_eq!(option("--group"), "saved-group");
    assert_eq!(
        option("--command"),
        ahu::cmux::startup_command(std::path::Path::new(env!("CARGO_BIN_EXE_ahu")), task_dir)
    );
    assert!(!ahu::headless::store(&repo).unwrap().exists());
    assert!(!f.external.path().join("runtime").exists());
}

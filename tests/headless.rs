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
assert 'ahu delegation contract (v2, headless)' in a[-1]
assert sys.stdin.read()==''
scenario=os.environ.get('SCENARIO','success')
if scenario in ('child','mailbox') and not a[-1].endswith('delegate synthetic task'): scenario='success'
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
  inbox=next(Path(os.environ['AHU_RUNTIME_DIR']).glob('*/'+os.environ['AHU_PARENT_TASK']+'/requests'))
  bad=inbox/'1111111111111111.request.json'; bad.write_text('{'); bad.chmod(0o600)
  bad=inbox/'2222222222222222.request.json'; bad.symlink_to('/dev/null')
  bad=inbox/'3333333333333333.request.json'; bad.write_text('{"executable":"forbidden"}'); bad.chmod(0o600)
  time.sleep(0.3)
 child=os.environ['CHILD_PROMPT']
 env=dict(os.environ,SCENARIO='success')
 r=subprocess.run([os.environ['AHU_BIN'],'launch','@worker','--headless','--background','--output','json','--prompt-file',child],env=env,capture_output=True,text=True)
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
            .env("AHU_STATE_DIR", self.external.path().join("state"))
            .env("HOME", self.external.path().join("home"))
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
fn foreground_preserves_external_evidence_and_identity_without_terminal() {
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
    let artifacts = PathBuf::from(v["artifacts"]["events"].as_str().unwrap());
    assert!(artifacts.exists());
    assert!(!artifacts.starts_with(f.repo.path()));
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
    let out = f.launch("success", &[]);
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
        let events = PathBuf::from(v["artifacts"]["events"].as_str().unwrap());
        let dir = events.parent().unwrap().parent().unwrap();
        let mut spec: Value =
            serde_json::from_slice(&std::fs::read(dir.join("headless.json")).unwrap()).unwrap();
        spec["attempt"] = 2.into();
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
                worktree.join(".agents/ahu/instructions/worker.md"),
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
            .any(|v| v.as_str().unwrap().contains("1 MiB"))
    );
}
#[test]
fn explicit_resume_recovers_a_pre_spawn_interrupted_metadata_transition() {
    let f = Fixture::new();
    let out = f.launch("success", &[]);
    assert!(out.status.success());
    let v = Fixture::value(&out);
    let id = v["task_id"].as_str().unwrap();
    let events = PathBuf::from(v["artifacts"]["events"].as_str().unwrap());
    let dir = events.parent().unwrap().parent().unwrap();
    let record: Value =
        serde_json::from_slice(&std::fs::read(dir.join("task.json")).unwrap()).unwrap();
    let spec: Value =
        serde_json::from_slice(&std::fs::read(dir.join("headless.json")).unwrap()).unwrap();
    let prompt = std::fs::read_to_string(dir.join("prompt.txt")).unwrap();
    std::fs::write(
        dir.join("resume-journal.json"),
        serde_json::to_vec(
            &serde_json::json!({"record":record,"spec":spec,"prompt":prompt,"next_attempt":2}),
        )
        .unwrap(),
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
    let events = PathBuf::from(value["artifacts"]["events"].as_str().unwrap());
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
    let path = f.repo.path().join(".agents/ahu/agents/writer.toml");
    let source = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        source.replace("[source]", "permissions = \"auto\"\n[source]"),
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
    assert!(!PathBuf::from(value["artifacts"]["events"].as_str().unwrap()).exists());
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
    let events = PathBuf::from(value["artifacts"]["events"].as_str().unwrap());
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
if sys.argv[-1].endswith('parent-shutdown'):
 time.sleep(1)
 r=subprocess.run([os.environ['AHU_BIN'],'launch','@worker','--headless','--background','--timeout','6','--prompt','slow-child','--output','json'],capture_output=True,text=True)
 assert r.returncode==0,r.stderr
 open(os.environ['CHILD_ID_FILE'],'w').write(json.loads(r.stdout)['task_id'])
 if os.environ['STOP_MODE']=='capture_failed': print('x'*(1024*1024+1),flush=True)
 time.sleep(30)
else:
 time.sleep(5)
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
                "4",
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
        assert_eq!(value["outcome"], mode, "{value}");
        let child = std::fs::read_to_string(&child_file).unwrap();
        let out = f
            .command()
            .args(["result", &child, "--output", "json"])
            .output()
            .unwrap();
        assert_eq!(Fixture::value(&out)["outcome"], "cancelled");
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
 inbox=next(Path(os.environ['AHU_RUNTIME_DIR']).glob('*/'+os.environ['AHU_PARENT_TASK']+'/requests'))
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
            at.elapsed() < std::time::Duration::from_secs(5),
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
    let script = original.replace("if scenario=='sleep': time.sleep(60)", r#"if scenario=='sleep':
 from pathlib import Path
 inbox=next(Path(os.environ['AHU_RUNTIME_DIR']).glob('*/'+os.environ['AHU_PARENT_TASK']+'/requests'))
 for i in range(270):
  p=inbox/(format(i,'016x')+'.request.json');p.write_text('{');p.chmod(0o600)
 time.sleep(60)"#);
    std::fs::write(f.bin.join("claude"), script).unwrap();
    let out = f.launch("sleep", &["--timeout", "10"]);
    let value = Fixture::value(&out);
    assert_eq!(value["outcome"], "capture_failed", "{value}");
    assert!(
        value["harness"]["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().contains("broker request limit"))
    );
    let events = PathBuf::from(value["artifacts"]["events"].as_str().unwrap());
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
        capped.blockers.iter().any(|b| b.contains("length")),
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
assert 'ahu delegation contract (v2, headless)' in a[-1]
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
    assert_eq!(v["harness"]["summary"], "validated synthetic proof");
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
/// records plain versions, so the gate must compare the first whitespace token
/// rather than the whole line — otherwise every decorated build of a validated
/// version is refused as unvalidated.
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
    let task = PathBuf::from(value["artifacts"]["events"].as_str().unwrap())
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
    let attempt = PathBuf::from(value["artifacts"]["events"].as_str().unwrap())
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

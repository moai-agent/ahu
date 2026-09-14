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
 r=subprocess.run([os.environ['AHU_BIN'],'launch','@worker','--background','--output','json','--prompt-file',child],env=env,capture_output=True,text=True)
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
        let mut c = Command::new(env!("CARGO_BIN_EXE_ahu"));
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
    let out = f
        .command()
        .current_dir(v["worktree"].as_str().unwrap())
        .env("AHU_PARENT_TASK", id)
        .env("AHU_EXECUTION_BACKEND", "headless")
        .args(["launch", "@worker", "--prompt", "child", "--output", "json"])
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
 r=subprocess.run([os.environ['AHU_BIN'],'launch','@worker','--background','--timeout','6','--prompt','slow-child','--output','json'],capture_output=True,text=True)
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

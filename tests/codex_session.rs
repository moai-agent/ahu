mod common;

#[test]
#[cfg(unix)]
fn coordinator_shortcuts_group_the_caller_before_starting_and_refuse_failed_grouping() {
    use std::os::unix::fs::PermissionsExt;
    for (shortcut, program) in [
        ("codex", "codex"),
        ("claude", "claude"),
        ("opencode", "opencode"),
        ("agy", "agy"),
    ] {
        for failure in ["none", "add", "verify", "identify", "conflict"] {
            let fixture = common::TestRepo::new();
            let scratch = tempfile::tempdir().unwrap();
            let repo = ahu::git::discover(fixture.path()).unwrap();
            let cmux = scratch.path().join("cmux");
            std::fs::write(&cmux, r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
base = Path(os.environ['AHU_TEST_COORDINATOR'])
failure = os.environ['AHU_TEST_GROUP_FAILURE']
args = sys.argv[1:]
if args[0] == 'ping': print('PONG'); sys.exit(0)
if args[0] == 'capabilities':
 print(json.dumps({'capabilities':['workspace.groups.v1','workspace.group_create.v1','workspace.create_in_group.v1']})); sys.exit(0)
if args[0] == 'new-workspace':
 (base/'new-workspace').touch(); print('workspace:coordinator'); sys.exit(0)
method, params = args[1], json.loads(args[2])
with (base/'calls').open('a') as f: f.write(json.dumps([method,params])+'\n')
if method == 'system.identify':
 assert params == {'caller':{'workspace_id':'caller'}}
 print(json.dumps({'caller':{'window_id':None if failure == 'identify' else ('target-window' if (base/'moved').exists() else 'caller-window')}}))
elif method == 'workspace.group.list':
 assert params['window_id'] in ['target-window','caller-window']
 members = ['anchor']
 if (base/'added').exists() and failure != 'verify': members.append('caller')
 if (base/'new-workspace').exists(): members.append('coordinator')
 groups = [{'id':'saved-group','name':'renamed repository','anchor_workspace_id':'anchor','member_workspace_ids':members}] if params['window_id']=='target-window' else []
 if failure == 'conflict' and params['window_id'] == 'target-window':
  groups.append({'id':'other-group','name':'other repository','anchor_workspace_id':'caller','member_workspace_ids':['caller']})
 print(json.dumps({'groups':groups}))
elif method == 'workspace.list': print(json.dumps({'workspaces':[]}))
elif method == 'workspace.move_to_window':
 assert params == {'workspace_id':'caller','window_id':'target-window'}
 (base/'moved').touch(); print('{}')
elif method == 'workspace.group.add':
 assert params == {'workspace_id':'caller','window_id':'target-window','group_id':'saved-group'}
 assert (base/'moved').exists()
 if failure == 'add': sys.exit(1)
 (base/'added').touch(); print('{}')
elif method == 'workspace.group.expand':
 assert params == {'group_id':'saved-group'}
 (base/'expanded').touch(); print('{}')
else: raise AssertionError(method)
"#).unwrap();
            std::fs::set_permissions(&cmux, std::fs::Permissions::from_mode(0o700)).unwrap();
            let bin = common::fake_harnesses(scratch.path(), &[program], |_| {
                scratch.path().join("unused")
            });
            std::fs::write(bin.join(program), "#!/bin/sh\ntest -f \"$AHU_TEST_COORDINATOR/expanded\" || exit 99\ntouch \"$AHU_TEST_COORDINATOR/started\"\nexit 7\n").unwrap();
            let mapping = ahu::state::coordination_dir(&repo)
                .unwrap()
                .join("cmux.json");
            ahu::state::write_json(
                &mapping,
                &ahu::launch::GroupMapping {
                    group_id: Some("saved-group".into()),
                    window_id: Some("target-window".into()),
                    anchor_workspace_id: Some("anchor".into()),
                },
            )
            .unwrap();
            let result = common::ahu()
                .arg(shortcut)
                .current_dir(fixture.path())
                .env("CMUX_WORKSPACE_ID", "caller")
                .env("AHU_CMUX_BIN", &cmux)
                .env("AHU_TEST_COORDINATOR", scratch.path())
                .env("AHU_TEST_GROUP_FAILURE", failure)
                .env(
                    "PATH",
                    format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
                )
                .output()
                .unwrap();
            assert_eq!(
                scratch.path().join("started").exists(),
                failure == "none",
                "{shortcut}/{failure}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            if failure == "none" {
                assert_eq!(result.status.code(), Some(7));
            } else if failure == "conflict" {
                assert!(scratch.path().join("new-workspace").exists());
                assert_eq!(result.status.code(), Some(0));
            } else {
                assert!(!result.status.success());
            }
            assert!(
                !ahu::state::coordination_dir(&repo)
                    .unwrap()
                    .join("launch.lock")
                    .exists()
            );
            assert!(!fixture.path().join(".worktrees").exists());
        }
    }
}

#[test]
fn codex_shortcut_has_fixed_yolo_options() {
    assert_eq!(
        ahu::cli::parse(["codex"]).unwrap(),
        ahu::cli::Command::Codex
    );
    for flag in [
        "--sandbox",
        "--ask-for-approval",
        "--dangerously-bypass-approvals-and-sandbox",
    ] {
        assert!(ahu::cli::parse(["codex", flag]).is_err());
    }
}

#[test]
#[cfg(unix)]
fn codex_session_inherits_terminal_context_and_uses_local_state_without_cmux() {
    let repo = common::TestRepo::new();
    let scratch = tempfile::tempdir().unwrap();
    let bin = common::fake_harnesses(scratch.path(), &["codex"], |_| {
        scratch.path().join("unused")
    });
    std::fs::write(
        bin.join("codex"),
        r#"#!/bin/sh
printf '%s\n' "$@" > "$AHU_TEST_ARGS"
pwd > "$AHU_TEST_CWD"
printf '%s' "${AHU_STATE_DIR-unset}" > "$AHU_TEST_STATE"
"$AHU_BIN" tasks --output json > "$AHU_TEST_TASKS" || exit 99
exit 7
"#,
    )
    .unwrap();
    let nested = repo.path().join("subdirectory");
    std::fs::create_dir(&nested).unwrap();
    let output = common::ahu()
        .arg("codex")
        .env_remove("CMUX_WORKSPACE_ID")
        .current_dir(&nested)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("AHU_STATE_DIR", scratch.path().join("parent-state"))
        .env("AHU_CMUX_BIN", scratch.path().join("missing-cmux"))
        .env("AHU_TEST_ARGS", scratch.path().join("args"))
        .env("AHU_TEST_CWD", scratch.path().join("cwd"))
        .env("AHU_TEST_STATE", scratch.path().join("state"))
        .env("AHU_TEST_TASKS", scratch.path().join("tasks"))
        .env("AHU_RUNTIME_DIR", scratch.path().join("runtime"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(scratch.path().join("args")).unwrap(),
        "--dangerously-bypass-approvals-and-sandbox\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "Codex coordinator: --dangerously-bypass-approvals-and-sandbox\n"
    );
    let cwd = std::fs::read_to_string(scratch.path().join("cwd")).unwrap();
    assert_eq!(
        std::path::Path::new(cwd.trim()).canonicalize().unwrap(),
        nested.canonicalize().unwrap()
    );
    let state = std::fs::read_to_string(scratch.path().join("state")).unwrap();
    assert_eq!(
        std::path::Path::new(&state),
        scratch.path().join("parent-state")
    );
    assert!(output.stdout.is_empty());
    assert!(!repo.path().join(".agents").exists());
    assert!(!repo.path().join(".worktrees").exists());
    assert!(!scratch.path().join("parent-state").exists());
    assert_eq!(common::git(repo.path(), &["status", "--porcelain"]), "");
}

#[test]
fn claude_shortcut_has_fixed_permission_bypass_and_rejects_overrides() {
    assert_eq!(
        ahu::cli::parse(["claude"]).unwrap(),
        ahu::cli::Command::Claude
    );
    for flag in [
        "--model",
        "--permission-mode",
        "--dangerously-skip-permissions",
    ] {
        assert!(ahu::cli::parse(["claude", flag]).is_err());
    }
}
#[test]
#[cfg(unix)]
fn claude_session_inherits_terminal_context_and_uses_local_state_without_cmux() {
    let repo = common::TestRepo::new();
    let scratch = tempfile::tempdir().unwrap();
    let bin = common::fake_harnesses(scratch.path(), &["claude"], |_| {
        scratch.path().join("unused")
    });
    std::fs::write(
        bin.join("claude"),
        r#"#!/bin/sh
printf '%s\n' "$@" > "$AHU_TEST_ARGS"
pwd > "$AHU_TEST_CWD"
printf '%s' "${AHU_STATE_DIR-unset}" > "$AHU_TEST_STATE"
"$AHU_BIN" tasks --output json > "$AHU_TEST_TASKS" || exit 99
exit 7
"#,
    )
    .unwrap();
    let nested = repo.path().join("subdirectory");
    std::fs::create_dir(&nested).unwrap();
    let output = common::ahu()
        .arg("claude")
        .env_remove("CMUX_WORKSPACE_ID")
        .current_dir(&nested)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("AHU_STATE_DIR", scratch.path().join("parent-state"))
        .env("AHU_CMUX_BIN", scratch.path().join("missing-cmux"))
        .env("AHU_TEST_ARGS", scratch.path().join("args"))
        .env("AHU_TEST_CWD", scratch.path().join("cwd"))
        .env("AHU_TEST_STATE", scratch.path().join("state"))
        .env("AHU_TEST_TASKS", scratch.path().join("tasks"))
        .env("AHU_RUNTIME_DIR", scratch.path().join("runtime"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(scratch.path().join("args")).unwrap(),
        "--dangerously-skip-permissions\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "Claude coordinator: --dangerously-skip-permissions\n"
    );
    let cwd = std::fs::read_to_string(scratch.path().join("cwd")).unwrap();
    assert_eq!(
        std::path::Path::new(cwd.trim()).canonicalize().unwrap(),
        nested.canonicalize().unwrap()
    );
    let state = std::fs::read_to_string(scratch.path().join("state")).unwrap();
    assert_eq!(
        std::path::Path::new(&state),
        scratch.path().join("parent-state")
    );
    assert!(output.stdout.is_empty());
    assert!(!repo.path().join(".agents").exists());
    assert!(!repo.path().join(".worktrees").exists());
    assert!(!scratch.path().join("parent-state").exists());
    assert_eq!(common::git(repo.path(), &["status", "--porcelain"]), "");
}

#[test]
fn opencode_shortcut_preserves_native_defaults_and_rejects_overrides() {
    assert_eq!(
        ahu::cli::parse(["opencode"]).unwrap(),
        ahu::cli::Command::OpenCode
    );
    // `--auto` widens to every permission OpenCode does not explicitly deny and
    // `--pure` disables the user's own plugins. Neither is ahu's call to make
    // on a coordinating session, so neither is accepted here.
    for flag in ["--model", "--auto", "--pure", "--agent"] {
        assert!(ahu::cli::parse(["opencode", flag]).is_err());
    }
}

#[test]
#[cfg(unix)]
fn opencode_session_inherits_terminal_context_and_uses_local_state_without_cmux() {
    let repo = common::TestRepo::new();
    let scratch = tempfile::tempdir().unwrap();
    let bin = common::fake_harnesses(scratch.path(), &["opencode"], |_| {
        scratch.path().join("unused")
    });
    std::fs::write(
        bin.join("opencode"),
        r#"#!/bin/sh
printf '%s\n' "$@" > "$AHU_TEST_ARGS"
pwd > "$AHU_TEST_CWD"
printf '%s' "${AHU_STATE_DIR-unset}" > "$AHU_TEST_STATE"
"$AHU_BIN" tasks --output json > "$AHU_TEST_TASKS" || exit 99
exit 7
"#,
    )
    .unwrap();
    let nested = repo.path().join("subdirectory");
    std::fs::create_dir(&nested).unwrap();
    let output = common::ahu()
        .arg("opencode")
        .env_remove("CMUX_WORKSPACE_ID")
        .current_dir(&nested)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("AHU_STATE_DIR", scratch.path().join("parent-state"))
        .env("AHU_CMUX_BIN", scratch.path().join("missing-cmux"))
        .env("AHU_TEST_ARGS", scratch.path().join("args"))
        .env("AHU_TEST_CWD", scratch.path().join("cwd"))
        .env("AHU_TEST_STATE", scratch.path().join("state"))
        .env("AHU_TEST_TASKS", scratch.path().join("tasks"))
        .env("AHU_RUNTIME_DIR", scratch.path().join("runtime"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // No arguments at all: the session is whatever the user's own OpenCode
    // configuration makes it.
    assert_eq!(
        std::fs::read_to_string(scratch.path().join("args")).unwrap(),
        "\n"
    );
    let cwd = std::fs::read_to_string(scratch.path().join("cwd")).unwrap();
    assert_eq!(
        std::path::Path::new(cwd.trim()).canonicalize().unwrap(),
        nested.canonicalize().unwrap()
    );
    let state = std::fs::read_to_string(scratch.path().join("state")).unwrap();
    assert_eq!(
        std::path::Path::new(&state),
        scratch.path().join("parent-state")
    );
    assert!(output.stdout.is_empty());
    assert!(!repo.path().join(".agents").exists());
    assert!(!repo.path().join(".worktrees").exists());
    assert!(!scratch.path().join("parent-state").exists());
    assert_eq!(common::git(repo.path(), &["status", "--porcelain"]), "");
}

#[test]
fn antigravity_shortcut_uses_yolo_mode_and_rejects_overrides() {
    assert_eq!(
        ahu::cli::parse(["agy"]).unwrap(),
        ahu::cli::Command::Antigravity
    );
    // The shortcut owns the fixed YOLO flag; callers cannot replace its mode.
    for flag in [
        "--model",
        "--mode",
        "--dangerously-skip-permissions",
        "--agent",
    ] {
        assert!(ahu::cli::parse(["agy", flag]).is_err());
    }
}

#[test]
#[cfg(unix)]
fn antigravity_session_inherits_terminal_context_and_uses_local_state_without_cmux() {
    let repo = common::TestRepo::new();
    let scratch = tempfile::tempdir().unwrap();
    let bin = common::fake_harnesses(scratch.path(), &["agy"], |_| scratch.path().join("unused"));
    std::fs::write(
        bin.join("agy"),
        r#"#!/bin/sh
printf '%s\n' "$@" > "$AHU_TEST_ARGS"
pwd > "$AHU_TEST_CWD"
printf '%s' "${AHU_STATE_DIR-unset}" > "$AHU_TEST_STATE"
"$AHU_BIN" tasks --output json > "$AHU_TEST_TASKS" || exit 99
exit 7
"#,
    )
    .unwrap();
    let nested = repo.path().join("subdirectory");
    std::fs::create_dir(&nested).unwrap();
    let output = common::ahu()
        .arg("agy")
        .env_remove("CMUX_WORKSPACE_ID")
        .current_dir(&nested)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("AHU_STATE_DIR", scratch.path().join("parent-state"))
        .env("AHU_CMUX_BIN", scratch.path().join("missing-cmux"))
        .env("AHU_TEST_ARGS", scratch.path().join("args"))
        .env("AHU_TEST_CWD", scratch.path().join("cwd"))
        .env("AHU_TEST_STATE", scratch.path().join("state"))
        .env("AHU_TEST_TASKS", scratch.path().join("tasks"))
        .env("AHU_RUNTIME_DIR", scratch.path().join("runtime"))
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // YOLO mode is fixed by the coordinator shortcut.
    assert_eq!(
        std::fs::read_to_string(scratch.path().join("args")).unwrap(),
        "--dangerously-skip-permissions\n"
    );
    let cwd = std::fs::read_to_string(scratch.path().join("cwd")).unwrap();
    assert_eq!(
        std::path::Path::new(cwd.trim()).canonicalize().unwrap(),
        nested.canonicalize().unwrap()
    );
    let state = std::fs::read_to_string(scratch.path().join("state")).unwrap();
    assert_eq!(
        std::path::Path::new(&state),
        scratch.path().join("parent-state")
    );
    assert!(output.stdout.is_empty());
    assert!(!repo.path().join(".agents").exists());
    assert!(!repo.path().join(".worktrees").exists());
    assert!(!scratch.path().join("parent-state").exists());
    assert_eq!(common::git(repo.path(), &["status", "--porcelain"]), "");
}

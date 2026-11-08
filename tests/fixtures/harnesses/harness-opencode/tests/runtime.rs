//! Exercise the actual executable boundary without launching a native CLI.
use serde_json::json;
use std::io::Write;
use std::process::{Command, Stdio};

// Preserve only coverage routing while keeping native/provider configuration absent.
fn runtime_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ahu-plugin-opencode"));
    command.env_clear();
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    command
}

#[test]
fn executable_serves_multiple_correlated_requests() {
    let mut child = runtime_command()
        .arg("--ahu-harness-runtime-protocol=1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    for (i, mode) in ["direct", "interactive", "headless"].iter().enumerate() {
        let request = json!({"protocol_version":1,"id":i+1,"operation":"launch","params":{
            "mode":mode,"model":"provider/model","permissions":if *mode=="headless" {"auto"} else {"prompt"},
            "work_mode":*mode=="headless","session":null,"native_version":"2.0.32",
            "timeout_seconds":60,"cwd":"/synthetic/worktree","broker_dir":null,"native_profile":null,"native_controls":[]
        }});
        writeln!(input, "{request}").unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 3);
    for (i, line) in lines.iter().enumerate() {
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(value["id"], i + 1);
        assert!(value.get("result").is_some());
        assert!(value.get("error").is_none());
    }
}
#[test]
fn executable_refuses_old_protocol_invocation() {
    let output = runtime_command()
        .arg("--ahu-harness-launch-protocol=2")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

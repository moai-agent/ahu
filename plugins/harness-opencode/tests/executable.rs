use std::io::Write;
use std::process::{Command, Stdio};

fn invoke(arguments: &[&str], input: &[u8]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ahu-plugin-opencode"))
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn executable_contract() {
    for arguments in [
        vec![],
        vec!["--ahu-harness-launch-protocol=1"],
        vec!["--ahu-harness-launch-protocol=2", "extra"],
    ] {
        let output = invoke(&arguments, b"");
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert_eq!(output.stderr, b"invalid invocation\n");
    }
    let output = invoke(
        &["--ahu-harness-launch-protocol=2"],
        b"sensitive-invalid-input",
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"invalid request\n");
    let request = serde_json::json!({"protocol_version":2,"harness":"opencode",
        "mode":"headless","model":"provider/model","permissions":"auto","work_mode":true,
        "session":null,"native_version":"2.0.32","timeout_seconds":1});
    let output = invoke(
        &["--ahu-harness-launch-protocol=2"],
        request.to_string().as_bytes(),
    );
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["harness"], "opencode");
    assert_eq!(
        plan["args"].as_array().unwrap().last().unwrap(),
        "<prompt: see prompt.txt>"
    );
}

#[test]
fn stalled_input_is_bounded_without_a_host_timeout() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ahu-plugin-opencode"))
        .arg("--ahu-harness-launch-protocol=2")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // Keep stdin open, supplying no EOF. The executable must terminate itself.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(2));
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("planner did not bound stalled input");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

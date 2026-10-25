use super::*;
use serde_json::json;

#[test]
fn exact_native_arguments() {
    let request = serde_json::from_value(json!({"protocol_version":2,"harness":HARNESS,
        "mode":"interactive","permissions":"prompt","work_mode":false,
        "session":serde_json::Value::Null,"model":"provider/model:tag","native_version":NATIVE_VERSION,"timeout_seconds":60})).unwrap();
    assert_eq!(
        plan(request).unwrap().args,
        [
            "-m",
            "provider/model:tag",
            "--no-daemon",
            "--",
            "<prompt: see prompt.txt>"
        ]
    );
    let request = serde_json::from_value(json!({"protocol_version":2,"harness":HARNESS,
        "mode":"interactive","permissions":"accept-edits","work_mode":false,
        "session":serde_json::Value::Null,"model":"provider/model:tag","native_version":NATIVE_VERSION,"timeout_seconds":60})).unwrap();
    assert_eq!(
        plan(request).unwrap().args,
        [
            "-m",
            "provider/model:tag",
            "--no-daemon",
            "--approve-for-me",
            "--",
            "<prompt: see prompt.txt>"
        ]
    );
    let request = serde_json::from_value(json!({"protocol_version":2,"harness":HARNESS,
        "mode":"interactive","permissions":"auto","work_mode":false,
        "session":serde_json::Value::Null,"model":"provider/model:tag","native_version":NATIVE_VERSION,"timeout_seconds":60})).unwrap();
    assert_eq!(
        plan(request).unwrap().args,
        [
            "-m",
            "provider/model:tag",
            "--no-daemon",
            "--ask-for-approval",
            "never",
            "--sandbox",
            "workspace-write",
            "--",
            "<prompt: see prompt.txt>"
        ]
    );
    let request = serde_json::from_value(json!({"protocol_version":2,"harness":HARNESS,
        "mode":"headless","permissions":"auto","work_mode":true,
        "session":serde_json::Value::Null,"model":"provider/model:tag","native_version":NATIVE_VERSION,"timeout_seconds":60})).unwrap();
    assert_eq!(
        plan(request).unwrap().args,
        [
            "exec",
            "--model",
            "provider/model:tag",
            "--json",
            "-c",
            "approval_policy=\"never\"",
            "-c",
            "sandbox_mode=\"danger-full-access\"",
            "--color",
            "never",
            "--",
            "<prompt: see prompt.txt>"
        ]
    );
    let request = serde_json::from_value(json!({"protocol_version":2,"harness":HARNESS,
        "mode":"headless","permissions":"auto","work_mode":true,
        "session":json!("session_123"),"model":"provider/model:tag","native_version":NATIVE_VERSION,"timeout_seconds":60})).unwrap();
    assert_eq!(
        plan(request).unwrap().args,
        [
            "exec",
            "resume",
            "--model",
            "provider/model:tag",
            "--json",
            "-c",
            "approval_policy=\"never\"",
            "-c",
            "sandbox_mode=\"danger-full-access\"",
            "--",
            "session_123",
            "<prompt: see prompt.txt>"
        ]
    );
}

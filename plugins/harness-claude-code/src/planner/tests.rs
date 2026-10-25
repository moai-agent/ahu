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
            "--model",
            "provider/model:tag",
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
            "--model",
            "provider/model:tag",
            "--permission-mode",
            "acceptEdits",
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
            "--model",
            "provider/model:tag",
            "--permission-mode",
            "auto",
            "--settings",
            "{\"enabledMcpjsonServers\":[\"ahu\"]}",
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
            "--print",
            "--model",
            "provider/model:tag",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-prompts",
            "none",
            "--permission-mode",
            "auto",
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
            "--print",
            "--model",
            "provider/model:tag",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-prompts",
            "none",
            "--permission-mode",
            "auto",
            "--resume",
            "session_123",
            "--",
            "<prompt: see prompt.txt>"
        ]
    );
}

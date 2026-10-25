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
            "mini",
            "--model",
            "provider/model:tag",
            "--prompt",
            "<prompt: see prompt.txt>"
        ]
    );
    let request = serde_json::from_value(json!({"protocol_version":2,"harness":HARNESS,
        "mode":"headless","permissions":"auto","work_mode":true,
        "session":serde_json::Value::Null,"model":"provider/model:tag","native_version":NATIVE_VERSION,"timeout_seconds":60})).unwrap();
    assert_eq!(
        plan(request).unwrap().args,
        [
            "run",
            "--standalone",
            "--format",
            "json",
            "--model",
            "provider/model:tag",
            "--auto",
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
            "run",
            "--standalone",
            "--format",
            "json",
            "--model",
            "provider/model:tag",
            "--auto",
            "--session",
            "session_123",
            "--",
            "<prompt: see prompt.txt>"
        ]
    );
}

#[test]
fn provider_model_shape_is_required() {
    for model in [
        "model",
        "/model",
        "provider/",
        "provider/a/b",
        "provider/-model",
        "provider/.model",
        "provider:tag/model",
    ] {
        let request = serde_json::from_value(json!({"protocol_version":2,"harness":HARNESS,
            "mode":"headless","permissions":"auto","work_mode":true,"session":null,
            "model":model,"native_version":NATIVE_VERSION,"timeout_seconds":60}))
        .unwrap();
        assert!(plan(request).is_err(), "{model}");
    }
}

use ahu_harness_events::Decoder;
use serde_json::{Value, json};

fn feed(decoder: &mut Decoder, event: Value) -> Value {
    decoder
        .observe_stdout(&serde_json::to_vec(&event).unwrap())
        .unwrap()
}

#[test]
fn all_providers_keep_terminal_summary_and_unknown_usage_separate() {
    let fixtures = [
        (
            "codex",
            vec![
                json!({"type":"item.completed","item":{"type":"agent_message","text":"answer"}}),
                json!({"type":"turn.completed"}),
            ],
        ),
        (
            "claude-code",
            vec![json!({"type":"result","subtype":"success","is_error":false,"result":"answer"})],
        ),
        (
            "antigravity",
            vec![json!({"type":"result","result":{"status":"success","response":"answer"}})],
        ),
        (
            "opencode",
            vec![
                json!({"type":"text","part":{"text":"answer"}}),
                json!({"type":"step_finish","part":{"id":"end","reason":"stop"}}),
            ],
        ),
        (
            "kiro",
            vec![
                json!({"type":"runStarted","data":{"engine":"v3","payloadSchema":"acp","acpProtocolVersion":1}}),
                json!({"type":"runFinished","data":{"sessionId":"sess_synthetic","status":"success","stopReason":"end_turn","finalText":"answer","finalTextTruncated":false}}),
            ],
        ),
    ];
    for (harness, events) in fixtures {
        let mut decoder = Decoder::new(harness).unwrap();
        for event in events {
            feed(&mut decoder, event);
        }
        let result = decoder.finish().unwrap();
        assert_eq!(result["terminal"], true, "{harness}");
        assert_eq!(result["failed"], false, "{harness}: {result}");
        assert_eq!(result["summary"], "answer");
        assert!(result["usage"]["total"].is_null());
        assert!(result["cost"].get("usd").is_none());
        assert_eq!(decoder.finish().unwrap(), result);
    }
    assert!(Decoder::new("invented").is_err());
}

#[test]
fn stream_order_and_opencode_snapshot_dedup_are_preserved() {
    let mut decoder = Decoder::new("opencode").unwrap();
    let step = json!({"type":"step_finish","sessionID":"ses_fixture","part":{"id":"step-a","reason":"stop","cost":0.25,"tokens":{"input_tokens":10}}});
    feed(&mut decoder, step.clone());
    decoder
        .observe_stderr(b"Reading prompt from stdin...")
        .unwrap();
    feed(&mut decoder, step);
    let result = feed(
        &mut decoder,
        json!({"type":"reasoning","part":{"text":"PRIVATE REASONING"}}),
    );
    assert_eq!(result["cost"]["usd"], 0.25);
    assert_eq!(result["terminal"], false);
    assert_eq!(result["usage"]["input"], 10);
    assert!(!result.to_string().contains("PRIVATE"));
    assert_eq!(decoder.finish().unwrap()["failed"], true);
}

#[test]
fn malformed_input_is_sticky_and_errors_never_echo_native_data() {
    for line in [b"PRIVATE INVALID".as_slice(), b"null", b"[]", b"{", b"\xff"] {
        let mut decoder = Decoder::new("codex").unwrap();
        let error = decoder.observe_stdout(line).unwrap_err();
        assert!(!error.to_string().contains("PRIVATE"));
        feed(&mut decoder, json!({"type":"turn.completed"}));
        let result = decoder.finish().unwrap();
        assert_eq!(result["failed"], true);
        assert_eq!(result["trajectory"]["malformed_events"], 1);
        assert_eq!(result["native_completeness"]["complete"], false);
    }
}

#[test]
fn session_drift_invalid_values_and_alias_conflicts_are_failures() {
    for value in [
        json!("other"),
        json!(42),
        json!("bad\nidentity"),
        Value::Null,
    ] {
        let mut decoder = Decoder::new("codex").unwrap();
        feed(
            &mut decoder,
            json!({"type":"thread.started","thread_id":"first"}),
        );
        let result = feed(
            &mut decoder,
            json!({"type":"turn.completed","thread_id":value}),
        );
        assert_eq!(result["failed"], true);
        assert_eq!(result["session"], "first");
    }
    let mut decoder = Decoder::new("codex").unwrap();
    assert_eq!(
        feed(
            &mut decoder,
            json!({"type":"thread.started","thread_id":"one","session_id":"two"})
        )["failed"],
        true
    );
}

#[test]
fn mixed_stderr_keeps_only_classifications_and_late_failure() {
    let mut decoder = Decoder::new("codex").unwrap();
    feed(&mut decoder, json!({"type":"turn.completed"}));
    for _ in 0..100 {
        decoder
            .observe_stderr(b"PRIVATE permission denied")
            .unwrap();
    }
    decoder
        .observe_stderr(b"PRIVATE unknown diagnostic")
        .unwrap();
    decoder.observe_stderr(b"PRIVATE invalid_api_key").unwrap();
    decoder
        .observe_stderr(b"PRIVATE resource_exhausted")
        .unwrap();
    let result = decoder.finish().unwrap();
    assert_eq!(result["failed"], true);
    assert_eq!(result["stderr_unclassified_lines"], 1);
    assert_eq!(result["stderr_diagnostics"].as_array().unwrap().len(), 3);
    assert!(!result.to_string().contains("PRIVATE"));
    assert!(decoder.observe_stdout(b"{}").is_err());
    assert!(decoder.observe_stderr(b"extra").is_err());
}

#[test]
fn bounds_on_lines_depth_summary_and_write_targets_fail_closed() {
    let mut decoder = Decoder::new("codex").unwrap();
    assert!(
        decoder
            .observe_stderr(&vec![b'x'; 1024 * 1024 + 1])
            .is_err()
    );
    assert_eq!(decoder.snapshot()["failed"], true);
    let mut decoder = Decoder::new("codex").unwrap();
    let mut nested = json!(null);
    for _ in 0..65 {
        nested = json!({"nested":nested});
    }
    assert!(
        decoder
            .observe_stdout(&serde_json::to_vec(&nested).unwrap())
            .is_err()
    );
    let mut decoder = Decoder::new("codex").unwrap();
    let message =
        json!({"type":"item.completed","item":{"type":"agent_message","text":"x".repeat(65537)}});
    assert!(
        decoder
            .observe_stdout(&serde_json::to_vec(&message).unwrap())
            .is_err()
    );
    assert_eq!(decoder.snapshot()["summary"], "");
    let mut decoder = Decoder::new("claude-code").unwrap();
    let event = json!({"type":"tool_use","name":"Write","input":{"file_path":"/synthetic/target","content":"PRIVATE TOOL PAYLOAD"}});
    for _ in 0..2 {
        feed(&mut decoder, event.clone());
    }
    assert_eq!(
        decoder.snapshot()["write_target_candidates"],
        json!(["/synthetic/target"])
    );
    assert!(!decoder.snapshot().to_string().contains("PRIVATE"));
    let event = json!({"type":"tool_use","name":"Write","input":{"file_path":"x".repeat(4097)}});
    assert!(
        decoder
            .observe_stdout(&serde_json::to_vec(&event).unwrap())
            .is_err()
    );
}

#[test]
fn duplicate_terminals_and_unknown_events_remain_visible() {
    let mut decoder = Decoder::new("codex").unwrap();
    feed(
        &mut decoder,
        json!({"type":"future-event","payload":"PRIVATE"}),
    );
    feed(&mut decoder, json!({"type":"turn.completed"}));
    feed(&mut decoder, json!({"type":"turn.completed"}));
    let result = decoder.finish().unwrap();
    assert_eq!(result["unknown_events"], 1);
    assert_eq!(result["failed"], true);
    assert!(!result.to_string().contains("PRIVATE"));
}

#[test]
fn kiro_replay_cannot_supply_writes_helpers_or_credit_usage() {
    let mut decoder = Decoder::new("kiro").unwrap();
    feed(
        &mut decoder,
        json!({"type":"runStarted","data":{"engine":"v3","payloadSchema":"acp"}}),
    );
    let result = feed(
        &mut decoder,
        json!({"type":"sessionUpdate","data":{"sessionId":"sess_fixture","update":{"sessionUpdate":"tool_call","toolCallId":"old","kind":"edit","status":"failed","locations":[{"path":"/synthetic/old"}],"_meta":{"kiro":{"replay":true,"kind":"agent-subtask"}}}}}),
    );
    assert_eq!(result["failed"], false);
    assert_eq!(result["write_target_candidates"], json!([]));
    assert_eq!(result["native"]["unknown_events"], 0);
    assert!(result["usage"]["total"].is_null());
}

#[test]
fn helper_overflow_cannot_certify_lifecycle_coverage() {
    let mut decoder = Decoder::new("claude-code").unwrap();
    for _ in 0..257 {
        feed(
            &mut decoder,
            json!({"type":"system","subtype":"task_progress","task_id":"unknown"}),
        );
    }
    let result = feed(
        &mut decoder,
        json!({"type":"result","subtype":"success","is_error":false,"result":"ok"}),
    );
    assert_eq!(result["failed"], true);
    assert_eq!(result["native_completeness"]["complete"], false);
    assert!(result["blockers"].as_array().unwrap().len() <= 32);
}

#[test]
fn cumulative_stream_and_write_set_limits_are_enforced() {
    let mut decoder = Decoder::new("codex").unwrap();
    let line = vec![b' '; 1024 * 1024];
    for _ in 0..64 {
        decoder.observe_stderr(&line).unwrap();
    }
    assert!(decoder.observe_stderr(b"x").is_err());
    assert_eq!(decoder.snapshot()["trajectory"]["stream_incomplete"], true);

    let mut decoder = Decoder::new("claude-code").unwrap();
    for index in 0..256 {
        feed(
            &mut decoder,
            json!({"type":"tool_use","name":"Write","input":{"path":format!("/synthetic/{index}")}}),
        );
    }
    // An already observed target consumes no additional slot.
    feed(
        &mut decoder,
        json!({"type":"tool_use","name":"Write","input":{"path":"/synthetic/0"}}),
    );
    let extra = json!({"type":"tool_use","name":"Write","input":{"path":"/synthetic/overflow"}});
    assert!(
        decoder
            .observe_stdout(&serde_json::to_vec(&extra).unwrap())
            .is_err()
    );
    assert_eq!(
        decoder.snapshot()["write_target_candidates"]
            .as_array()
            .unwrap()
            .len(),
        256
    );
}

#[test]
fn conflicting_or_redacted_model_evidence_never_claims_one_inference_identity() {
    for (alias, coverage) in [
        (json!("different"), "conflicting"),
        (json!("sk-private"), "redacted"),
    ] {
        let mut decoder = Decoder::new("opencode").unwrap();
        let result = feed(
            &mut decoder,
            json!({"type":"step_finish","model":"fixture","model_name":alias,"part":{"id":"end","reason":"stop"}}),
        );
        assert_eq!(
            result["model_attributes"]["ahu.harness.model.reported.coverage"],
            coverage
        );
        assert!(
            result["model_attributes"]
                .get("ahu.harness.model.reported")
                .is_none()
        );
        assert!(!result.to_string().contains("sk-private"));
    }
}

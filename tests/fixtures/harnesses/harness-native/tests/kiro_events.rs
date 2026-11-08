//! Migrated native event assertions exercise the plugin dependency, never core.
fn kiro_event(events: &mut ahu_harness_events::Decoder, event: serde_json::Value) {
    events
        .observe_stdout(&serde_json::to_vec(&event).unwrap())
        .unwrap();
}
fn finished() -> serde_json::Value {
    serde_json::json!({"type":"runFinished","data":{"sessionId":"sess_fixture","status":"success","stopReason":"end_turn","finalText":"private response","finalTextTruncated":false}})
}
fn kiro_started() -> ahu_harness_events::Decoder {
    let mut events = ahu_harness_events::Decoder::new("kiro").unwrap();
    kiro_event(
        &mut events,
        serde_json::json!({"type":"runStarted","data":{"engine":"v3","payloadSchema":"acp","acpProtocolVersion":1}}),
    );
    events
}
#[test]
fn kiro_requires_final_run_result_and_preserves_denials_and_identity_failures() {
    use serde_json::json;
    let mut events = kiro_started();
    kiro_event(
        &mut events,
        json!({"type":"sessionUpdate","data":{"sessionId":"sess_fixture","update":{"sessionUpdate":"session_info_update","_meta":{"kiro":{"kind":"turn_end","stopReason":"end_turn"}}}}}),
    );
    assert!(!events.snapshot()["terminal"].as_bool().unwrap());
    kiro_event(&mut events, finished());
    assert!(
        events.snapshot()["terminal"].as_bool().unwrap()
            && !events.snapshot()["failed"].as_bool().unwrap()
    );
    assert_eq!(events.snapshot()["session"].as_str(), Some("sess_fixture"));
    // The plugin returns an ephemeral summary; core owns excluding it from
    // durable event serialization. Other observation fields contain no body.
    let mut observation = events.snapshot();
    assert_eq!(observation["summary"], "private response");
    observation.as_object_mut().unwrap().remove("summary");
    assert!(!observation.to_string().contains("private response"));
    assert!(events.snapshot()["cost"]["usd"].is_null());
    kiro_event(&mut events, finished());
    assert!(events.snapshot()["failed"].as_bool().unwrap());
    for (index, update) in [
        json!({"sessionUpdate":"tool_call_update","toolCallId":"a","status":"failed","rawOutput":{"message":"private denial"},"_meta":{"kiro":{"policyDenial":{"effect":"deny"}}}}),
        json!({"sessionUpdate":"session_info_update"}),
    ].into_iter().enumerate() {
        let mut events = kiro_started();
        kiro_event(
            &mut events,
            json!({"type":"sessionUpdate","data":{"sessionId":if index==0 {"sess_fixture"} else {"sess_other"},"update":update}}),
        );
        kiro_event(&mut events, finished());
        assert!(events.snapshot()["failed"].as_bool().unwrap());
        assert!(
            !serde_json::to_string(&events.snapshot())
                .unwrap()
                .contains("private denial")
        );
    }
    for (field, value) in [
        ("status", json!("interrupted")),
        ("stopReason", json!("max_tokens")),
        ("finalTextTruncated", json!(true)),
        ("sessionId", json!("--bad")),
    ] {
        let mut event = finished();
        event["data"][field] = value;
        let mut events = kiro_started();
        kiro_event(&mut events, event);
        assert!(events.snapshot()["failed"].as_bool().unwrap(), "{field}");
    }
    let mut events = kiro_started();
    kiro_event(
        &mut events,
        json!({"type":"runError","data":{"sessionId":"sess_fixture","stage":"prompt","message":"private provider error"}}),
    );
    assert!(
        events.snapshot()["failed"].as_bool().unwrap()
            && events.snapshot()["terminal"].as_bool().unwrap()
    );
    assert_eq!(events.snapshot()["session"].as_str(), Some("sess_fixture"));
    assert!(
        !serde_json::to_string(&events.snapshot())
            .unwrap()
            .contains("private provider error")
    );
    let mut events = kiro_started();
    assert!(events.observe_stdout(b"{broken").is_err());
    assert!(
        events.snapshot()["failed"].as_bool().unwrap()
            && !events.snapshot()["terminal"].as_bool().unwrap()
    );
}

#[test]
fn kiro_events_feed_partial_acp_call_evidence_without_inferred_usage() {
    use serde_json::json;
    let mut events = ahu_harness_events::Decoder::new("kiro").unwrap();
    kiro_event(
        &mut events,
        json!({"type":"runStarted","data":{"engine":"v3","payloadSchema":"acp"}}),
    );
    kiro_event(
        &mut events,
        json!({"type":"sessionUpdate","data":{"sessionId":"sess_fixture","update":{"sessionUpdate":"tool_call","toolCallId":"fixture-call","title":"Read File","status":"pending"}}}),
    );
    kiro_event(
        &mut events,
        json!({"type":"sessionUpdate","data":{"sessionId":"sess_fixture","update":{"sessionUpdate":"tool_call_update","toolCallId":"fixture-call","status":"completed"}}}),
    );
    kiro_event(&mut events, finished());
    let observation = &events.snapshot()["trajectory"];
    assert_eq!(observation["source"], json!("kiro_session_updates"));
    assert_eq!(observation["coverage"], json!("partial"));
    assert_eq!(observation["tool_calls"], json!(1));
    assert_eq!(observation["completed_tool_calls"], json!(1));
    assert_eq!(observation["tool_errors"], json!(0));
    assert_eq!(observation["steps"], serde_json::Value::Null);
    assert_eq!(observation["repeated_tool_calls"], serde_json::Value::Null);
    assert!(events.snapshot()["usage"]["input"].is_null());
    assert!(events.snapshot()["cost"]["usd"].is_null());
}

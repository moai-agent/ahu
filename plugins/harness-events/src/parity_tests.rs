//! Native parser regressions relocated from the core integration suite.
#[test]
fn protocol_parser_rejects_uppercase_error_duplicate_and_session_drift() {
    let mut agy = crate::events::Events::default();
    agy.observe(
        "antigravity",
        br#"{"type":"result","status":"ERROR","response":"quota exhausted"}"#,
    );
    assert!(agy.failed && agy.terminal);
    let mut claude = crate::events::Events::default();
    claude.observe("claude-code", br#"{"type":"system","session_id":"first"}"#);
    claude.observe(
        "claude-code",
        br#"{"type":"result","session_id":"second","result":"ok"}"#,
    );
    assert!(claude.failed);
    let mut codex = crate::events::Events::default();
    codex.observe("codex", br#"{"type":"turn.completed"}"#);
    codex.observe("codex", br#"{"type":"turn.completed"}"#);
    assert!(codex.failed);
}

#[test]
fn protocol_coverage_matrix_normalizes_common_lifecycle_and_usage_fields() {
    use crate::events::Events;
    use serde_json::json;

    let fixtures = [
        (
            "codex",
            vec![
                json!({"type":"thread.started","thread_id":"thr-codex"}),
                json!({"type":"item.completed","item":{"type":"function_call","name":"skill","input":{"skill":"ahu-direct-agents"}}}),
                json!({"type":"item.completed","item":{"type":"agent_message","text":"done"}}),
                json!({"type":"turn.completed","usage":{"input_tokens":11,"output_tokens":7,"total_tokens":18},"model":"gpt-test"}),
            ],
        ),
        (
            "claude-code",
            vec![
                json!({"type":"system","session_id":"ses-claude","subtype":"init"}),
                json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"Skill","input":{"skill":"ahu-direct-agents"}}]}}),
                json!({"type":"assistant","message":{"content":[]}}),
                json!({"type":"result","subtype":"success","is_error":false,"result":"done","usage":{"input_tokens":13,"output_tokens":5,"total_tokens":18},"model":"claude-test"}),
            ],
        ),
        (
            "antigravity",
            vec![
                json!({"type":"init","session_id":"ses-agy"}),
                json!({"type":"tool_use","name":"Skill","input":{"skill":"ahu-direct-agents"}}),
                json!({"type":"result","status":"success","response":"done","usage":{"prompt_tokens":17,"completion_tokens":4,"total_tokens":21},"model_name":"gemini-test"}),
            ],
        ),
        (
            "opencode",
            vec![
                json!({"type":"step_start","sessionID":"ses-opencode","part":{"type":"step-start"}}),
                json!({"type":"tool_use","sessionID":"ses-opencode","part":{"type":"tool","tool":"skill","state":{"input":{"skill":"ahu-direct-agents"}}}}),
                json!({"type":"text","sessionID":"ses-opencode","part":{"type":"text","text":"done"}}),
                json!({"type":"step_finish","sessionID":"ses-opencode","part":{"type":"step-finish","reason":"stop","usage":{"input_tokens":19,"output_tokens":6,"total_tokens":25},"model":"glm-test"}}),
            ],
        ),
    ];

    for (harness, fixture) in fixtures {
        let mut events = Events::default();
        for event in fixture {
            events.observe(harness, serde_json::to_string(&event).unwrap().as_bytes());
        }
        assert!(
            events.terminal,
            "{harness} did not produce a terminal result"
        );
        assert!(
            !events.failed,
            "{harness} produced blockers: {:?}",
            events.blockers
        );
        assert_eq!(
            events.unknown_events, 0,
            "{harness} fixture was not fully recognized"
        );
        assert!(
            events.session.is_some(),
            "{harness} session was not captured"
        );
        assert!(
            events.usage.total.is_some(),
            "{harness} total usage was not captured"
        );
        assert!(
            events.model.is_some(),
            "{harness} resolved model was not captured"
        );
        assert_eq!(
            events.skills.len(),
            1,
            "{harness} skill invocation was not captured"
        );
        assert_eq!(events.skills[0].name, "ahu-direct-agents");
    }
}

#[test]
fn antigravity_stream_json_shape_is_normalized() {
    use crate::events::Events;

    let mut events = Events::default();
    events.observe(
        "antigravity",
        br#"{"event":"init","conversation_id":"agy-session","init":{"cwd":"/tmp/probe"}}"#,
    );
    events.observe(
        "antigravity",
        br#"{"event":"step_update","step_update":{"conversation_id":"agy-session","state":"DONE","step_type":"agent_response","usage":{"input_tokens":11,"output_tokens":4,"thinking_tokens":2,"total_tokens":15}}}"#,
    );
    events.observe(
        "antigravity",
        br#"{"event":"result","result":{"conversation_id":"agy-session","status":"SUCCESS","response":"PROBE_SKILL_OK","usage":{"input_tokens":11,"output_tokens":4,"thinking_tokens":2,"total_tokens":15}}}"#,
    );
    assert!(events.terminal);
    assert!(!events.failed, "{:?}", events.blockers);
    assert_eq!(events.session.as_deref(), Some("agy-session"));
    assert_eq!(events.usage.total, Some(15));
    assert_eq!(events.usage.reasoning, Some(2));
    assert_eq!(events.summary, "PROBE_SKILL_OK");
}

#[test]
fn skill_probe_response_text_is_not_an_invocation_event() {
    use serde_json::json;

    for (harness, event) in [
        (
            "codex",
            json!({"type":"item.completed","item":{"type":"agent_message","text":"PROBE_SKILL_OK"}}),
        ),
        (
            "opencode",
            json!({"type":"text","part":{"type":"text","text":"PROBE_SKILL_OK"}}),
        ),
        (
            "claude-code",
            json!({"type":"result","subtype":"success","result":"PROBE_SKILL_OK"}),
        ),
        (
            "antigravity",
            json!({"event":"result","result":{"status":"SUCCESS","response":"PROBE_SKILL_OK"}}),
        ),
    ] {
        let mut events = crate::events::Events::default();
        events.observe(harness, &serde_json::to_vec(&event).unwrap());
        assert!(
            events.skills.is_empty(),
            "{harness} inferred skill use from text"
        );
        let metadata = serde_json::to_string(&events).unwrap();
        assert!(!metadata.contains("PROBE_SKILL_OK"));
    }
}

#[test]
fn skill_probe_antigravity_nested_tool_event_records_only_invocation_metadata() {
    let mut events = crate::events::Events::default();
    // Synthetic protocol fixture, not evidence of live project discovery.
    events.observe("antigravity", br#"{"event":"step_update","step_update":{"tool_name":"Skill","tool_info":{"parameters":{"skill":"probe-skill","prompt":"synthetic-private-input"}}}}"#);
    assert_eq!(events.skills.len(), 1);
    assert_eq!(events.skills[0].name, "probe-skill");
    assert_eq!(events.skills[0].status, "invoked");
    assert!(events.skills[0].source.is_none());
    assert!(events.skills[0].digest.is_none());
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("synthetic-private-input")
    );
}

#[test]
fn incomplete_terminal_objects_are_protocol_failures() {
    for harness in ["claude-code", "antigravity"] {
        for event in [
            br#"{"type":"result"}"#.as_slice(),
            br#"{"type":"result","status":true,"result":17}"#,
        ] {
            let mut events = crate::events::Events::default();
            events.observe(harness, event);
            assert!(events.failed && events.terminal, "{harness}");
        }
    }
}
#[test]
fn the_opencode_event_stream_is_terminal_only_when_a_step_stops() {
    use crate::events::Events;

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

    // `stop` closes an assistant message/step in OpenCode, not necessarily
    // the whole run. A tool result can be followed by another assistant
    // message that also ends with `stop` before the final response.
    let intermediate_stop = format!(
        r#"{{"type":"step_finish","timestamp":4,"sessionID":"{SESSION}","part":{{"type":"step-finish","reason":"stop"}}}}"#
    );
    let later_tool_step = format!(
        r#"{{"type":"step_finish","timestamp":5,"sessionID":"{SESSION}","part":{{"type":"step-finish","reason":"tool-calls"}}}}"#
    );
    let final_text = format!(
        r#"{{"type":"text","timestamp":6,"sessionID":"{SESSION}","part":{{"type":"text","text":"the actual final answer"}}}}"#
    );
    let final_stop = format!(
        r#"{{"type":"step_finish","timestamp":7,"sessionID":"{SESSION}","part":{{"type":"step-finish","reason":"stop"}}}}"#
    );
    let multi_message = observe(&[
        &step_start,
        &intermediate_stop,
        &later_tool_step,
        &final_text,
        &final_stop,
    ]);
    assert!(multi_message.terminal);
    assert!(!multi_message.failed, "{:?}", multi_message.blockers);
    assert!(
        !multi_message
            .blockers
            .iter()
            .any(|b| b.contains("multiple terminal events")),
        "OpenCode stop boundaries are not duplicate task results"
    );
    assert_eq!(multi_message.summary, "the actual final answer");

    // An earlier stop must not survive a later step that ends at a tool-call
    // boundary; otherwise a zero exit could turn an incomplete run into a pass.
    let unfinished_after_stop = observe(&[&stop, &step_start, &tool_step]);
    assert!(
        !unfinished_after_stop.terminal,
        "a later tool-call step supersedes an earlier stop: {unfinished_after_stop:?}"
    );

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
        capped
            .blockers
            .iter()
            .any(|b| b.contains("without a stop reason")),
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

/// A parseable OpenCode version newer than the catalog evidence floats by
/// default and is recorded by the headless task.
#[test]
fn excessive_native_counters_are_rejected_before_helper_accounting() {
    let mut events = crate::events::Events::default();
    events.observe("claude-code", br#"{"type":"result","subtype":"success","is_error":false,"result":"synthetic","subagent_stats":{"refused":{"depth_limit":18446744073709551615,"concurrency_limit":1,"budget":1}}}"#);
    assert!(events.failed);
    assert!(
        events
            .blockers
            .iter()
            .any(|b| b.contains("evaluation bound"))
    );
}

#[test]
fn review_regression_ordinary_payloads_do_not_use_helper_bounds() {
    use crate::events::Events;
    use serde_json::json;
    for (harness, payload, terminal) in [
        (
            "codex",
            json!({"type":"item.completed","item":{"type":"command_execution","aggregated_output":"x".repeat(4097)}}),
            json!({"type":"turn.completed"}),
        ),
        (
            "codex",
            json!({"type":"item.completed","item":{"type":"function_call","arguments":"x".repeat(4097)}}),
            json!({"type":"turn.completed"}),
        ),
        (
            "claude-code",
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","input":{"command":"x".repeat(4097)}}]},"unused":"x".repeat(4097)}),
            json!({"type":"result","subtype":"success","is_error":false,"result":"done"}),
        ),
    ] {
        let mut events = Events::default();
        events.observe(harness, &serde_json::to_vec(&payload).unwrap());
        events.observe(harness, &serde_json::to_vec(&terminal).unwrap());
        assert!(events.terminal && !events.failed, "{harness}: {events:?}");
    }
}

#[test]
fn review_regression_helper_identifiers_still_bounded() {
    let mut events = crate::events::Events::default();
    events.observe("claude-code", &serde_json::to_vec(&serde_json::json!({
        "type":"system","subtype":"task_started","task_id":"x".repeat(4097),"subagent_type":"reader"
    })).unwrap());
    assert!(events.failed);
}
#[test]
fn ordinary_event_subtypes_do_not_consume_helper_budget() {
    let mut events = crate::events::Events::default();
    for _ in 0..300 {
        events.observe("codex", br#"{"type":"item.completed","subtype":"task_payload","item":{"type":"command_execution"}}"#);
    }
    events.observe("codex", br#"{"type":"turn.completed"}"#);
    assert!(events.terminal && !events.failed);
}

#[test]
fn skill_probe_normalized_records_are_bounded_and_harness_scoped() {
    use crate::events::Events;
    use serde_json::json;
    let fixtures = [
        (
            "codex",
            json!({"type":"item.completed","item":{"type":"function_call","name":"skill","input":{"skill":"plugin:probe-skill","args":"PRIVATE INPUT"}}}),
        ),
        (
            "claude-code",
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"Skill","input":{"skill":"plugin:probe-skill","args":"PRIVATE INPUT"}}]}}),
        ),
        (
            "opencode",
            json!({"type":"tool_use","part":{"type":"tool","tool":"skill","state":{"input":{"name":"plugin:probe-skill","args":"PRIVATE INPUT"}}}}),
        ),
        (
            "antigravity",
            json!({"event":"step_update","step_update":{"tool_name":"Skill","tool_info":{"name":"Skill","parameters":{"skill":"plugin:probe-skill","args":"PRIVATE INPUT"}}}}),
        ),
    ];
    for (harness, fixture) in &fixtures {
        let mut events = Events::default();
        events.observe(harness, &serde_json::to_vec(fixture).unwrap());
        let value = serde_json::to_value(&events).unwrap();
        assert_eq!(value["skill_observation"], "observed");
        assert_eq!(events.skills.len(), 1, "{harness}");
        let record = &value["skills"][0];
        assert_eq!(record["harness"], *harness);
        assert_eq!(record["evidence"], "observed");
        assert_eq!(record["execution"], "unverified");
        // The decoder has no wall-clock input; it must not invent an absolute timestamp.
        assert!(record["observed_at"].is_null());
        assert!(record.get("source").is_none());
        assert!(!value.to_string().contains("PRIVATE INPUT"));
        for (other, _) in &fixtures {
            if other != harness {
                let mut wrong = Events::default();
                wrong.observe(other, &serde_json::to_vec(fixture).unwrap());
                assert!(wrong.skills.is_empty(), "{harness} accepted as {other}");
            }
        }
        let input_path = match *harness {
            "codex" => "/item/input",
            "claude-code" => "/message/content/0/input",
            "opencode" => "/part/state/input",
            _ => "/step_update/tool_info/parameters",
        };
        for input in [
            json!({"skill": "secret prompt text"}),
            json!({"skill": "x".repeat(129)}),
            json!({"skill_name":"probe-skill"}),
            json!("{\"skill\":\"probe-skill\"}"),
        ] {
            let mut invalid = fixture.clone();
            *invalid.pointer_mut(input_path).unwrap() = input;
            let mut events = Events::default();
            events.observe(harness, &serde_json::to_vec(&invalid).unwrap());
            assert!(events.skills.is_empty(), "{harness}");
            assert_eq!(
                serde_json::to_value(events).unwrap()["skill_observation"],
                "unverified"
            );
        }
        let mut drift = fixture.clone();
        drift["type"] = json!("future-event");
        drift["event"] = json!("future-event");
        let mut events = Events::default();
        events.observe(harness, &serde_json::to_vec(&drift).unwrap());
        assert!(events.skills.is_empty());
        assert_eq!(
            serde_json::to_value(&events).unwrap()["skill_observation"],
            "unverified"
        );
        assert_eq!(events.skill_unknown_events, 1, "{harness}");
    }
}

#[test]
fn skill_probe_rejects_payloads_and_bounds_records() {
    use crate::events::Events;
    use serde_json::json;
    for name in [
        json!(""),
        json!("x".repeat(129)),
        json!("/tmp/secret"),
        json!("prompt with spaces"),
        json!("line\nbreak"),
        json!({"credential":"secret"}),
        json!(null),
    ] {
        let mut events = Events::default();
        let event = json!({"type":"tool_use","name":"Skill","input":{"skill":name}});
        events.observe("antigravity", &serde_json::to_vec(&event).unwrap());
        assert!(events.skills.is_empty());
        assert_eq!(
            serde_json::to_value(events).unwrap()["skill_observation"],
            "unverified"
        );
    }
    for event in [
        json!({"type":"result","name":"Skill","input":{"skill":"probe-skill"}}),
        json!({"type":"tool_use","name":"mcp.skill","input":{"skill":"probe-skill"}}),
        json!({"type":"assistant","message":{"content":[{"type":"text","name":"Skill","input":{"skill":"probe-skill"}}]}}),
        json!({"type":"system","skills":["probe-skill"]}),
    ] {
        for harness in [
            "codex",
            "claude-code",
            "opencode",
            "antigravity",
            "future-harness",
        ] {
            let mut events = Events::default();
            events.observe(harness, &serde_json::to_vec(&event).unwrap());
            assert!(events.skills.is_empty());
        }
    }
    let mut events = Events::default();
    for _ in 0..140 {
        events.observe(
            "antigravity",
            br#"{"type":"tool_use","name":"Skill","input":{"skill":"probe-skill"}}"#,
        );
    }
    assert_eq!(events.skills.len(), 128);
    assert!(events.failed);
    assert_eq!(
        events
            .blockers
            .iter()
            .filter(|b| *b == "skill invocation limit exceeded")
            .count(),
        1
    );
    let legacy: crate::events::SkillInvocation =
        serde_json::from_value(json!({"name":"probe-skill","status":"invoked"})).unwrap();
    assert_eq!(
        serde_json::to_value(legacy).unwrap()["evidence"],
        "unverified"
    );
}

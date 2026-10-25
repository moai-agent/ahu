use super::*;
const SESSION: &str = "ses_synthetic123";
const STARTED: u64 = 1000;
fn export(messages: serde_json::Value) -> String {
    serde_json::json!({
        "info": {
            "id": SESSION,
            "projectID": "prj_synthetic",
            "outcome": "succeeded",
            "cost": 0,
            "tokens": {"input": 12, "output": 4, "reasoning": 0,
                       "cache": {"read": 0, "write": 0}},
            "time": {"created": STARTED + 10, "updated": STARTED + 900,
                     "idle": STARTED + 900},
            "title": format!("[redacted:session-title:{SESSION}]"),
            "location": {"directory": format!("/[redacted:session-directory:{SESSION}]")},
        },
        "messages": messages,
    })
    .to_string()
}
fn prompt() -> serde_json::Value {
    serde_json::json!({"id":"msg_prompt","type":"user","time":{"created":STARTED + 11},
                           "text":"[redacted:text:msg_prompt]"})
}
fn assistant(id: &str, finish: &str, tool_status: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "type": "assistant", "agent": "build",
        "model": {"providerID": "ollama", "modelID": "glm-5.3:cloud"},
        "content": [
            {"type":"tool","id":"prt_tool","tool":"write","callID":"call_synthetic",
             "state":{"status":tool_status,"input":{"redacted":format!("tool-input:{id}")}}},
            {"type":"text","id":"prt_text","text":format!("[redacted:text:{id}]")},
        ],
        "finish": finish,
        "time": {"created": STARTED + 12, "completed": STARTED + 800},
    })
}
fn idle(outcome: &str) -> serde_json::Value {
    serde_json::json!({"id":"msg_idle","type":"idle","outcome":outcome,
                           "time":{"created":STARTED + 900}})
}
fn accepted() -> serde_json::Value {
    serde_json::json!([
        prompt(),
        assistant("msg_turn", "stop", "completed"),
        idle("succeeded")
    ])
}
#[test]
fn a_sanitized_export_establishes_completion_only_for_this_attempts_finished_turn() {
    export_establishes_completion(&export(accepted()), SESSION, STARTED).unwrap();

    // Several steps: only the last one has to carry the stop.
    let stepped = serde_json::json!([
        prompt(),
        assistant("msg_step", "tool-calls", "completed"),
        {"id":"msg_compaction","type":"compaction","status":"completed",
         "time":{"created":STARTED + 850}},
        assistant("msg_turn", "stop", "completed"),
        idle("succeeded"),
    ]);
    export_establishes_completion(&export(stepped), SESSION, STARTED).unwrap();

    let rejected: [(&str, String, &str); 14] = [
        (
            "a different session",
            export(accepted()).replace(SESSION, "ses_someoneElsesSession0001"),
            "different session",
        ),
        (
            "an export of a session older than this attempt",
            export(accepted()),
            "predates this attempt",
        ),
        (
            "a stop that is not the session's last word",
            serde_json::to_string(&serde_json::json!({
                "info": serde_json::from_str::<serde_json::Value>(&export(accepted()))
                    .unwrap()["info"],
                "messages": [prompt(), assistant("msg_turn", "stop", "completed"),
                             idle("succeeded"), assistant("msg_after", "stop", "completed")],
            }))
            .unwrap(),
            "idle before its last message",
        ),
        (
            "a later tool call after the final stop",
            export(serde_json::json!([
                prompt(),
                assistant("msg_turn", "stop", "completed"),
                assistant("msg_after", "tool-calls", "completed"),
                idle("succeeded"),
            ])),
            "did not finish with stop",
        ),
        (
            "a tool call that did not complete",
            export(serde_json::json!([
                prompt(),
                assistant("msg_turn", "stop", "running"),
                idle("succeeded"),
            ])),
            "tool call did not complete",
        ),
        (
            "a refused tool call, which the projection records as an error",
            export(serde_json::json!([
                prompt(),
                assistant("msg_turn", "stop", "error"),
                idle("succeeded"),
            ])),
            "tool call did not complete",
        ),
        (
            "an assistant error",
            export(serde_json::json!([
                prompt(),
                {
                    "id":"msg_turn","type":"assistant","agent":"build",
                    "model":{"providerID":"ollama","modelID":"glm-5.3:cloud"},
                    "content":[], "finish":"stop",
                    "error":{"name":"ProviderError"},
                    "time":{"created":STARTED + 12,"completed":STARTED + 800},
                },
                idle("succeeded"),
            ])),
            "recorded an error",
        ),
        (
            "an interrupted run",
            export(serde_json::json!([
                prompt(),
                assistant("msg_turn", "stop", "completed"),
                idle("interrupted"),
            ])),
            "did not record success",
        ),
        (
            "a session carrying an earlier turn",
            export(serde_json::json!([
                prompt(),
                assistant("msg_earlier", "stop", "completed"),
                prompt(),
                assistant("msg_turn", "stop", "completed"),
                idle("succeeded"),
            ])),
            "exactly this attempt's one turn",
        ),
        (
            "a session that never recorded an execution outcome",
            export(serde_json::json!([
                prompt(),
                assistant("msg_turn", "stop", "completed"),
                {"id":"msg_system","type":"system","text":"[redacted:system:msg_system]",
                 "time":{"created":STARTED + 901}},
            ])),
            "does not end with an idle message",
        ),
        (
            "a session with no assistant message at all",
            export(serde_json::json!([prompt(), idle("succeeded")])),
            "no assistant message",
        ),
        (
            "a message type this check cannot describe",
            export(serde_json::json!([
                prompt(),
                {"id":"msg_shell","type":"shell","shellID":"shl_1","status":"completed",
                 "command":"[redacted:shell-command:msg_shell]",
                 "time":{"created":STARTED + 13}},
                assistant("msg_turn", "stop", "completed"),
                idle("succeeded"),
            ])),
            "does not recognize",
        ),
        ("malformed output", "{\"info\": ".to_string(), "not JSON"),
        (
            "output that is JSON but not an export",
            serde_json::json!({"data": {"info": {"id": SESSION}}}).to_string(),
            "no session info",
        ),
    ];
    for (case, body, expected) in rejected {
        // The stale-session case is the only one that reuses the accepted
        // body; it is rejected by the attempt window rather than by shape.
        let started = if expected == "predates this attempt" {
            STARTED + 1_000_000
        } else {
            STARTED
        };
        let error = export_establishes_completion(&body, SESSION, started).expect_err(case);
        assert!(error.contains(expected), "{case}: {error}");
    }
}
#[test]
fn an_export_is_matched_only_against_a_well_formed_opencode_session_id() {
    assert!(valid_session_id(SESSION));
    for rejected in [
        "",
        "ses_",
        "ses_short",
        "msg_f5cc153b6ffeNq1dwHoZGMECKw",
        "ses_f5cc153b6ffe;rm -rf /",
        "ses_f5cc153b6ffe\nses_other",
    ] {
        if rejected == "ses_short" {
            // Long enough and well shaped: the bound is on length, not on
            // this adapter guessing OpenCode's id body.
            assert!(valid_session_id(rejected));
            continue;
        }
        assert!(!valid_session_id(rejected), "{rejected:?}");
    }
    assert!(!valid_session_id(&format!("ses_{}", "a".repeat(200))));
    let error = export_establishes_completion(&export(accepted()), "not-a-session", STARTED)
        .expect_err("a malformed identifier cannot be matched");
    assert!(error.contains("not an OpenCode session id"), "{error}");
}
#[test]
fn an_oversized_export_is_bounded_before_its_shape_is_trusted() {
    let mut messages = vec![prompt()];
    for index in 0..5000 {
        messages.push(assistant(
            &format!("msg_{index}"),
            "tool-calls",
            "completed",
        ));
    }
    messages.push(assistant("msg_turn", "stop", "completed"));
    messages.push(idle("succeeded"));
    let error = export_establishes_completion(
        &export(serde_json::Value::Array(messages)),
        SESSION,
        STARTED,
    )
    .expect_err("a projection past the inspection bound is not evidence");
    assert!(error.contains("inspection bound"), "{error}");
}

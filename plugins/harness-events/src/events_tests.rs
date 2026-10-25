use super::*;
use crate::writes::collect_write_paths;
#[test]
fn reported_cost_is_normalized_without_claiming_billing() {
    let mut claude = Events::default();
    claude.observe(
            "claude-code",
            br#"{"type":"result","subtype":"success","total_cost_usd":0.0125,"usage":{"input_tokens":10}}"#,
        );
    assert_eq!(claude.cost.usd, Some(0.0125));
    assert_eq!(
        claude.cost.source.as_deref(),
        Some("claude_code_result_total")
    );

    let mut opencode = Events::default();
    let step =
        br#"{"type":"step_finish","part":{"id":"step-1","cost":0.003,"reason":"tool-calls"}}"#;
    opencode.observe("opencode", step);
    opencode.observe("opencode", step);
    opencode.observe(
        "opencode",
        br#"{"type":"step_finish","part":{"id":"step-2","cost":0.002,"reason":"stop"}}"#,
    );
    assert_eq!(opencode.cost.usd, Some(0.005));
    assert_eq!(
        opencode.cost.source.as_deref(),
        Some("opencode_step_finish_sum")
    );

    let mut unsupported = Events::default();
    unsupported.observe(
        "codex",
        br#"{"type":"turn.completed","usage":{"input_tokens":10},"total_cost_usd":99}"#,
    );
    assert_eq!(unsupported.cost.usd, None);
    unsupported.observe(
        "antigravity",
        br#"{"type":"result","status":"success","response":"ok","total_cost_usd":99}"#,
    );
    assert_eq!(unsupported.cost.usd, None);

    let mut invalid = Events::default();
    invalid.observe("claude-code", br#"{"type":"result","total_cost_usd":-1}"#);
    assert_eq!(invalid.cost.usd, None);

    let encoded = serde_json::to_value(&opencode.cost).unwrap();
    assert_eq!(encoded["usd"], 0.005);
    assert_eq!(encoded["source"], "opencode_step_finish_sum");
}
#[test]
fn write_path_discovery_handles_normalized_keys_nested_tools_and_embedded_json() {
    let event = serde_json::json!({
        "outer": [{
            "tool_name": "WriteFile",
            "arguments": r#"{"file_path":"/tmp/project/../outside/new.txt"}"#
        }],
        "tool": "Read",
        "path": "/tmp/not-a-write.txt"
    });
    let paths = collect_write_paths(&event);
    assert!(
        paths
            .iter()
            .any(|path| path == "/tmp/project/../outside/new.txt")
    );
    assert!(!paths.iter().any(|path| path == "/tmp/not-a-write.txt"));
}
#[test]
fn native_metadata_is_projected_to_only_supported_helper_fields() {
    let progress = native_metadata(
        "claude-code",
        &serde_json::json!({
            "type":"system", "subtype":"task_progress", "task_id":"child",
            "usage":{"total_tokens":12}, "secret":"ignore"
        }),
    );
    assert_eq!(progress["task_id"], "child");
    assert_eq!(progress["usage"]["total_tokens"], 12);
    assert!(progress.get("secret").is_none());

    let result = native_metadata(
        "claude-code",
        &serde_json::json!({
            "type":"result", "subagent_stats":{
                "spawned":2, "completed":1, "max_depth":3,
                "refused":{"budget":4, "secret":9},
                "by_type":{"worker":1, "invalid":"x"}
            }, "huge_payload":"ignored"
        }),
    );
    assert_eq!(result["subagent_stats"]["spawned"], 2);
    assert_eq!(result["subagent_stats"]["refused"]["budget"], 4);
    assert!(result["subagent_stats"]["refused"].get("secret").is_none());
    assert!(result.get("huge_payload").is_none());

    assert_eq!(
        native_metadata("codex", &serde_json::json!({"type":"item.started"})),
        serde_json::json!({})
    );
    assert_eq!(
        native_metadata("codex", &serde_json::json!({"type":"collab.started"})),
        serde_json::json!({"type":"collab"})
    );
}
#[test]
fn native_metadata_bounds_and_session_validation_fail_closed() {
    assert!(bounded_native_metadata(
        &serde_json::json!({"a": [1, true, null]})
    ));
    assert!(!bounded_native_metadata(&serde_json::json!(
        "x".repeat(4097)
    )));
    assert!(!bounded_native_metadata(&serde_json::json!(vec![0; 257])));
    assert!(!bounded_native_metadata(&serde_json::json!(u64::MAX)));
    assert!(valid_session("session-1"));
    assert!(!valid_session(" \n"));
    assert!(!valid_session("bad\nlocator"));
    assert!(!valid_session(&"x".repeat(4097)));
}
#[test]
fn stderr_capture_keeps_only_categories_or_a_count() {
    let mut events = Events::default();
    for line in [b"permission denied".as_slice(), b"ordinary diagnostic", b""] {
        events.observe_stderr(line);
    }
    assert!(events.failed);
    assert_eq!(events.stderr_diagnostics, ["permission denial"]);
    assert_eq!(events.stderr_unclassified_lines, 1);
}
#[test]
fn stdin_progress_is_informational_but_nearby_diagnostics_are_not_suppressed() {
    let mut events = Events::default();
    events.observe_stderr(b"Reading prompt from stdin...");
    events.observe_stderr(b"Reading additional input from stdin...\n");
    assert_eq!(events.stderr_informational_lines, 2);
    assert_eq!(events.stderr_unclassified_lines, 0);
    assert!(!events.failed);
    events.observe_stderr(b"Reading prompt from stdin... warning");
    events.observe_stderr(b"Could not create otel exporter: failed to build OTLP metrics exporter");
    assert_eq!(events.stderr_unclassified_lines, 2);
    events.observe_stderr(b"Reading prompt from stdin... permission denied");
    assert!(events.failed);
    assert_eq!(events.stderr_diagnostics, ["permission denial"]);
}
#[test]
fn event_observation_enforces_helper_count_and_metadata_bounds() {
    let mut events = Events::default();
    let progress = br#"{"type":"system","subtype":"task_progress","task_id":"synthetic"}"#;
    for _ in 0..256 {
        events.observe("claude-code", progress);
    }
    assert!(!events.failed);
    assert_eq!(events.native_event_count, 256);
    events.observe("claude-code", progress);
    assert!(events.failed);
    assert_eq!(events.native_event_count, 257);
    assert_eq!(events.blockers, ["native helper evaluation limit exceeded"]);

    let mut events = Events::default();
    let oversized =
        json!({"type": "system", "subtype": "task_progress", "task_id": "x".repeat(4097)});
    events.observe("claude-code", &serde_json::to_vec(&oversized).unwrap());
    assert!(events.failed);
    assert_eq!(
        events.blockers,
        ["native metadata evaluation bound exceeded"]
    );
    assert!(events.native.helpers().is_empty());
}

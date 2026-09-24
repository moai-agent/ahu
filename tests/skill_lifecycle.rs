//! Synthetic protocol contracts, not live harness conformance evidence.
use ahu::headless::Events;
use serde_json::{Value, json};

fn observe(events: &mut Events, harness: &str, event: Value) {
    events.observe(harness, &serde_json::to_vec(&event).unwrap());
}

fn fixture(harness: &str, failed: bool) -> Vec<Value> {
    match harness {
        "codex" => vec![
            json!({"type":"item.completed","item":{"type":"function_call","name":"skill","call_id":"call-1","input":{"skill":"probe-skill","prompt":"PRIVATE"}}}),
            json!({"type":"item.completed","item":{"type":"function_call_output","call_id":"call-1","is_error":failed,"output":"PRIVATE"}}),
        ],
        "claude-code" => vec![
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","name":"Skill","id":"call-1","input":{"skill":"probe-skill","prompt":"PRIVATE"}}]}}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"call-1","is_error":failed,"content":"PRIVATE"}]}}),
        ],
        "antigravity" => vec![
            json!({"type":"tool_use","name":"Skill","id":"call-1","input":{"skill":"probe-skill","prompt":"PRIVATE"}}),
            json!({"type":"tool_result","tool_use_id":"call-1","is_error":failed,"content":"PRIVATE"}),
        ],
        "opencode" => vec![
            json!({"type":"tool_use","part":{"type":"tool","tool":"skill","callID":"call-1","state":{"status":if failed {"error"} else {"completed"},"input":{"name":"probe-skill","prompt":"PRIVATE"},"output":"PRIVATE"}}}),
        ],
        _ => unreachable!(),
    }
}

#[test]
fn skill_completion_matrix_retains_only_bounded_metadata() {
    for harness in ["codex", "claude-code", "opencode", "antigravity"] {
        for failed in [false, true] {
            let mut events = Events::default();
            for event in fixture(harness, failed) {
                observe(&mut events, harness, event);
            }
            let metadata = serde_json::to_value(&events).unwrap();
            let skill = &metadata["skills"][0];
            assert_eq!(
                skill["status"],
                if failed { "failed" } else { "completed" },
                "{harness}"
            );
            assert_eq!(skill["execution"], "observed");
            assert!(skill["completed_at"].is_string());
            // Only correlated, separate reports have a measured receipt interval.
            assert_eq!(skill["elapsed_ms"].is_u64(), harness != "opencode");
            assert_eq!(events.skills.len(), 1);
            assert_eq!(events.unknown_events, 0);
            assert!(events.skills[0].source.is_none());
            let serialized = metadata.to_string();
            assert!(!serialized.contains("PRIVATE"));
            assert!(!serialized.contains("call-1"));
        }
    }
}

#[test]
fn skill_completion_requires_explicit_status_and_unique_correlation() {
    for harness in ["codex", "claude-code", "antigravity"] {
        let reports = fixture(harness, false);
        let mut events = Events::default();
        observe(&mut events, harness, reports[1].clone()); // orphan
        assert!(events.skills.is_empty());
        observe(&mut events, harness, reports[0].clone());
        observe(&mut events, harness, reports[0].clone()); // ambiguous ID
        observe(&mut events, harness, reports[1].clone());
        assert!(events.skills.iter().all(|s| s.status == "invoked"));
        assert!(
            serde_json::to_value(&events).unwrap()["skill_unknown_events"]
                .as_u64()
                .unwrap()
                >= 2
        );
    }
    let mut events = Events::default();
    observe(
        &mut events,
        "opencode",
        json!({"type":"tool_use","part":{"type":"tool","tool":"skill","state":{"status":"future-status","input":{"name":"probe-skill"}}}}),
    );
    assert_eq!(events.skills[0].status, "invoked");
    assert_eq!(
        serde_json::to_value(&events).unwrap()["skill_unknown_events"],
        1
    );
}

#[test]
fn skill_snapshot_updates_and_conflicts_fail_closed() {
    for harness in ["opencode", "antigravity"] {
        let snapshot = |status: &str| {
            if harness == "opencode" {
                json!({"type":"tool_use","part":{"type":"tool","tool":"skill","callID":"call-1","state":{"status":status,"input":{"name":"probe-skill"}}}})
            } else {
                json!({"event":"step_update","step_update":{"tool_name":"Skill","state":status,"tool_info":{"id":"call-1","parameters":{"skill":"probe-skill"}}}})
            }
        };
        let (running, done, failed) = if harness == "opencode" {
            ("running", "completed", "error")
        } else {
            ("RUNNING", "DONE", "ERROR")
        };
        let mut events = Events::default();
        for status in [running, running, done, done] {
            observe(&mut events, harness, snapshot(status));
        }
        assert_eq!(events.skills.len(), 1);
        assert_eq!(events.skills[0].status, "completed");
        assert!(events.skills[0].elapsed_ms.is_some());
        observe(&mut events, harness, snapshot(failed));
        observe(&mut events, harness, snapshot(done));
        assert_eq!(events.skills[0].status, "invoked");
        assert!(events.skills[0].completed_at.is_none());
        assert!(events.skill_unknown_events >= 2);
    }
}

#[test]
fn skill_results_do_not_infer_success_or_restore_deserialized_correlation() {
    let reports = fixture("claude-code", false);
    for error in [Value::Null, json!("false"), json!(0)] {
        let mut events = Events::default();
        observe(&mut events, "claude-code", reports[0].clone());
        let mut result = reports[1].clone();
        result["message"]["content"][0]["is_error"] = error;
        observe(&mut events, "claude-code", result);
        assert_eq!(events.skills[0].status, "invoked");
        assert_eq!(events.skill_unknown_events, 1);
    }
    let mut events = Events::default();
    observe(&mut events, "claude-code", reports[0].clone());
    let mut saved = serde_json::to_value(events).unwrap();
    saved["summary"] = json!("");
    saved["native_observations"] = json!([]);
    let mut restored: Events = serde_json::from_value(saved).unwrap();
    observe(&mut restored, "claude-code", reports[1].clone());
    assert_eq!(restored.skills[0].status, "invoked");
    assert_eq!(restored.skill_unknown_events, 1);
}

#[test]
fn skill_separate_results_reject_conflicts_and_unbounded_ids() {
    for harness in ["codex", "claude-code", "antigravity"] {
        let good = fixture(harness, false);
        let bad = fixture(harness, true);
        let mut events = Events::default();
        for event in [&good[0], &good[1], &good[1]] {
            observe(&mut events, harness, event.clone());
        }
        assert_eq!(events.skills[0].status, "completed");
        assert_eq!(events.skill_unknown_events, 0);
        observe(&mut events, harness, bad[1].clone());
        observe(&mut events, harness, good[1].clone());
        assert_eq!(events.skills[0].status, "invoked");
        assert_eq!(
            events.skills[0].execution,
            ahu::telemetry::SkillEvidence::Unverified
        );
        assert!(events.skills[0].elapsed_ms.is_none());

        let (invocation_path, result_path) = match harness {
            "codex" => ("/item/call_id", "/item/call_id"),
            "claude-code" => ("/message/content/0/id", "/message/content/0/tool_use_id"),
            _ => ("/id", "/tool_use_id"),
        };
        for id in [json!("x".repeat(129)), json!("line\nbreak"), Value::Null] {
            let mut events = Events::default();
            let mut invocation = good[0].clone();
            let mut result = good[1].clone();
            *invocation.pointer_mut(invocation_path).unwrap() = id.clone();
            *result.pointer_mut(result_path).unwrap() = id;
            observe(&mut events, harness, invocation);
            observe(&mut events, harness, result);
            assert_eq!(events.skills[0].status, "invoked");
            assert_eq!(events.skill_unknown_events, 1);
        }
    }
}

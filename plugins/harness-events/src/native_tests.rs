use super::*;
use serde_json::json;
fn task_started(task_id: &str, role: &str, depth: u64, backgrounded: bool) -> Value {
    json!({
        "type": "system", "subtype": "task_started", "task_id": task_id,
        "tool_use_id": format!("toolu_{task_id}"),
        "description": "Read proof.txt",
        "subagent_type": role, "spawn_depth": depth, "is_backgrounded": backgrounded,
        "task_type": "local_agent"
    })
}
fn shell_task_started(task_id: &str, description: &str) -> Value {
    json!({
        "type": "system", "subtype": "task_started", "task_id": task_id,
        "tool_use_id": format!("toolu_{task_id}"),
        "description": description, "is_backgrounded": false,
        "task_type": "local_bash"
    })
}
fn shell_task_notification(task_id: &str, summary: &str) -> Value {
    json!({
        "type": "system", "subtype": "task_notification", "task_id": task_id,
        "tool_use_id": format!("toolu_{task_id}"),
        "status": "completed", "output_file": "", "summary": summary
    })
}
fn task_notification(task_id: &str, status: &str, summary: &str) -> Value {
    json!({
        "type": "system", "subtype": "task_notification", "task_id": task_id,
        "status": status, "summary": summary,
        "output_file": format!("/runtime/tasks/{task_id}.output"),
        "usage": {"total_tokens": 10_629}
    })
}
fn result(stats: Value) -> Value {
    json!({"type": "result", "subtype": "success", "is_error": false, "subagent_stats": stats})
}
fn stats(spawned: u64, completed: u64, max_depth: u64, refused: Value, by_type: Value) -> Value {
    json!({
        "spawned": spawned, "completed": completed, "failed": 0, "max_depth": max_depth,
        "spawned_by_subagents": 0, "started_in_background": 0,
        "killed": {"parent": 0, "user": 0, "system": 0},
        "refused": refused, "by_type": by_type
    })
}
fn feed(events: &[Value]) -> Observations {
    let mut observations = Observations::default();
    for event in events {
        observations.observe("claude-code", event);
    }
    observations
}
#[test]
fn a_joined_helper_is_complete_work() {
    let observations = feed(&[
        task_started("a38fe4f46", "ahu-reader", 1, false),
        json!({
            "type": "system", "subtype": "task_progress", "task_id": "a38fe4f46",
            "usage": {"total_tokens": 10_227}, "last_tool_name": "Read"
        }),
        task_notification("a38fe4f46", "completed", "proof.txt contains one line"),
        result(stats(
            1,
            1,
            1,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({"ahu-reader": 1}),
        )),
    ]);
    let helpers = observations.helpers();
    assert_eq!(helpers.len(), 1);
    assert_eq!(helpers[0].status, HelperStatus::Completed);
    assert_eq!(helpers[0].role, "ahu-reader");
    assert!(helpers[0].summary.is_empty());
    assert_eq!(helpers[0].total_tokens, Some(10_629));
    assert!(helpers[0].output_file.is_some());

    let completeness = observations.completeness();
    assert!(completeness.complete, "{completeness:?}");
    assert!(completeness.terminated_cleanly && completeness.evidence_complete);
    assert!(completeness.unobserved.is_empty());
    assert_eq!(completeness.joined, vec!["a38fe4f46".to_string()]);
    assert!(completeness.blockers().is_empty());
}
#[test]
fn a_helper_that_never_reported_blocks_the_attempt() {
    let observations = feed(&[
        task_started("first", "ahu-reader", 1, true),
        task_started("second", "ahu-reader", 1, true),
        task_notification("first", "completed", "done"),
        result(stats(
            2,
            1,
            1,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({"ahu-reader": 2}),
        )),
    ]);
    let completeness = observations.completeness();
    assert!(!completeness.complete);
    assert_eq!(completeness.unjoined, vec!["second".to_string()]);
    let blockers = completeness.blockers().join("\n");
    assert!(
        blockers.contains("second") && blockers.contains("partial"),
        "{blockers}"
    );
}
#[test]
fn a_stream_without_a_terminal_event_is_not_complete() {
    let observations = feed(&[
        task_started("only", "ahu-reader", 1, false),
        task_notification("only", "completed", "done"),
    ]);
    let completeness = observations.completeness();
    assert!(!completeness.complete);
    assert!(
        completeness.unknown.iter().any(|u| u.contains("terminal")),
        "{completeness:?}"
    );
    assert!(
        !completeness.terminated_cleanly,
        "the stream never ended: {completeness:?}"
    );
    assert!(
        completeness.evidence_complete,
        "everything that did happen was visible: {completeness:?}"
    );
    assert!(
        completeness.blockers().is_empty(),
        "an unknown is reported as an unknown, not as a failure: {completeness:?}"
    );
}
#[test]
fn refusals_at_the_harness_limits_are_reported() {
    let observations = feed(&[
        task_started("one", "ahu-reader", 1, false),
        task_notification("one", "completed", "done"),
        result(stats(
            1,
            1,
            1,
            json!({"depth_limit": 0, "concurrency_limit": 2, "budget": 0}),
            json!({"ahu-reader": 1}),
        )),
    ]);
    assert_eq!(observations.refusals().concurrency_limit, 2);
    assert_eq!(observations.refusals().total(), 2);
    let completeness = observations.completeness();
    assert!(
        completeness.unknown.iter().any(|u| u.contains("refused")),
        "{completeness:?}"
    );
    assert!(
        completeness.complete,
        "a refusal the harness handled does not make the joined work incomplete: {completeness:?}"
    );
}
#[test]
fn a_stopped_helper_is_reported_as_an_open_question() {
    let observations = feed(&[
        task_started("stopped", "ahu-reader", 1, false),
        task_notification("stopped", "killed", ""),
        result(stats(
            1,
            0,
            1,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({"ahu-reader": 1}),
        )),
    ]);
    assert_eq!(observations.helpers()[0].status, HelperStatus::Killed);
    let completeness = observations.completeness();
    assert!(
        completeness
            .unknown
            .iter()
            .any(|u| u.contains("provider-side")),
        "{completeness:?}"
    );
}
#[test]
fn helpers_without_events_hold_the_attempt_open() {
    let observations = feed(&[
        task_started("seen", "ahu-reader", 1, false),
        task_notification("seen", "completed", "done"),
        result(stats(
            3,
            3,
            1,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({"ahu-reader": 3}),
        )),
    ]);
    let completeness = observations.completeness();
    assert!(
        completeness
            .unobserved
            .iter()
            .any(|u| u.contains('3') && u.contains("cannot be joined")),
        "the shortfall must be recorded as unobserved work: {completeness:?}"
    );
    assert!(
        completeness.terminated_cleanly,
        "every helper ahu saw did join, so the attempt did end tidily: {completeness:?}"
    );
    assert!(
        !completeness.evidence_complete,
        "two helpers ran that ahu never saw: {completeness:?}"
    );
    assert!(
        !completeness.complete,
        "an attempt resting on work ahu never saw is not complete: {completeness:?}"
    );
    assert!(
        completeness
            .blockers()
            .iter()
            .any(|b| b.contains("cannot be joined")),
        "unobserved helper work must reach the result envelope: {completeness:?}"
    );
}
#[test]
fn a_refusal_is_not_treated_as_unobserved_work() {
    let observations = feed(&[
        task_started("one", "ahu-reader", 1, false),
        task_notification("one", "completed", "done"),
        result(stats(
            1,
            1,
            1,
            json!({"depth_limit": 0, "concurrency_limit": 2, "budget": 1}),
            json!({"ahu-reader": 1}),
        )),
    ]);
    let completeness = observations.completeness();
    assert!(
        completeness.unobserved.is_empty(),
        "a refused helper never ran, so nothing about it is unobserved: {completeness:?}"
    );
    assert!(
        completeness.unknown.iter().any(|u| u.contains("refused")),
        "{completeness:?}"
    );
    assert!(
        completeness.complete && completeness.evidence_complete,
        "a refusal the harness accounted for does not hold the attempt open: {completeness:?}"
    );
    assert!(completeness.blockers().is_empty(), "{completeness:?}");
}
#[test]
fn an_unreadable_status_leaves_a_helper_unjoined_and_the_evidence_incomplete() {
    let observations = feed(&[
        task_started("odd", "ahu-reader", 1, false),
        task_notification("odd", "quiesced", "?"),
        result(stats(
            1,
            1,
            1,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({"ahu-reader": 1}),
        )),
    ]);
    let completeness = observations.completeness();
    assert_eq!(completeness.unjoined, vec!["odd".to_string()]);
    assert!(
        !completeness.terminated_cleanly,
        "an unrecognised status is not a terminal one: {completeness:?}"
    );
    assert!(!completeness.evidence_complete && !completeness.complete);
    assert!(
        completeness.unobserved.is_empty(),
        "ahu saw this helper start; its outcome is unjoined, not unobserved: {completeness:?}"
    );
}
#[test]
fn an_unrecognised_helper_status_is_counted_not_guessed() {
    let observations = feed(&[
        task_started("odd", "ahu-reader", 1, false),
        task_notification("odd", "quiesced", "?"),
        result(stats(
            1,
            0,
            1,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({"ahu-reader": 1}),
        )),
    ]);
    assert_eq!(observations.helpers()[0].status, HelperStatus::Unknown);
    assert_eq!(observations.unknown_events, 1);
    let completeness = observations.completeness();
    assert!(!completeness.complete);
    assert!(
        completeness
            .blockers()
            .iter()
            .any(|b| b.contains("could not be read")),
        "{completeness:?}"
    );
}
#[test]
fn progress_after_a_terminal_event_does_not_reopen_a_helper() {
    let observations = feed(&[
        task_started("late", "ahu-reader", 1, false),
        task_notification("late", "completed", "done"),
        json!({
            "type": "system", "subtype": "task_progress", "task_id": "late",
            "usage": {"total_tokens": 99}, "last_tool_name": "Read"
        }),
        result(stats(
            1,
            1,
            1,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({"ahu-reader": 1}),
        )),
    ]);
    assert_eq!(observations.helpers()[0].status, HelperStatus::Completed);
    assert!(observations.completeness().complete);
}
#[test]
fn ordinary_events_are_not_counted_as_unreadable() {
    let observations = feed(&[
        json!({"type": "system", "subtype": "init", "tools": ["Read"], "agents": ["Explore"]}),
        json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "hello"}]}}),
        json!({"type": "system", "subtype": "task_updated", "task_id": "x", "patch": {}}),
        json!({"type": "rate_limit_event", "rate_limit_info": {"status": "allowed"}}),
        result(stats(
            0,
            0,
            0,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({}),
        )),
    ]);
    assert_eq!(observations.unknown_events, 0);
    assert!(observations.helpers().is_empty());
    assert!(observations.completeness().complete);
}
#[test]
fn a_helper_event_without_a_task_id_is_counted() {
    let observations = feed(&[
        json!({"type": "system", "subtype": "task_started", "subagent_type": "ahu-reader"}),
        json!({"type": "system", "subtype": "task_notification", "status": "completed"}),
    ]);
    assert_eq!(observations.unknown_events, 2);
    assert!(observations.helpers().is_empty());
}
#[test]
fn the_roles_helpers_actually_ran_under_are_recorded() {
    let observations = feed(&[
        task_started("a", "general-purpose", 1, false),
        task_notification("a", "completed", "done"),
        result(stats(
            1,
            1,
            1,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({"general-purpose": 1}),
        )),
    ]);
    assert_eq!(observations.helpers()[0].role, "general-purpose");
    assert_eq!(
        observations.observed_roles().get("general-purpose"),
        Some(&1),
        "the observed roster is what the record carries, not the requested role"
    );
}
#[test]
fn unjoinable_helper_events_from_another_harness_are_counted() {
    let mut observations = Observations::default();
    observations.observe(
        "codex",
        &json!({
            "type": "item.completed",
            "item": {
                "id": "item_1", "type": "collab_tool_call", "tool": "wait",
                "receiver_thread_ids": [], "agents_states": {}, "status": "completed"
            }
        }),
    );
    observations.observe(
        "codex",
        &json!({"type": "item.completed", "item": {"type": "agent_message", "text": "done"}}),
    );
    assert_eq!(observations.unknown_events, 1);
    assert!(
        observations.helpers().is_empty(),
        "an event that names no helper must not invent one"
    );
}
#[test]
fn background_shell_tasks_are_not_native_helpers() {
    let observations = feed(&[
        shell_task_started(
            "b1yc7l2f0",
            "Launch registered reviewer child headless in background",
        ),
        shell_task_notification(
            "b1yc7l2f0",
            "Launch registered reviewer child headless in background",
        ),
        shell_task_started("bmm869562", "Wait for the registered child task"),
        shell_task_notification("bmm869562", "Wait for the registered child task"),
        result(stats(
            0,
            0,
            0,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({}),
        )),
    ]);
    assert!(
        observations.helpers().is_empty(),
        "a background shell task is the parent's own Bash tool, not a helper: {:?}",
        observations.helpers()
    );
    assert_eq!(
        observations.shell_tasks(),
        ["b1yc7l2f0".to_string(), "bmm869562".to_string()],
        "the attempt did run them, so the record should still say so"
    );
    assert_eq!(
        observations.unknown_events, 0,
        "the harness said what these were; nothing here is unclassifiable"
    );

    let completeness = observations.completeness();
    assert!(
        completeness.violations.is_empty(),
        "running a background command is not native delegation: {completeness:?}"
    );
    assert!(
        completeness.complete && completeness.blockers().is_empty(),
        "the attempt must not be failed by its own background shell work: {completeness:?}"
    );
}
#[test]
fn shell_tasks_do_not_register_as_unobserved_helper_work() {
    let observations = feed(&[
        shell_task_started(
            "b1yc7l2f0",
            "Launch registered reviewer child headless in background",
        ),
        shell_task_notification(
            "b1yc7l2f0",
            "Launch registered reviewer child headless in background",
        ),
        result(stats(
            0,
            0,
            0,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({}),
        )),
    ]);
    let completeness = observations.completeness();
    assert!(completeness.unobserved.is_empty(), "{completeness:?}");
    assert!(completeness.evidence_complete && completeness.complete);
}
#[test]
fn shell_and_helper_tasks_are_told_apart_on_one_stream() {
    let observations = feed(&[
        shell_task_started("shell1", "Launch registered child"),
        task_started("helper1", "ahu-reader", 1, false),
        shell_task_notification("shell1", "Launch registered child"),
        task_notification("helper1", "completed", "proof.txt contains one line"),
        result(stats(
            1,
            1,
            1,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({"ahu-reader": 1}),
        )),
    ]);
    assert_eq!(observations.helpers().len(), 1);
    assert_eq!(observations.helpers()[0].task_id, "helper1");
    assert_eq!(observations.helpers()[0].role, "ahu-reader");
    assert_eq!(observations.shell_tasks(), ["shell1".to_string()]);

    let completeness = observations.completeness();
    assert!(completeness.complete, "{completeness:?}");
    assert_eq!(completeness.joined, vec!["helper1".to_string()]);
}
#[test]
fn an_unfinished_shell_task_does_not_block_the_attempt() {
    let observations = feed(&[
        shell_task_started("shell1", "Tail a log"),
        result(stats(
            0,
            0,
            0,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({}),
        )),
    ]);
    let completeness = observations.completeness();
    assert!(completeness.unjoined.is_empty(), "{completeness:?}");
    assert!(completeness.complete, "{completeness:?}");
}
#[test]
fn an_unrecognised_agent_task_type_is_still_a_helper() {
    let observations = feed(&[
        json!({
            "type": "system", "subtype": "task_started", "task_id": "novel",
            "subagent_type": "some-future-role", "spawn_depth": 1,
            "is_backgrounded": false, "task_type": "remote_agent_v2"
        }),
        task_notification("novel", "completed", "done"),
        result(stats(
            1,
            1,
            1,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({"some-future-role": 1}),
        )),
    ]);
    assert_eq!(
        observations.helpers().len(),
        1,
        "{:?}",
        observations.helpers()
    );
    assert_eq!(observations.helpers()[0].role, "some-future-role");
    assert!(observations.shell_tasks().is_empty());

    let completeness = observations.completeness();
    assert!(completeness.complete);
    assert_eq!(completeness.joined, ["novel"]);
    // The host applies its frozen policy to the retained helper observations.
}
#[test]
fn a_task_that_cannot_be_placed_is_counted_not_assumed() {
    let observations = feed(&[
        json!({
            "type": "system", "subtype": "task_started", "task_id": "mystery",
            "description": "something new", "task_type": "local_something_else"
        }),
        json!({
            "type": "system", "subtype": "task_notification", "task_id": "mystery",
            "status": "completed", "summary": "done"
        }),
        result(stats(
            0,
            0,
            0,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({}),
        )),
    ]);
    assert!(observations.helpers().is_empty(), "nothing was established");
    assert!(
        observations.shell_tasks().is_empty(),
        "nor was it shown harmless"
    );
    assert_eq!(
        observations.unknown_events, 1,
        "counted once at its start, not again for each later event"
    );
    let completeness = observations.completeness();
    assert!(
        !completeness.evidence_complete && !completeness.complete,
        "an unplaceable task leaves the attempt's coverage incomplete: {completeness:?}"
    );
}
#[test]
fn a_notification_for_an_unseen_task_invents_no_helper() {
    let observations = feed(&[
        shell_task_notification("never-started", "Launch registered child"),
        result(stats(
            0,
            0,
            0,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({}),
        )),
    ]);
    assert!(observations.helpers().is_empty());
    assert_eq!(observations.unknown_events, 1);
    let completeness = observations.completeness();
    assert!(
        completeness.violations.is_empty(),
        "an unplaceable task is not evidence that a helper ran: {completeness:?}"
    );
    assert!(!completeness.evidence_complete, "{completeness:?}");
}

//! What ahu is allowed to claim about native helpers.
//!
//! Each test here corresponds to a boundary that was checked against the
//! installed Claude Code CLI. The point of the suite is not that the strings
//! match; it is that a boundary ahu could not actually impose never turns into
//! an enforced control, and that a helper whose outcome was never reported
//! never turns into finished work.
//!
//! The event fixtures are the real event shapes the CLI emits in
//! `--output-format stream-json`, reduced to the fields this module reads.

use ahu::native::{self, HelperStatus, Mechanism, Observations, Profile, Request};
use serde_json::{Value, json};

/// A bounded request whose fields a test can then vary one at a time.
fn bounded() -> Request<'static> {
    Request {
        harness: "claude-code",
        harness_version: "2.1.270",
        policy: native::BOUNDED,
        session_model: "claude-opus-5",
        helper_model: Some("claude-haiku-4-5-20251001"),
        helper_role: "ahu-reader",
        max_concurrent: 2,
        max_depth: 1,
        budget_usd: Some(4.0),
        assignment_writes: false,
    }
}

fn disabled() -> Request<'static> {
    Request {
        policy: native::DISABLED,
        ..bounded()
    }
}

fn profile_of(request: &Request<'_>) -> Profile {
    native::profile(request).expect("profile should be available")
}

/// The value of `--flag` in an argument vector, if the flag is present.
fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

fn env_of<'a>(profile: &'a Profile, name: &str) -> Option<&'a str> {
    profile
        .env
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn control<'a>(profile: &'a Profile, id: &str) -> Option<&'a native::Control> {
    profile.enforced.iter().find(|control| control.id == id)
}

// ---------------------------------------------------------------------------
// The policy gate
// ---------------------------------------------------------------------------

/// An unvalidated harness is refused rather than given a best-effort profile.
///
/// The refusal has to name the boundary, because the caller's only correct
/// response is to report it; a message that just said "unsupported" would invite
/// the caller to drop to a weaker policy and carry on.
#[test]
fn bounded_is_refused_for_harnesses_without_validated_controls() {
    for harness in ["codex", "antigravity", "something-else"] {
        let request = Request {
            harness,
            ..bounded()
        };
        let error = native::profile(&request).expect_err("bounded must be refused");
        let message = error.to_string();
        assert!(
            message.contains(harness),
            "refusal must name the harness: {message}"
        );
        assert!(
            message.contains("disabled"),
            "refusal must point at the policy that is available: {message}"
        );
    }
}

/// A version whose controls were never checked is refused by the same gate.
#[test]
fn bounded_is_refused_for_unvalidated_versions() {
    let request = Request {
        harness_version: "2.1.269",
        ..bounded()
    };
    let error = native::profile(&request).expect_err("bounded must be refused");
    let message = error.to_string();
    assert!(
        message.contains("2.1.269") && message.contains("2.1.270"),
        "refusal must name both the rejected and the validated version: {message}"
    );
}

/// Only depth 1 is enforceable, so only depth 1 is offered.
#[test]
fn bounded_is_refused_for_a_depth_that_cannot_be_enforced() {
    for depth in [0, 2, 3] {
        let request = Request {
            max_depth: depth,
            ..bounded()
        };
        let error = native::profile(&request).expect_err("only depth 1 is enforceable");
        assert!(
            error.to_string().contains("depth"),
            "refusal must name the depth boundary"
        );
    }
}

#[test]
fn an_unknown_policy_name_is_refused() {
    let request = Request {
        policy: "advisory",
        ..bounded()
    };
    let error = native::profile(&request).expect_err("unknown policies must be refused");
    assert!(error.to_string().contains("advisory"));
}

/// The ceiling binds the owning agent too, so a writing assignment is refused
/// rather than handed a session that silently cannot do its work.
#[test]
fn bounded_is_refused_for_an_assignment_that_needs_to_write() {
    let request = Request {
        assignment_writes: true,
        ..bounded()
    };
    let error = native::profile(&request).expect_err("a writing assignment must be refused");
    let message = error.to_string();
    assert!(
        message.contains("shell") || message.contains("commands"),
        "refusal must say what the assignment would lose: {message}"
    );
    assert!(
        message.contains("disabled"),
        "refusal must name the policy implementation work needs: {message}"
    );
}

// ---------------------------------------------------------------------------
// What the bounded profile actually imposes
// ---------------------------------------------------------------------------

/// The tool allowlist is the whole ceiling, so the tools that would let a
/// helper escape it must not be in the list.
///
/// This is one assertion per capability rather than an equality check against a
/// fixed list, so that adding a genuinely read-only tool later does not look
/// like a regression while adding a shell would.
#[test]
fn the_bounded_tool_ceiling_excludes_every_escape_route() {
    let profile = profile_of(&bounded());
    let tools = flag(&profile.args, "--tools").expect("--tools must be passed");
    let allowed: Vec<&str> = tools.split(',').collect();
    for forbidden in [
        "Bash",
        "Write",
        "Edit",
        "NotebookEdit",
        "EnterWorktree",
        "ExitWorktree",
        "Workflow",
        "SendMessage",
        "ListAgents",
        "WebFetch",
        "RemoteTrigger",
    ] {
        assert!(
            !allowed.contains(&forbidden),
            "{forbidden} must not be inside the bounded ceiling: {tools}"
        );
    }
    assert!(
        allowed.contains(&"Read") && allowed.contains(&"Task"),
        "a bounded session still reads files and delegates: {tools}"
    );
}

/// With no shell in the ceiling there is no route to cmux, and that is the only
/// reason ahu may say so.
#[test]
fn the_absence_of_a_shell_is_what_backs_the_cmux_claim() {
    let profile = profile_of(&bounded());
    let control = control(&profile, "native.helper.no_cmux").expect("the claim must be a control");
    assert_eq!(
        control.mechanism,
        Mechanism::Structural,
        "a cmux claim resting on anything but a missing tool would be a prompt-level claim"
    );
}

/// Depth, concurrency, and the helper model are environment controls, and the
/// values that reach the child are the ones the caller froze.
#[test]
fn depth_concurrency_and_helper_model_reach_the_child_as_set() {
    let profile = profile_of(&bounded());
    assert_eq!(
        env_of(&profile, "CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH"),
        Some("1")
    );
    assert_eq!(
        env_of(&profile, "CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"),
        Some("2")
    );
    assert_eq!(
        env_of(&profile, "CLAUDE_CODE_SUBAGENT_MODEL"),
        Some("claude-haiku-4-5-20251001")
    );
    assert_eq!(
        env_of(&profile, "CLAUDE_CODE_SUBAGENT_MODEL_FORCE"),
        Some("1"),
        "without the force flag the pin is a preference, not a control"
    );
    assert_eq!(profile.helper_model, "claude-haiku-4-5-20251001");
    assert_eq!(profile.max_depth, 1);
}

/// A helper with no model of its own runs on the owning agent's frozen model,
/// never on whatever the harness would have picked.
#[test]
fn a_helper_without_its_own_model_is_pinned_to_the_session_model() {
    let request = Request {
        helper_model: None,
        ..bounded()
    };
    let profile = profile_of(&request);
    assert_eq!(profile.helper_model, "claude-opus-5");
    assert_eq!(
        env_of(&profile, "CLAUDE_CODE_SUBAGENT_MODEL"),
        Some("claude-opus-5")
    );
}

/// Ambient native settings are removed, so the frozen policy is what runs even
/// when the launching environment had opinions of its own.
#[test]
fn ambient_native_settings_are_scrubbed_under_every_policy() {
    for request in [bounded(), disabled()] {
        let profile = profile_of(&request);
        for name in [
            "CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION",
            "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS",
            "CLAUDE_CODE_SUBAGENT_MODEL_FORCE",
            "CLAUDE_CODE_FORK_SUBAGENT",
        ] {
            assert!(
                profile.env_remove.iter().any(|key| key == name),
                "{name} must be scrubbed under the {} policy",
                profile.policy
            );
        }
    }
}

/// Every variable the profile sets is also one it scrubs first, so a value
/// inherited from the launching environment can never survive alongside it.
#[test]
fn every_variable_the_profile_sets_is_scrubbed_first() {
    let profile = profile_of(&bounded());
    for (key, _) in &profile.env {
        assert!(
            profile.env_remove.contains(key),
            "{key} is set but not scrubbed, so an inherited value could race it"
        );
    }
}

/// The repository's own settings stay in force. They cannot widen the ceiling,
/// and dropping them would take the repository's safety hooks and deny rules
/// with them.
#[test]
fn repository_settings_are_not_disabled_by_the_bounded_profile() {
    let profile = profile_of(&bounded());
    assert!(
        !profile.args.iter().any(|arg| arg == "--setting-sources"),
        "excluding settings would remove the repository's hooks and deny rules"
    );
    assert!(
        profile.args.iter().any(|arg| arg == "--strict-mcp-config"),
        "MCP servers are excluded by their own flag, not by dropping settings"
    );
    let control =
        control(&profile, "native.settings.retained").expect("retention must be disclosed");
    assert!(
        control.detail.contains("deny"),
        "retention is claimed for the constraint that was observed to take effect: {}",
        control.detail
    );
    assert!(
        !control.detail.contains("hook"),
        "hook execution was never demonstrated, so it must not be claimed as retained"
    );
    assert!(
        profile.gaps.iter().any(|gap| gap.contains("hook")),
        "the undemonstrated hook behaviour belongs in the gaps: {:?}",
        profile.gaps
    );
}

/// A budget is only claimed as a control when one was actually requested.
#[test]
fn the_spend_ceiling_is_claimed_only_when_it_is_set() {
    let with_budget = profile_of(&bounded());
    assert_eq!(flag(&with_budget.args, "--max-budget-usd"), Some("4"));
    assert!(control(&with_budget, "native.helper.budget").is_some());

    let without = profile_of(&Request {
        budget_usd: None,
        ..bounded()
    });
    assert!(
        flag(&without.args, "--max-budget-usd").is_none(),
        "no budget flag without a budget"
    );
    assert!(
        control(&without, "native.helper.budget").is_none(),
        "a spend ceiling that was never set is not an enforced control"
    );
}

#[test]
fn a_nonsensical_budget_is_refused() {
    for budget in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let request = Request {
            budget_usd: Some(budget),
            ..bounded()
        };
        assert!(
            native::profile(&request).is_err(),
            "a budget of {budget} must be refused"
        );
    }
}

/// The role name goes into a JSON object key, so the names accepted are the
/// ones that survive that without quoting questions.
#[test]
fn a_helper_role_name_must_be_a_plain_identifier() {
    for role in ["", "has space", "quote\"", "brace}", "dot.dot", "sla/sh"] {
        let request = Request {
            helper_role: role,
            ..bounded()
        };
        assert!(
            native::profile(&request).is_err(),
            "role {role:?} must be refused"
        );
    }
    for role in ["ahu-reader", "reader_2", "Reader"] {
        let request = Request {
            helper_role: role,
            ..bounded()
        };
        assert!(
            native::profile(&request).is_ok(),
            "role {role:?} must be accepted"
        );
    }
}

/// The helper definition ahu passes agrees with the ceiling it enforces: a
/// definition that advertised a tool the ceiling withholds would be a claim
/// about the helper that the session does not back.
#[test]
fn the_helper_definition_agrees_with_the_enforced_ceiling() {
    let profile = profile_of(&bounded());
    let agents = flag(&profile.args, "--agents").expect("--agents must be passed");
    let parsed: Value = serde_json::from_str(agents).expect("the definition must be valid JSON");
    let definition = parsed
        .get("ahu-reader")
        .expect("the definition is keyed by the requested role");
    assert_eq!(
        definition.get("model").and_then(Value::as_str),
        Some("claude-haiku-4-5-20251001")
    );
    let tools: Vec<&str> = definition
        .get("tools")
        .and_then(Value::as_array)
        .expect("the definition must list tools")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let ceiling = flag(&profile.args, "--tools").expect("--tools must be passed");
    for tool in &tools {
        assert!(
            ceiling.split(',').any(|allowed| allowed == *tool),
            "{tool} is advertised to the helper but is outside the ceiling"
        );
    }
    assert!(
        !tools.contains(&"Task"),
        "a helper does not delegate further"
    );
}

// ---------------------------------------------------------------------------
// Honesty of the disclosures
// ---------------------------------------------------------------------------

/// The two boundaries that could not be imposed have to be stated, because the
/// profile otherwise reads as if every requested boundary held.
#[test]
fn the_unenforceable_boundaries_are_disclosed() {
    let profile = profile_of(&bounded());
    let gaps = profile.gaps.join("\n");
    assert!(
        gaps.contains("ahu-reader") && gaps.contains("observed"),
        "the role must be disclosed as requested and observed: {gaps}"
    );
    assert!(
        gaps.contains("count"),
        "the absent count cap must be disclosed: {gaps}"
    );
    assert!(
        gaps.contains("read-only"),
        "the owner's own read-only confinement must be disclosed: {gaps}"
    );
    assert!(
        gaps.contains("cancel"),
        "unobservable provider-side cancellation must be disclosed: {gaps}"
    );
    assert!(
        !profile
            .enforced
            .iter()
            .any(|control| { control.id.contains("role") || control.id.contains("count") }),
        "a boundary that is disclosed as a gap must not also be claimed as enforced"
    );
}

/// No control may rest on instruction text. The type system already forbids it;
/// this test is here so that a future variant cannot be added without the
/// suite noticing.
#[test]
fn no_control_is_backed_by_prompt_text() {
    for request in [bounded(), disabled()] {
        let profile = profile_of(&request);
        for control in &profile.enforced {
            assert!(
                matches!(
                    control.mechanism,
                    Mechanism::Flag | Mechanism::Env | Mechanism::Structural
                ),
                "{} must be imposed by a flag, a variable, or a missing tool",
                control.id
            );
            let detail = control.detail.to_lowercase();
            assert!(
                !detail.contains("instruct") && !detail.contains("prompt text"),
                "{} describes itself as a prompt-level restriction",
                control.id
            );
        }
    }
}

/// The integrity record needs the mechanism alongside the control, so a later
/// reader can tell an enforced boundary from an asserted one.
#[test]
fn control_ids_carry_their_mechanism() {
    let profile = profile_of(&bounded());
    let ids = profile.control_ids();
    assert!(ids.iter().any(|id| id == "native.helper.depth=env"));
    assert!(ids.iter().any(|id| id == "native.helper.tools=flag"));
    assert!(
        ids.iter()
            .any(|id| id == "native.helper.no_cmux=structural")
    );
    assert_eq!(ids.len(), profile.enforced.len());
}

// ---------------------------------------------------------------------------
// The disabled policy
// ---------------------------------------------------------------------------

/// Turning delegation off is a control in its own right, and on Claude Code it
/// is imposed by withholding the tool rather than by asking.
#[test]
fn the_disabled_policy_withholds_the_delegation_tool() {
    let profile = profile_of(&disabled());
    assert_eq!(flag(&profile.args, "--disallowedTools"), Some("Task"));
    let control =
        control(&profile, "native.delegation.off").expect("disabling must be reported as enforced");
    assert_eq!(control.mechanism, Mechanism::Structural);
    assert!(!profile.helpers_permitted());
    assert!(profile.gaps.is_empty());
}

/// For a harness whose defaults were never checked, ahu says it does not know
/// rather than claiming delegation is off.
#[test]
fn disabling_is_not_claimed_for_an_unchecked_harness() {
    let profile = profile_of(&Request {
        harness: "antigravity",
        ..disabled()
    });
    assert!(
        profile.enforced.is_empty(),
        "no control was passed, so none may be claimed"
    );
    assert!(
        profile.gaps.iter().any(|gap| gap.contains("antigravity")),
        "the unchecked default must be disclosed: {:?}",
        profile.gaps
    );
}

// ---------------------------------------------------------------------------
// Observation and the join requirement
// ---------------------------------------------------------------------------

fn task_started(task_id: &str, role: &str, depth: u64, backgrounded: bool) -> Value {
    json!({
        "type": "system", "subtype": "task_started", "task_id": task_id,
        "tool_use_id": format!("toolu_{task_id}"),
        "description": "Read proof.txt",
        "subagent_type": role, "spawn_depth": depth, "is_backgrounded": backgrounded,
        "task_type": "local_agent"
    })
}

/// A background shell task, in the shape the CLI actually emits for one.
///
/// It travels on the same `task_*` events as a helper and differs only by
/// `task_type`: no `subagent_type`, no `spawn_depth`, and an empty `output_file`
/// on the notification.
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

/// The ordinary case: one helper started, reported, and joined.
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
    assert_eq!(helpers[0].summary, "proof.txt contains one line");
    assert_eq!(helpers[0].total_tokens, Some(10_629));
    assert!(helpers[0].output_file.is_some());

    let completeness = observations.completeness(&profile_of(&bounded()));
    assert!(completeness.complete, "{completeness:?}");
    assert!(completeness.terminated_cleanly && completeness.evidence_complete);
    assert!(completeness.unobserved.is_empty());
    assert_eq!(completeness.joined, vec!["a38fe4f46".to_string()]);
    assert!(completeness.blockers().is_empty());
}

/// A helper that started and never reported is unfinished work, however
/// confidently the attempt itself ended.
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
    let completeness = observations.completeness(&profile_of(&bounded()));
    assert!(!completeness.complete);
    assert_eq!(completeness.unjoined, vec!["second".to_string()]);
    let blockers = completeness.blockers().join("\n");
    assert!(
        blockers.contains("second") && blockers.contains("partial"),
        "{blockers}"
    );
}

/// A stream that stops before the harness says anything terminal is not a
/// success with no helpers; it is an attempt whose coverage is unknown.
#[test]
fn a_stream_without_a_terminal_event_is_not_complete() {
    let observations = feed(&[
        task_started("only", "ahu-reader", 1, false),
        task_notification("only", "completed", "done"),
    ]);
    let completeness = observations.completeness(&profile_of(&bounded()));
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

/// A helper deeper than the frozen policy is a violation, not a curiosity, and
/// it is reported whether it is seen per-helper or only in the statistics.
#[test]
fn a_helper_beyond_the_frozen_depth_is_a_violation() {
    let observations = feed(&[
        task_started("outer", "general-purpose", 1, false),
        task_started("inner", "general-purpose", 2, false),
        task_notification("inner", "completed", "nested"),
        task_notification("outer", "completed", "outer"),
        result(stats(
            2,
            2,
            2,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({"general-purpose": 2}),
        )),
    ]);
    let completeness = observations.completeness(&profile_of(&bounded()));
    assert!(!completeness.complete);
    assert!(
        completeness.violations.iter().any(|v| v.contains("inner")),
        "the nested helper must be named: {completeness:?}"
    );
    assert!(
        completeness
            .violations
            .iter()
            .any(|v| v.contains("harness reported")),
        "the harness's own depth statistic must be checked too: {completeness:?}"
    );
    assert!(!completeness.blockers().is_empty());
}

/// Under the disabled policy a helper running at all is a violation, which is
/// what makes the policy worth recording rather than merely passing.
#[test]
fn a_helper_under_the_disabled_policy_is_a_violation() {
    let observations = feed(&[
        task_started("unexpected", "general-purpose", 1, false),
        task_notification("unexpected", "completed", "ran anyway"),
        result(stats(
            1,
            1,
            1,
            json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
            json!({"general-purpose": 1}),
        )),
    ]);
    let completeness = observations.completeness(&profile_of(&disabled()));
    assert!(!completeness.complete);
    assert!(
        completeness
            .violations
            .iter()
            .any(|v| v.contains("permits none")),
        "{completeness:?}"
    );
}

/// A refused helper means work that was not done. It is not a failure of the
/// attempt, so it is an unknown rather than a blocker, but it cannot be silent.
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
    let completeness = observations.completeness(&profile_of(&bounded()));
    assert!(
        completeness.unknown.iter().any(|u| u.contains("refused")),
        "{completeness:?}"
    );
    assert!(
        completeness.complete,
        "a refusal the harness handled does not make the joined work incomplete: {completeness:?}"
    );
}

/// A stopped helper leaves a question ahu cannot answer, and it is recorded as
/// a question rather than as a clean cancellation.
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
    let completeness = observations.completeness(&profile_of(&bounded()));
    assert!(
        completeness
            .unknown
            .iter()
            .any(|u| u.contains("provider-side")),
        "{completeness:?}"
    );
}

/// Helpers the harness counted but never emitted events for are work that
/// happened out of ahu's sight, so the attempt cannot be called complete.
///
/// The attempt itself ends tidily here — every helper ahu *saw* joined — which
/// is exactly the shape that used to report `complete: true` while naming
/// helpers it never observed.
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
    let completeness = observations.completeness(&profile_of(&bounded()));
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

/// The architect's case: a clean Claude success reporting one spawned and one
/// completed helper, with no helper events at all, under the disabled policy.
///
/// There is enough evidence here to establish that native work occurred, so it
/// is a policy violation even though not one helper record exists to hang it on.
#[test]
fn a_helper_visible_only_in_terminal_counters_violates_the_disabled_policy() {
    let observations = feed(&[result(stats(
        1,
        1,
        1,
        json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
        json!({"general-purpose": 1}),
    ))]);
    assert!(
        observations.helpers().is_empty(),
        "the stream carried no helper events, by construction"
    );
    let completeness = observations.completeness(&profile_of(&disabled()));
    assert!(
        completeness
            .violations
            .iter()
            .any(|v| v.contains("permits none")),
        "a helper counted by the harness is still a helper: {completeness:?}"
    );
    assert!(
        completeness
            .violations
            .iter()
            .any(|v| v.contains("terminal statistics")),
        "the violation must say the helper was visible only in the totals: {completeness:?}"
    );
    assert!(!completeness.complete && !completeness.terminated_cleanly);
    assert!(
        !completeness.blockers().is_empty(),
        "the violation must reach the result envelope: {completeness:?}"
    );
}

/// The same stream under the bounded policy is not a violation, but the helper
/// is still invisible, so the envelope and the completeness field have to agree
/// that something is missing rather than one of them reporting a clean run.
#[test]
fn a_helper_visible_only_in_terminal_counters_is_unobserved_under_bounded() {
    let observations = feed(&[result(stats(
        1,
        1,
        1,
        json!({"depth_limit": 0, "concurrency_limit": 0, "budget": 0}),
        json!({"ahu-reader": 1}),
    ))]);
    let completeness = observations.completeness(&profile_of(&bounded()));
    assert!(
        completeness.violations.is_empty(),
        "bounded permits helpers, so running one is not a violation: {completeness:?}"
    );
    assert_eq!(completeness.unobserved.len(), 1, "{completeness:?}");
    assert!(
        !completeness.evidence_complete && !completeness.complete,
        "{completeness:?}"
    );
    assert!(
        completeness.terminated_cleanly,
        "nothing ahu saw was left open: {completeness:?}"
    );
    assert!(
        !completeness.blockers().is_empty(),
        "the envelope must report the missing helper: {completeness:?}"
    );
}

/// A refusal is not unobserved work. The harness declining to start a helper is
/// complete information about something that did not happen, so it is reported
/// without holding the attempt open — the distinction the fix had to preserve.
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
    let completeness = observations.completeness(&profile_of(&bounded()));
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

/// An unrecognised helper status fails both halves, and for two different
/// reasons worth keeping apart.
///
/// The event was unreadable, so the evidence is incomplete; and because ahu
/// cannot tell whether that status meant the helper finished, the helper stays
/// unjoined rather than being counted as done on the strength of a word this
/// version does not know.
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
    let completeness = observations.completeness(&profile_of(&bounded()));
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

/// A status this version does not recognise is counted, never guessed at, and
/// the count blocks the attempt because helper coverage is then incomplete.
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
    let completeness = observations.completeness(&profile_of(&bounded()));
    assert!(!completeness.complete);
    assert!(
        completeness
            .blockers()
            .iter()
            .any(|b| b.contains("could not be read")),
        "{completeness:?}"
    );
}

/// A late progress event must not reopen a helper the harness already finished.
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
    assert!(observations.completeness(&profile_of(&bounded())).complete);
}

/// Events that say nothing about helpers must not inflate the unknown count,
/// or every ordinary attempt would look like one with unreadable coverage.
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
    assert!(observations.completeness(&profile_of(&disabled())).complete);
}

/// A helper event with no task id cannot be joined to anything, so it is
/// counted rather than attached to whichever helper happens to be open.
#[test]
fn a_helper_event_without_a_task_id_is_counted() {
    let observations = feed(&[
        json!({"type": "system", "subtype": "task_started", "subagent_type": "ahu-reader"}),
        json!({"type": "system", "subtype": "task_notification", "status": "completed"}),
    ]);
    assert_eq!(observations.unknown_events, 2);
    assert!(observations.helpers().is_empty());
}

/// The roles a helper actually ran under are recorded, because the requested
/// role is not enforced and the difference is the evidence for that gap.
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

/// Codex emits helper activity ahu cannot join: its collaboration events carry
/// no identifiable receiver. Counting them keeps that gap visible rather than
/// letting an unreadable stream pass for a quiet one.
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

// ---------------------------------------------------------------------------
// Background shell tasks are not native helpers
// ---------------------------------------------------------------------------

/// The live mixed-workflow regression, in the shape the CLI actually emitted.
///
/// A parent under the disabled policy ran two background Bash tasks to launch
/// and await a registered ahu child. Those travel on the same `task_*` events a
/// helper does, and counting them as native delegation reported a policy
/// violation and failed an attempt whose real work was correct.
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

    let completeness = observations.completeness(&profile_of(&disabled()));
    assert!(
        completeness.violations.is_empty(),
        "running a background command is not native delegation: {completeness:?}"
    );
    assert!(
        completeness.complete && completeness.blockers().is_empty(),
        "the attempt must not be failed by its own background shell work: {completeness:?}"
    );
}

/// The terminal counters agree with that reading, and the two must not be made
/// to contradict each other: a stream with shell tasks and `spawned: 0` has no
/// unobserved helper work in it.
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
    let completeness = observations.completeness(&profile_of(&bounded()));
    assert!(completeness.unobserved.is_empty(), "{completeness:?}");
    assert!(completeness.evidence_complete && completeness.complete);
}

/// Shell and helper tasks interleaved on one stream are told apart by what the
/// harness called each of them, not by their order or their neighbours.
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

    let completeness = observations.completeness(&profile_of(&bounded()));
    assert!(completeness.complete, "{completeness:?}");
    assert_eq!(completeness.joined, vec!["helper1".to_string()]);
}

/// A shell task that never reports does not hold the attempt open either: it is
/// not a helper, so there is no helper to be unjoined.
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
    let completeness = observations.completeness(&profile_of(&disabled()));
    assert!(completeness.unjoined.is_empty(), "{completeness:?}");
    assert!(completeness.complete, "{completeness:?}");
}

// ---------------------------------------------------------------------------
// Unknown delegation is still detected
// ---------------------------------------------------------------------------

/// An agent variant this version has never heard of is still a helper.
///
/// `subagent_type` is decisive: narrowing to a known `task_type` alone would let
/// a future agent kind pass as ordinary background work, which is the opposite
/// of the mistake being fixed.
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

    let completeness = observations.completeness(&profile_of(&disabled()));
    assert!(
        completeness
            .violations
            .iter()
            .any(|v| v.contains("permits none")),
        "an unfamiliar agent kind under a policy permitting none is still a violation: {completeness:?}"
    );
}

/// A task that names neither a known kind nor a role cannot be placed, so it is
/// counted rather than assumed to be harmless background work.
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
    let completeness = observations.completeness(&profile_of(&bounded()));
    assert!(
        !completeness.evidence_complete && !completeness.complete,
        "an unplaceable task leaves the attempt's coverage incomplete: {completeness:?}"
    );
}

/// A task whose start ahu never saw cannot be placed either, and must not have a
/// helper invented for it — the shape that would reintroduce the live defect
/// whenever a stream is truncated.
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
    let completeness = observations.completeness(&profile_of(&disabled()));
    assert!(
        completeness.violations.is_empty(),
        "an unplaceable task is not evidence that a helper ran: {completeness:?}"
    );
    assert!(!completeness.evidence_complete, "{completeness:?}");
}

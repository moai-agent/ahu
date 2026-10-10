//! Kiro V3 ACP JSONL (2.28/2.29). Only runFinished can report success;
//! tool failures and turn_end are not task outcomes. Payloads stay native.
use super::Events;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub(super) struct State {
    started: bool,
    tools: BTreeMap<String, ToolStatus>,
    unresolved_error: bool,
    approvals: BTreeMap<String, BTreeMap<String, bool>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolStatus {
    Pending,
    Completed,
    Failed,
}

pub(super) fn valid_session(session: &str) -> bool {
    session.starts_with("sess_")
        && (6..=128).contains(&session.len())
        && session
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

impl Events {
    fn kiro_failure(&mut self, diagnostic: &str) {
        self.failed = true;
        // Diagnostics contain only adapter-owned categories, never payload text.
        if self.blockers.len() < 32 && !self.blockers.iter().any(|b| b == diagnostic) {
            self.blockers.push(diagnostic.into());
        }
    }

    fn finish_kiro(&mut self, failed: bool) {
        if self.terminal {
            self.kiro_failure("multiple terminal events");
        }
        self.terminal = true;
        self.failed |= failed;
    }

    pub(super) fn observe_kiro(&mut self, event: &Value) {
        // Kiro config updates describe selection, not inference identity. Keep
        // reported model coverage unknown rather than promoting that selection.
        self.model_evidence.observe(std::iter::empty());
        // Forward every event, including replay and failures, to the eval observer.
        self.trajectory.observe("kiro", event);
        let kind = event["type"].as_str().unwrap_or("");
        let data = &event["data"];
        if matches!(kind, "sessionUpdate" | "runFinished")
            || (kind == "runError" && !data["sessionId"].is_null())
        {
            match data["sessionId"].as_str().filter(|s| valid_session(s)) {
                Some(session) if self.session.as_deref().is_none_or(|s| s == session) => {
                    self.session = Some(session.into());
                }
                _ => self.kiro_failure("Kiro session identity is missing, invalid or changed"),
            }
        }
        match kind {
            "runStarted" => {
                if self.kiro.started || self.terminal {
                    self.kiro_failure("multiple or out-of-order Kiro run starts");
                }
                self.kiro.started = true;
                if data["engine"] != "v3"
                    || data["payloadSchema"] != "acp"
                    || (!data["acpProtocolVersion"].is_null() && data["acpProtocolVersion"] != 1)
                {
                    self.kiro_failure("unsupported Kiro stream protocol");
                }
            }
            "runFinished" => {
                if !self.kiro.started {
                    self.kiro_failure("missing Kiro run start");
                }
                let success = data["status"] == "success"
                    && data["stopReason"] == "end_turn"
                    && data["finalText"].is_string()
                    && data["finalTextTruncated"] == false;
                self.summary = data["finalText"].as_str().unwrap_or("").into();
                if !success {
                    self.kiro_failure("Kiro ended without a complete successful result");
                }
                if self.kiro.tools.values().any(|s| *s == ToolStatus::Pending) {
                    self.kiro_failure("Kiro ended with unresolved tool calls");
                }
                if !self.kiro.approvals.is_empty() {
                    self.kiro_failure("Kiro ended with unresolved tool approvals");
                }
                if self.kiro.unresolved_error {
                    self.kiro_failure("Kiro ended with an unresolved task error");
                }
                self.finish_kiro(!success);
            }
            "sessionUpdate" => {
                let update = &data["update"];
                // load_session replays prior errors, tools and turn outcomes. They
                // belong to history, not this attempt; identity is still checked.
                if update["_meta"]["kiro"]["replay"] == true {
                    return;
                }
                if self.terminal {
                    self.kiro_failure("Kiro session update after terminal result");
                }
                match update["sessionUpdate"].as_str().unwrap_or("") {
                    "config_option_update" => {
                        if let Some(options) = update["configOptions"].as_array() {
                            for option in options {
                                if option["id"] == "model" {
                                    self.model = option["currentValue"].as_str().map(str::to_owned);
                                }
                            }
                        }
                    }
                    "tool_call" | "tool_call_update" => self.observe_kiro_tool(update),
                    "session_info_update" => {
                        let meta = &update["_meta"]["kiro"];
                        self.observe_kiro_approval(meta);
                        match meta["kind"].as_str() {
                            // No causal recovery ID accompanies display_error.
                            // A later success record cannot prove it was resolved.
                            Some("display_error") => self.kiro.unresolved_error = true,
                            Some("turn_completion")
                                if matches!(
                                    meta["status"].as_str(),
                                    Some("failed" | "aborted")
                                ) =>
                            {
                                self.kiro_failure("Kiro reported an unsuccessful turn");
                            }
                            Some("turn_end")
                                if matches!(
                                    meta["stopReason"].as_str(),
                                    Some(
                                        "error"
                                            | "refusal"
                                            | "cancelled"
                                            | "max_tokens"
                                            | "max_turn_requests"
                                    )
                                ) =>
                            {
                                self.kiro_failure("Kiro reported an unsuccessful turn");
                            }
                            _ => (),
                        }
                    }
                    "available_commands_update"
                    | "agent_message_chunk"
                    | "agent_thought_chunk"
                    | "user_message_chunk"
                    | "plan" => (),
                    _ => self.unknown_events += 1,
                }
            }
            "error" | "runError" => {
                self.kiro_failure(error_diagnostic(data));
                // runError can occur before session creation, including interruption.
                // It is a terminal failure; a later success cannot overwrite it.
                if kind == "runError" {
                    self.finish_kiro(true);
                }
            }
            _ => self.unknown_events += 1,
        }
    }

    fn observe_kiro_approval(&mut self, meta: &Value) {
        match meta["kind"].as_str() {
            Some("pending_interaction") if meta["interactionType"] == "tool_approval" => {
                let Some(id) = meta["toolCallId"]
                    .as_str()
                    .filter(|id| !id.is_empty() && id.len() <= 256)
                else {
                    self.kiro_failure("Kiro tool approval identity is missing or invalid");
                    return;
                };
                let Some(options) = meta["options"]
                    .as_array()
                    .filter(|a| !a.is_empty() && a.len() <= 32)
                else {
                    self.kiro_failure("Kiro tool approval options are invalid");
                    return;
                };
                if self.kiro.approvals.len() >= 4096 {
                    self.kiro_failure("Kiro tool approval tracking limit exceeded");
                    return;
                }
                let mut choices = BTreeMap::new();
                for option in options {
                    let Some(id) = option["optionId"]
                        .as_str()
                        .filter(|id| !id.is_empty() && id.len() <= 256)
                    else {
                        self.kiro_failure("Kiro tool approval options are invalid");
                        return;
                    };
                    let allowed = match option["kind"].as_str() {
                        Some("allow_once" | "allow_always") => true,
                        Some("reject_once" | "reject_always") => false,
                        _ => {
                            self.kiro_failure("Kiro tool approval options are invalid");
                            return;
                        }
                    };
                    if choices.insert(id.into(), allowed).is_some() {
                        self.kiro_failure("Kiro tool approval options are invalid");
                    }
                }
                if self.kiro.approvals.insert(id.into(), choices).is_some() {
                    self.kiro_failure("Kiro tool approval replaced before resolution");
                }
            }
            Some("interaction_resolved") => {
                let id = meta["toolCallId"].as_str().unwrap_or("");
                if let Some(choices) = self.kiro.approvals.remove(id) {
                    let allowed = meta["outcome"] == "selected"
                        && meta["selectedOption"]
                            .as_str()
                            .and_then(|id| choices.get(id))
                            == Some(&true);
                    if !allowed {
                        self.kiro_failure("Kiro tool approval was denied, cancelled or unresolved");
                    }
                }
            }
            _ => (),
        }
    }

    fn observe_kiro_tool(&mut self, update: &Value) {
        let meta = &update["_meta"]["kiro"];
        if meta["policyDenial"]["effect"] == "deny"
            || ["hookAsk", "hookConfirm", "safetyOverride"]
                .iter()
                .any(|key| {
                    matches!(
                        meta[key]["decision"].as_str(),
                        Some("rejected" | "denied" | "cancelled" | "error")
                    )
                })
            || (update["status"] == "failed" && native_denial(&update["rawOutput"]))
        {
            self.kiro_failure("Kiro reported a denied tool call");
        }
        if meta["sandboxFailure"]["enabled"] == true {
            self.kiro_failure("Kiro reported a tool sandbox failure");
        }
        // Kiro's structured marker identifies helpers. Titles are display text.
        if meta["kind"] == "agent-subtask" || meta["agentSubtaskId"].is_string() {
            self.native
                .observe("kiro", &serde_json::json!({"type":"collab"}));
        }
        let Some(id) = update["toolCallId"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 256)
        else {
            self.kiro_failure("Kiro tool call identity is missing or invalid");
            return;
        };
        if !self.kiro.tools.contains_key(id) && self.kiro.tools.len() >= 4096 {
            self.kiro_failure("Kiro tool outcome tracking limit exceeded");
            return;
        }
        let status = match update["status"].as_str() {
            Some("pending" | "in_progress") => ToolStatus::Pending,
            Some("completed") => ToolStatus::Completed,
            Some("failed") => ToolStatus::Failed,
            None if update["status"].is_null() => {
                // ACP updates may change only content; keep the prior status.
                self.kiro
                    .tools
                    .entry(id.into())
                    .or_insert(ToolStatus::Pending);
                return;
            }
            _ => {
                self.kiro_failure("Kiro tool call status is invalid");
                return;
            }
        };
        if self
            .kiro
            .tools
            .get(id)
            .is_some_and(|prior| *prior != ToolStatus::Pending && *prior != status)
        {
            self.kiro_failure("Kiro tool call has conflicting outcomes");
        }
        // Kiro maps Error, HandledError, Canceled and Rejected to ACP failed.
        // An ordinary failed tool is settled, not proof that the task failed.
        // Run/turn errors, denial evidence and incomplete calls remain blockers.
        self.kiro.tools.insert(id.into(), status);
    }
}

fn native_denial(output: &Value) -> bool {
    let Some(text) = output.as_str().or_else(|| output["message"].as_str()) else {
        return false;
    };
    // Native rejection strings can be the shell result's escaped message. Match
    // only the full native envelope, never a substring of arbitrary tool output.
    let text = text
        .strip_prefix("Output:\\n")
        .and_then(|s| s.strip_suffix("\\n\\nExit Code: -1"))
        .or_else(|| {
            text.strip_prefix("Output:\n")
                .and_then(|s| s.strip_suffix("\n\nExit Code: -1"))
        })
        .unwrap_or(text);
    matches!(
        text,
        "Tool use was not permitted."
            | "The user rejected this tool call."
            | "The tool call was cancelled before it was approved."
            | "The policy check was not approved to run as requested."
    ) || text.starts_with("The user rejected this tool call: ")
}

fn error_diagnostic(data: &Value) -> &'static str {
    // These stages occur in the native CLI launcher. Unknown strings are not
    // copied, even when short: error fields can contain prompts, paths or secrets.
    match data["stage"].as_str() {
        Some("initialize") => "Kiro run error: ACP initialization",
        Some("new_session") => "Kiro run error: session creation",
        Some(
            "load_session"
            | "load_session_unsupported"
            | "resume_convert"
            | "resume_no_session"
            | "resume_lookup",
        ) => "Kiro run error: session resume",
        Some("current_dir") => "Kiro run error: working directory",
        Some("interrupted") => "Kiro run error: interrupted",
        Some("prompt") => "Kiro run error: prompt execution",
        _ => "Kiro emitted an error (unclassified)",
    }
}

/// ACP locations are mutation targets only for the structured operation kinds.
pub(super) fn write_paths(event: &Value) -> Vec<String> {
    let update = &event["data"]["update"];
    if event["type"] != "sessionUpdate"
        || update["_meta"]["kiro"]["replay"] == true
        || !matches!(
            update["sessionUpdate"].as_str(),
            Some("tool_call" | "tool_call_update")
        )
        || !matches!(update["kind"].as_str(), Some("edit" | "delete" | "move"))
    {
        return Vec::new();
    }
    update["locations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|location| location["path"].as_str().map(str::to_owned))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn start() -> Events {
        let mut events = Events::default();
        events.observe_kiro(&json!({"type":"runStarted","data":{"engine":"v3","payloadSchema":"acp","acpProtocolVersion":1}}));
        events
    }

    fn update(events: &mut Events, update: Value) {
        events.observe_kiro(
            &json!({"type":"sessionUpdate","data":{"sessionId":"sess_synthetic","update":update}}),
        );
    }

    fn terminal() -> Value {
        json!({"type":"runFinished","data":{"sessionId":"sess_synthetic","status":"success","stopReason":"end_turn","finalText":"synthetic result","finalTextTruncated":false}})
    }

    #[test]
    fn recovered_tool_error_is_not_a_task_failure_and_still_reaches_evals() {
        let mut e = start();
        update(
            &mut e,
            json!({"sessionUpdate":"tool_call","toolCallId":"a","status":"in_progress","kind":"read"}),
        );
        update(
            &mut e,
            json!({"sessionUpdate":"tool_call_update","toolCallId":"a","status":"failed","rawOutput":{"message":"ENOENT: synthetic missing file"}}),
        );
        update(
            &mut e,
            json!({"sessionUpdate":"tool_call","toolCallId":"b","status":"completed","kind":"read"}),
        );
        e.observe_kiro(&terminal());
        assert!(e.terminal && !e.failed, "{e:?}");
        assert_eq!(e.trajectory.observation.tool_calls, Some(2));
        assert_eq!(e.trajectory.observation.tool_errors, Some(1));
        assert_eq!(e.trajectory.observation.completed_tool_calls, Some(2));
    }

    #[test]
    fn structured_tool_approvals_distinguish_allow_deny_cancel_and_missing_resolution() {
        for (outcome, option, failed) in [
            ("selected", "yes", false),
            ("selected", "no", true),
            ("cancelled", "", true),
            ("selected", "unknown", true),
            ("unknown", "yes", true),
            ("missing", "", true),
        ] {
            let mut e = start();
            update(
                &mut e,
                json!({"sessionUpdate":"session_info_update","_meta":{"kiro":{
                    "kind":"pending_interaction","interactionType":"tool_approval","toolCallId":"a",
                    "options":[{"optionId":"yes","kind":"allow_once"},{"optionId":"no","kind":"reject_once"}]
                }}}),
            );
            if outcome != "missing" {
                update(
                    &mut e,
                    json!({"sessionUpdate":"session_info_update","_meta":{"kiro":{
                        "kind":"interaction_resolved","toolCallId":"a","outcome":outcome,"selectedOption":option
                    }}}),
                );
            }
            e.observe_kiro(&terminal());
            assert_eq!(e.failed, failed, "{outcome}/{option}: {e:?}");
        }
    }

    #[test]
    fn explicit_denial_evidence_survives_success_and_keeps_payloads_private() {
        for (meta, output) in [
            (
                json!({"policyDenial":{"effect":"deny","resource":"private-resource"}}),
                json!("private-message"),
            ),
            (
                json!({"hookAsk":{"decision":"rejected","reason":"private-message"}}),
                Value::Null,
            ),
            (json!({"hookConfirm":{"decision":"cancelled"}}), Value::Null),
            (json!({"safetyOverride":{"decision":"denied"}}), Value::Null),
            (json!({"sandboxFailure":{"enabled":true}}), Value::Null),
            (
                json!({}),
                json!({"message":"Output:\\nThe user rejected this tool call.\\n\\nExit Code: -1"}),
            ),
            (
                json!({}),
                json!({"message":"Output:\nThe user rejected this tool call.\n\nExit Code: -1"}),
            ),
            (json!({}), json!("Tool use was not permitted.")),
            (
                json!({}),
                json!("The user rejected this tool call: private-message"),
            ),
        ] {
            let mut e = start();
            update(
                &mut e,
                json!({"sessionUpdate":"tool_call_update","toolCallId":"a","status":"failed","rawOutput":output,"_meta":{"kiro":meta}}),
            );
            e.observe_kiro(&terminal());
            assert!(e.failed && e.terminal, "{e:?}");
            let stored = serde_json::to_string(&e).unwrap();
            assert!(!stored.contains("private-message") && !stored.contains("private-resource"));
        }
        assert!(!native_denial(&json!(
            "File contents: The user rejected this tool call."
        )));
    }

    #[test]
    fn replay_does_not_poison_current_attempt_or_report_old_writes() {
        let mut e = start();
        for mut u in [
            json!({"sessionUpdate":"tool_call","toolCallId":"old","kind":"edit","status":"pending","locations":[{"path":"/synthetic/outside"}]}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"old","status":"failed","rawOutput":"Tool use was not permitted."}),
            json!({"sessionUpdate":"session_info_update","_meta":{"kiro":{"kind":"turn_completion","status":"failed"}}}),
            json!({"sessionUpdate":"session_info_update","_meta":{"kiro":{"kind":"display_error","message":"private-history"}}}),
        ] {
            u["_meta"]["kiro"]["replay"] = json!(true);
            assert!(write_paths(&json!({"type":"sessionUpdate","data":{"update":u}})).is_empty());
            update(&mut e, u);
        }
        e.observe_kiro(&terminal());
        assert!(!e.failed, "{e:?}");
    }

    #[test]
    fn task_errors_are_not_cleared_by_later_success_metadata() {
        for resolved in [false, true] {
            let mut e = start();
            update(
                &mut e,
                json!({"sessionUpdate":"session_info_update","_meta":{"kiro":{"kind":"display_error","message":"private-error"}}}),
            );
            if resolved {
                update(
                    &mut e,
                    json!({"sessionUpdate":"session_info_update","_meta":{"kiro":{"kind":"turn_completion","status":"success"}}}),
                );
            }
            e.observe_kiro(&terminal());
            assert!(
                e.failed,
                "display_error has no correlated recovery evidence"
            );
        }
        for meta in [
            json!({"kind":"turn_completion","status":"failed"}),
            json!({"kind":"turn_completion","status":"aborted"}),
            json!({"kind":"turn_end","stopReason":"cancelled"}),
            json!({"kind":"turn_end","stopReason":"error"}),
        ] {
            let mut e = start();
            update(
                &mut e,
                json!({"sessionUpdate":"session_info_update","_meta":{"kiro":meta}}),
            );
            e.observe_kiro(&terminal());
            assert!(e.failed);
        }
        let mut e = start();
        update(
            &mut e,
            json!({"sessionUpdate":"session_info_update","_meta":{"kiro":{"kind":"turn_end","stopReason":"end_turn"}}}),
        );
        assert!(!e.terminal, "turn end is not workflow/run completion");
    }

    #[test]
    fn malformed_interrupted_and_duplicate_terminal_records_never_succeed() {
        for (field, value) in [
            ("status", json!("interrupted")),
            ("status", Value::Null),
            ("stopReason", json!("cancelled")),
            ("stopReason", Value::Null),
            ("finalText", Value::Null),
            ("finalTextTruncated", json!(true)),
            ("finalTextTruncated", Value::Null),
            ("sessionId", json!("sess_changed")),
            ("sessionId", json!("--bad")),
            ("sessionId", Value::Null),
        ] {
            let mut e = start();
            update(&mut e, json!({"sessionUpdate":"session_info_update"}));
            let mut end = terminal();
            end["data"][field] = value;
            e.observe_kiro(&end);
            assert!(e.failed && e.terminal, "{field}: {e:?}");
        }
        for conflict in [false, true] {
            let mut e = start();
            e.observe_kiro(&terminal());
            let mut end = terminal();
            if conflict {
                end["data"]["status"] = json!("failed");
            }
            e.observe_kiro(&end);
            assert!(e.failed && e.blockers.iter().any(|b| b == "multiple terminal events"));
        }
        let mut e = Events::default();
        e.observe_kiro(&terminal());
        assert!(e.failed);
        let mut e = start();
        e.observe("kiro", b"{truncated");
        e.observe_kiro(&terminal());
        assert!(e.failed);
        let mut e = start();
        e.observe_kiro(&json!({"type":"runStarted","data":{"engine":"v3","payloadSchema":"acp"}}));
        assert!(e.failed);
        let mut e = start();
        e.observe_kiro(&terminal());
        update(&mut e, json!({"sessionUpdate":"agent_message_chunk"}));
        assert!(e.failed);
    }

    #[test]
    fn incomplete_invalid_and_conflicting_tool_outcomes_fail_closed() {
        for updates in [
            vec![json!({"toolCallId":"a","status":"pending"})],
            vec![json!({"toolCallId":"a","status":"unknown"})],
            vec![json!({"status":"failed"})],
            vec![
                json!({"toolCallId":"a","status":"failed"}),
                json!({"toolCallId":"a","status":"completed"}),
            ],
        ] {
            let mut e = start();
            for mut u in updates {
                u["sessionUpdate"] = json!("tool_call_update");
                update(&mut e, u);
            }
            e.observe_kiro(&terminal());
            assert!(e.failed, "{e:?}");
        }
        let mut e = start();
        for status in [
            json!("in_progress"),
            Value::Null,
            json!("completed"),
            Value::Null,
            json!("completed"),
        ] {
            update(
                &mut e,
                json!({"sessionUpdate":"tool_call_update","toolCallId":"a","status":status}),
            );
        }
        e.observe_kiro(&terminal());
        assert!(!e.failed, "{e:?}");
    }

    #[test]
    fn error_diagnostics_are_bounded_and_do_not_copy_native_strings() {
        for kind in ["runError", "error"] {
            let mut e = start();
            for _ in 0..100 {
                // runError normally appears once; use error to exercise deduplication.
                e.observe_kiro(&json!({"type":"error","data":{"stage":"private-stage","message":"private-message"}}));
            }
            e.observe_kiro(
                &json!({"type":kind,"data":{"stage":"load_session","message":"private-message"}}),
            );
            assert!(e.failed);
            assert_eq!(e.terminal, kind == "runError");
            assert_eq!(e.blockers.len(), 2);
            assert!(
                e.blockers
                    .iter()
                    .any(|b| b == "Kiro run error: session resume")
            );
            assert!(!serde_json::to_string(&e).unwrap().contains("private-"));
            e.observe_kiro(&terminal());
            assert!(e.failed);
        }
    }

    #[test]
    fn kiro_outcome_state_and_diagnostics_are_bounded() {
        let mut e = start();
        for i in 0..4097 {
            update(
                &mut e,
                json!({"sessionUpdate":"tool_call_update","toolCallId":format!("call_{i}"),"status":"completed"}),
            );
        }
        assert!(e.failed);
        assert_eq!(e.kiro.tools.len(), 4096);
        for _ in 0..100 {
            e.observe_kiro(&json!({"type":"runError","data":{"stage":"interrupted","message":"private-message"}}));
        }
        assert!(e.terminal);
        assert_eq!(e.blockers.len(), 3);
        assert!(!serde_json::to_string(&e).unwrap().contains("call_4096"));
    }

    #[test]
    fn invalid_protocol_and_session_changes_fail_even_on_replayed_updates() {
        for data in [
            json!({"engine":"v2","payloadSchema":"acp"}),
            json!({"engine":"v3","payloadSchema":"other"}),
            json!({"engine":"v3","payloadSchema":"acp","acpProtocolVersion":2}),
        ] {
            let mut e = Events::default();
            e.observe_kiro(&json!({"type":"runStarted","data":data}));
            e.observe_kiro(&terminal());
            assert!(e.failed);
        }
        let mut e = start();
        update(&mut e, json!({"sessionUpdate":"session_info_update"}));
        e.observe_kiro(&json!({"type":"sessionUpdate","data":{"sessionId":"sess_other","update":{"sessionUpdate":"session_info_update","_meta":{"kiro":{"replay":true}}}}}));
        e.observe_kiro(&terminal());
        assert!(e.failed);
    }

    #[test]
    fn tool_titles_never_establish_helpers_or_writes() {
        let mut e = start();
        update(
            &mut e,
            json!({"sessionUpdate":"tool_call","toolCallId":"a","status":"completed","title":"Read subagent workflow documentation"}),
        );
        assert_eq!(e.native.unknown_events, 0);
        update(
            &mut e,
            json!({"sessionUpdate":"tool_call","toolCallId":"b","status":"completed","title":"neutral","_meta":{"kiro":{"kind":"agent-subtask","agentSubtaskId":"child"}}}),
        );
        assert_eq!(e.native.unknown_events, 1);
        assert!(write_paths(&json!({"type":"sessionUpdate","data":{"update":{"sessionUpdate":"tool_call","kind":"read","title":"Write File","locations":[{"path":"/synthetic/read"}]}}})).is_empty());
    }

    #[test]
    fn only_edit_calls_report_write_locations() {
        let mut event = json!({"type":"sessionUpdate","data":{"update":{"sessionUpdate":"tool_call","kind":"edit","locations":[{"path":"/tmp/outside"}],"rawInput":{"text":"private payload"}}}});
        assert_eq!(write_paths(&event), vec!["/tmp/outside"]);
        event["data"]["update"]["kind"] = json!("read");
        assert!(write_paths(&event).is_empty());
    }

    #[test]
    fn batch_permissions_resume_and_prompt_are_literal() {
        use crate::agent::Permissions;
        use crate::harness::LaunchRequest;
        use crate::headless::{Options, Spec, batch_command};
        let mut spec = Spec {
            schema_version: 2,
            options: Options::default(),
            harness_version: "2.28.0".into(),
            executable_digest: "0".repeat(64),
            parent_task: None,
            parent_attempt: None,
            root_task: None,
            broker_request: None,
            child_grants: vec![],
            depth: 0,
            attempt: 1,
            session: Some("sess_fixture".into()),
            broker_dir: None,
            native_profile: None,
            native_controls: vec![],
            gaps: vec![],
        };
        let mut request = LaunchRequest {
            model: "claude-sonnet-4.5",
            prompt: "Literal $(never) `execute`; --version",
            cwd: std::path::Path::new("/tmp"),
            permissions: Permissions::Auto,
        };
        let command = batch_command("kiro", &request, &spec).unwrap();
        assert_eq!(command.args.last().unwrap(), request.prompt);
        assert!(command.args.iter().any(|a| a == "--trust-all-tools"));
        assert!(
            command
                .args
                .windows(2)
                .any(|a| a == ["--resume-id", "sess_fixture"])
        );
        assert!(!command.args.iter().any(|a| a == "--"));
        request.permissions = Permissions::AcceptEdits;
        assert!(batch_command("kiro", &request, &spec).is_err());
        request.permissions = Permissions::Prompt;
        spec.session = Some("--version".into());
        assert!(batch_command("kiro", &request, &spec).is_err());
        spec.session = None;
        request.prompt = "--version";
        assert!(batch_command("kiro", &request, &spec).is_err());
    }
}

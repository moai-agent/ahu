//! Kiro CLI 2.28.0 V3 ACP JSONL. Only runFinished completes an attempt;
//! turn_end may precede pending workflows. Payloads stay in native history.
use super::Events;
use serde_json::Value;

pub(super) fn valid_session(session: &str) -> bool {
    session.starts_with("sess_")
        && (6..=128).contains(&session.len())
        && session
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

impl Events {
    pub(super) fn observe_kiro(&mut self, event: &Value) {
        let kind = event["type"].as_str().unwrap_or("");
        let data = &event["data"];
        if matches!(kind, "sessionUpdate" | "runFinished") {
            match data["sessionId"].as_str().filter(|s| valid_session(s)) {
                Some(session) if self.session.as_deref().is_none_or(|s| s == session) => {
                    self.session = Some(session.into());
                }
                _ => {
                    self.failed = true;
                    self.blockers
                        .push("Kiro session identity is missing, invalid or changed".into());
                }
            }
        }
        match kind {
            "runStarted" => {
                if data["engine"] != "v3" || data["payloadSchema"] != "acp" {
                    self.failed = true;
                    self.blockers
                        .push("unsupported Kiro stream protocol".into());
                }
            }
            "runFinished" => {
                let success = data["status"] == "success"
                    && data["stopReason"] == "end_turn"
                    && data["finalText"].is_string()
                    && data["finalTextTruncated"] == false;
                self.summary = data["finalText"].as_str().unwrap_or("").into();
                if !success {
                    self.blockers
                        .push("Kiro ended without a complete successful result".into());
                }
                self.finish(!success);
            }
            "sessionUpdate" => {
                let update = &data["update"];
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
                    "tool_call" | "tool_call_update" => {
                        // Native Kiro can emit runFinished success after a denied
                        // tool. Never turn that into an ahu success. Raw error
                        // text and tool arguments are deliberately not retained.
                        if update["status"] == "failed" {
                            self.failed = true;
                            self.blockers
                                .push("Kiro reported a failed or denied tool call".into());
                        }
                        let title = update["title"].as_str().unwrap_or("").to_ascii_lowercase();
                        if title.contains("subagent") || title.contains("workflow") {
                            self.native
                                .observe("kiro", &serde_json::json!({"type":"collab"}));
                        }
                    }
                    "session_info_update"
                    | "available_commands_update"
                    | "agent_message_chunk"
                    | "agent_thought_chunk"
                    | "user_message_chunk"
                    | "plan" => (),
                    _ => self.unknown_events += 1,
                }
            }
            "error" => {
                self.failed = true;
                self.blockers.push("Kiro emitted an error".into());
            }
            _ => self.unknown_events += 1,
        }
    }
}

/// ACP locations are write targets only on edit calls, never on read tools.
pub(super) fn write_paths(event: &Value) -> Vec<String> {
    let update = &event["data"]["update"];
    if event["type"] != "sessionUpdate"
        || !matches!(
            update["sessionUpdate"].as_str(),
            Some("tool_call" | "tool_call_update")
        )
        || !(update["kind"] == "edit"
            || matches!(
                update["title"].as_str(),
                Some("Write File" | "Edit File" | "Delete File")
            ))
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

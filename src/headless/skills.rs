//! Bounded synthetic protocol adapters. A tool result is not proof of instruction adherence.
use super::{Events, SkillInvocation};
use crate::task;
use serde_json::Value;
use std::time::Instant;

#[derive(Debug)]
pub(super) struct Call {
    id: String,
    harness: String,
    index: usize,
    started: Instant,
    ambiguous: bool,
}

fn call_id(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_graphic()))
}

impl Events {
    pub(super) fn observe_skill(&mut self, harness: &str, event: &Value) {
        use crate::telemetry::SkillEvidence;
        // Exact envelope/tool pairs only. New versions must not turn arbitrary
        // nested payloads, response text, or similarly named MCP tools into use.
        let kind = event.get("type").and_then(Value::as_str);
        let mut inputs = Vec::new();
        match harness {
            "codex"
                if kind == Some("item.completed")
                    && event.pointer("/item/type").and_then(Value::as_str)
                        == Some("function_call")
                    && event.pointer("/item/name").and_then(Value::as_str) == Some("skill") =>
            {
                inputs.push((
                    event.pointer("/item/input"),
                    event.pointer("/item/call_id"),
                    None,
                ));
            }
            "claude-code" if kind == Some("assistant") => {
                if let Some(content) = event.pointer("/message/content").and_then(Value::as_array) {
                    for block in content {
                        if block["type"] == "tool_use" && block["name"] == "Skill" {
                            inputs.push((block.get("input"), block.get("id"), None));
                        }
                    }
                }
            }
            "opencode"
                if kind == Some("tool_use")
                    && event.pointer("/part/type").and_then(Value::as_str) == Some("tool")
                    && event.pointer("/part/tool").and_then(Value::as_str) == Some("skill") =>
            {
                let status = self.skill_status(event.pointer("/part/state/status"), false);
                inputs.push((
                    event.pointer("/part/state/input"),
                    event.pointer("/part/callID"),
                    status,
                ));
            }
            "antigravity" if kind == Some("tool_use") && event["name"] == "Skill" => {
                inputs.push((event.get("input"), event.get("id"), None));
            }
            "antigravity" if event["event"] == "step_update" => {
                let step = &event["step_update"];
                // Some envelopes carry both aliases; this is still one report.
                if step["tool_name"] == "Skill"
                    || step
                        .pointer("/tool_info/name")
                        .is_some_and(|v| v == "Skill")
                {
                    let status = self.skill_status(step.get("state"), true);
                    inputs.push((
                        step.pointer("/tool_info/parameters"),
                        step.pointer("/tool_info/id"),
                        status,
                    ));
                }
            }
            _ => (),
        }
        for (input, id, completion) in inputs {
            let name = input
                .and_then(|v| v.get("skill").or_else(|| v.get("name")))
                .and_then(Value::as_str)
                .filter(|name| {
                    !name.is_empty()
                        && name.len() <= 128
                        && name.bytes().all(|b| {
                            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':')
                        })
                        && name
                            .bytes()
                            .next()
                            .is_some_and(|b| b.is_ascii_alphanumeric())
                });
            let Some(name) = name else {
                self.skill_unknown_events = self.skill_unknown_events.saturating_add(1);
                if self.skill_observation != SkillEvidence::Observed {
                    self.skill_observation = SkillEvidence::Unverified;
                }
                continue;
            };
            if let Some(id) = call_id(id) {
                let matches: Vec<usize> = self
                    .skill_calls
                    .iter()
                    .enumerate()
                    .filter(|(_, call)| call.harness == harness && call.id == id)
                    .map(|(index, _)| index)
                    .collect();
                if !matches.is_empty() {
                    let call = &self.skill_calls[matches[0]];
                    let index = call.index;
                    // These adapters report snapshots of the same tool call.
                    if harness == "opencode"
                        || (harness == "antigravity" && event["event"] == "step_update")
                    {
                        let skill = &self.skills[index];
                        if matches.len() == 1 && !call.ambiguous && skill.name == name {
                            if let Some(failed) = completion {
                                let status = if failed { "failed" } else { "completed" };
                                if skill.status == "invoked" {
                                    let elapsed =
                                        call.started.elapsed().as_millis().min(u64::MAX as u128)
                                            as u64;
                                    self.complete_skill(index, failed, Some(elapsed));
                                    continue;
                                }
                                if skill.status == status {
                                    continue;
                                }
                            } else if skill.status == "invoked" {
                                continue;
                            }
                        }
                        self.invalidate_skill_calls(&matches);
                        continue;
                    }
                    // Reused IDs cannot support a unique completion attribution.
                    self.invalidate_skill_calls(&matches);
                }
            }
            if self.skills.len() >= 128 {
                self.failed = true;
                if !self
                    .blockers
                    .iter()
                    .any(|b| b == "skill invocation limit exceeded")
                {
                    self.blockers.push("skill invocation limit exceeded".into());
                }
                break;
            }
            self.skill_observation = SkillEvidence::Observed;
            let index = self.skills.len();
            self.skills.push(SkillInvocation {
                name: name.into(),
                source: None,
                digest: None,
                status: "invoked".into(),
                completed_at: None,
                elapsed_ms: None,
                harness: Some(harness.into()),
                observed_at: Some(task::now_rfc3339()),
                evidence: SkillEvidence::Observed,
                execution: SkillEvidence::Unverified,
            });
            if let Some(id) = call_id(id) {
                self.skill_calls.push(Call {
                    id: id.into(),
                    harness: harness.into(),
                    index,
                    started: Instant::now(),
                    ambiguous: false,
                });
            }
            if let Some(failed) = completion {
                self.complete_skill(index, failed, None);
            }
        }
        self.observe_skill_results(harness, event);
    }

    fn invalidate_skill_calls(&mut self, matches: &[usize]) {
        self.skill_unknown_events = self.skill_unknown_events.saturating_add(1);
        for index in matches {
            let call = &mut self.skill_calls[*index];
            call.ambiguous = true;
            let skill = &mut self.skills[call.index];
            skill.status = "invoked".into();
            skill.execution = crate::telemetry::SkillEvidence::Unverified;
            skill.completed_at = None;
            skill.elapsed_ms = None;
        }
    }

    fn skill_status(&mut self, status: Option<&Value>, nested: bool) -> Option<bool> {
        match (nested, status.and_then(Value::as_str)) {
            (false, Some("completed")) | (true, Some("DONE")) => Some(false),
            (false, Some("error")) | (true, Some("ERROR")) => Some(true),
            (_, None) if status.is_none() => None,
            (false, Some("pending" | "running")) | (true, Some("RUNNING")) => None,
            _ => {
                self.skill_unknown_events = self.skill_unknown_events.saturating_add(1);
                None
            }
        }
    }

    fn complete_skill(&mut self, index: usize, failed: bool, elapsed_ms: Option<u64>) {
        let skill = &mut self.skills[index];
        skill.status = if failed { "failed" } else { "completed" }.into();
        skill.execution = crate::telemetry::SkillEvidence::Observed;
        skill.completed_at = Some(task::now_rfc3339());
        skill.elapsed_ms = elapsed_ms;
    }

    fn observe_skill_results(&mut self, harness: &str, event: &Value) {
        match (harness, event["type"].as_str()) {
            ("codex", Some("item.completed"))
                if event["item"]["type"] == "function_call_output" =>
            {
                self.skill_result(
                    harness,
                    event.pointer("/item/call_id"),
                    event.pointer("/item/is_error"),
                );
            }
            ("claude-code", Some("user")) => {
                if let Some(blocks) = event.pointer("/message/content").and_then(Value::as_array) {
                    for block in blocks {
                        if block["type"] == "tool_result" {
                            self.skill_result(
                                harness,
                                block.get("tool_use_id"),
                                block.get("is_error"),
                            );
                        }
                    }
                }
            }
            ("antigravity", Some("tool_result")) => {
                self.skill_result(harness, event.get("tool_use_id"), event.get("is_error"));
            }
            _ => (),
        }
    }

    fn skill_result(&mut self, harness: &str, id: Option<&Value>, error: Option<&Value>) {
        let matched = call_id(id).and_then(|id| {
            let mut calls = self
                .skill_calls
                .iter()
                .filter(|call| call.harness == harness && call.id == id);
            let call = calls.next()?;
            if call.ambiguous || calls.next().is_some() {
                return None;
            }
            Some((
                call.index,
                call.started.elapsed().as_millis().min(u64::MAX as u128) as u64,
            ))
        });
        if let (Some((index, elapsed)), Some(failed)) = (matched, error.and_then(Value::as_bool)) {
            if self.skills[index].status == "invoked" {
                self.complete_skill(index, failed, Some(elapsed));
                return;
            }
            if self.skills[index].status == if failed { "failed" } else { "completed" } {
                return;
            }
            let matches: Vec<usize> = self
                .skill_calls
                .iter()
                .enumerate()
                .filter(|(_, call)| call.index == index)
                .map(|(index, _)| index)
                .collect();
            self.invalidate_skill_calls(&matches);
            return;
        }
        // Orphan, ambiguous, duplicate, or malformed results remain visible.
        self.skill_unknown_events = self.skill_unknown_events.saturating_add(1);
    }
}

//! Bounded projections of native event streams. No payloads or identifiers are
//! serialized; a repeated tool is not necessarily redundant or unproductive.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const MAX_IDENTITIES: usize = 4096;
const MAX_TOOLS: usize = 256;
const MAX_IDENTIFIER_BYTES: usize = 1024;
type Key = [u8; 32];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    #[default]
    None,
    Partial,
    /// Complete only for the supported shapes in the captured native stream,
    /// never a claim about provider internals or helper work.
    CompleteObservedStream,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    #[default]
    Unsupported,
    CodexTurns,
    ClaudeAssistantMessages,
    OpencodeSteps,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub source: Source,
    pub coverage: Coverage,
    pub steps: Option<u64>,
    pub tool_calls: Option<u64>,
    pub tool_errors: Option<u64>,
    pub repeated_tool_calls: Option<u64>,
    pub repeated_tool_errors: Option<u64>,
    /// Success after an observed error from the same tool; not task recovery.
    pub tool_recoveries: Option<u64>,
    pub malformed_events: u64,
    pub unclassified_events: u64,
    pub tracking_limited: bool,
    pub stream_incomplete: bool,
}

#[derive(Debug, Default)]
struct Tool {
    failed: bool,
    ever_failed: bool,
}
#[derive(Debug)]
struct Call {
    tool: Key,
    finished: bool,
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Observer {
    #[serde(flatten)]
    pub observation: Observation,
    #[serde(skip)]
    calls: BTreeMap<Key, Call>,
    #[serde(skip)]
    tools: BTreeMap<Key, Tool>,
    #[serde(skip)]
    steps: BTreeSet<Key>,
    #[serde(skip)]
    started: bool,
    #[serde(skip)]
    ended: bool,
    #[serde(skip)]
    pending: usize,
}

fn increment(value: &mut Option<u64>) {
    *value = Some(value.unwrap_or(0).saturating_add(1));
}
fn key(value: Option<&str>) -> Option<Key> {
    let value = value.filter(|s| !s.is_empty() && s.len() <= MAX_IDENTIFIER_BYTES)?;
    Some(Sha256::digest(value.as_bytes()).into())
}
fn string<'a>(v: &'a Value, path: &str) -> Option<&'a str> {
    v.pointer(path).and_then(Value::as_str)
}
impl Observer {
    pub fn incomplete(&mut self) {
        self.observation.stream_incomplete = true;
        self.refresh();
    }
    pub fn malformed(&mut self) {
        self.observation.malformed_events = self.observation.malformed_events.saturating_add(1);
        self.incomplete();
    }
    fn unknown(&mut self) {
        self.observation.unclassified_events =
            self.observation.unclassified_events.saturating_add(1);
    }
    fn refresh(&mut self) {
        let o = &mut self.observation;
        o.coverage = if o.source == Source::Unsupported {
            Coverage::None
        } else if self.started
            && self.ended
            && !o.stream_incomplete
            && !o.tracking_limited
            && o.malformed_events == 0
            && o.unclassified_events == 0
            && self.pending == 0
        {
            Coverage::CompleteObservedStream
        } else {
            Coverage::Partial
        };
    }
    fn step(&mut self, id: Option<&str>) {
        let Some(id) = key(id) else {
            self.unknown();
            return;
        };
        if self.steps.contains(&id) {
            return;
        }
        if self.steps.len() == MAX_IDENTITIES {
            self.observation.tracking_limited = true;
            return;
        }
        self.steps.insert(id);
        increment(&mut self.observation.steps);
    }
    fn call(&mut self, id: Option<&str>, name: Option<&str>, outcome: Option<bool>) {
        let (Some(id), Some(tool)) = (key(id), key(name)) else {
            self.unknown();
            return;
        };
        if let Some(call) = self.calls.get(&id) {
            if call.tool != tool {
                self.unknown();
                return;
            }
        } else {
            if self.calls.len() == MAX_IDENTITIES
                || (!self.tools.contains_key(&tool) && self.tools.len() == MAX_TOOLS)
            {
                self.observation.tracking_limited = true;
                return;
            }
            increment(&mut self.observation.tool_calls);
            if self.tools.contains_key(&tool) {
                increment(&mut self.observation.repeated_tool_calls);
            }
            self.tools.entry(tool).or_default();
            self.calls.insert(
                id,
                Call {
                    tool,
                    finished: false,
                },
            );
            self.pending += 1;
        }
        self.finish_call(id, outcome);
    }
    fn finish_call(&mut self, id: Key, outcome: Option<bool>) {
        let Some(failed) = outcome else {
            return;
        };
        let Some(call) = self.calls.get_mut(&id) else {
            self.unknown();
            return;
        };
        if call.finished {
            return;
        }
        call.finished = true;
        self.pending -= 1;
        let tool = self
            .tools
            .get_mut(&call.tool)
            .expect("tracked call has a tool");
        if failed {
            increment(&mut self.observation.tool_errors);
            if tool.ever_failed {
                increment(&mut self.observation.repeated_tool_errors);
            }
            tool.failed = true;
            tool.ever_failed = true;
        } else if tool.failed {
            increment(&mut self.observation.tool_recoveries);
            tool.failed = false;
        }
    }
    pub fn observe(&mut self, harness: &str, v: &Value) {
        let source = match harness {
            "codex" => Source::CodexTurns,
            "claude-code" => Source::ClaudeAssistantMessages,
            "opencode" => Source::OpencodeSteps,
            _ => Source::Unsupported,
        };
        if source == Source::Unsupported {
            return;
        }
        if self.observation.source == Source::Unsupported {
            self.observation.source = source;
            self.observation.steps = Some(0);
            self.observation.tool_calls = Some(0);
            self.observation.tool_errors = Some(0);
            self.observation.repeated_tool_calls = Some(0);
            self.observation.repeated_tool_errors = Some(0);
            self.observation.tool_recoveries = Some(0);
        } else if self.observation.source != source {
            self.incomplete();
            return;
        }
        // Native helper messages are outside the owning agent's trajectory.
        if harness == "claude-code" && v.get("parent_tool_use_id").is_some_and(|p| !p.is_null()) {
            self.unknown();
            self.refresh();
            return;
        }
        let kind = string(v, "/type")
            .or_else(|| string(v, "/event"))
            .unwrap_or("");
        match (harness, kind) {
            ("codex", "thread.started") => self.started = true,
            ("codex", "turn.started") => (),
            ("codex", "turn.completed") => {
                if self.ended {
                    self.unknown();
                }
                increment(&mut self.observation.steps);
                self.ended = true;
            }
            ("codex", "item.started" | "item.updated" | "item.completed") => {
                let item = &v["item"];
                let category = string(item, "/type").unwrap_or("");
                let name = match category {
                    "command_execution" | "file_change" | "web_search" => Some(category.to_owned()),
                    "mcp_tool_call" => string(item, "/server")
                        .zip(string(item, "/tool"))
                        .filter(|(s, t)| s.len() + t.len() < MAX_IDENTIFIER_BYTES)
                        .map(|(s, t)| format!("{}:{s}{t}", s.len())),
                    "agent_message" | "reasoning" | "todo_list" => {
                        self.refresh();
                        return;
                    }
                    _ => {
                        self.unknown();
                        self.refresh();
                        return;
                    }
                };
                let outcome = if kind == "item.completed" {
                    // MCP transport errors and result.isError are distinct;
                    // either is explicit failure evidence.
                    if item.get("error").is_some_and(|e| !e.is_null())
                        || item.pointer("/result/isError").and_then(Value::as_bool) == Some(true)
                        || string(item, "/status") == Some("failed")
                        || item
                            .get("exit_code")
                            .and_then(Value::as_i64)
                            .is_some_and(|c| c != 0)
                    {
                        Some(true)
                    } else if string(item, "/status") == Some("completed")
                        || item.get("exit_code").and_then(Value::as_i64) == Some(0)
                    {
                        Some(false)
                    } else {
                        None
                    }
                } else {
                    None
                };
                self.call(string(item, "/id"), name.as_deref(), outcome);
            }
            ("claude-code", "system") if string(v, "/subtype") == Some("init") => {
                self.started = true
            }
            ("claude-code", "assistant") => {
                self.step(string(v, "/message/id"));
                if let Some(content) = v.pointer("/message/content").and_then(Value::as_array) {
                    for block in content {
                        match string(block, "/type") {
                            Some("tool_use") => {
                                self.call(string(block, "/id"), string(block, "/name"), None)
                            }
                            Some("text" | "thinking" | "redacted_thinking") => (),
                            _ => self.unknown(),
                        }
                    }
                } else {
                    self.unknown();
                }
            }
            ("claude-code", "user") => {
                if let Some(content) = v.pointer("/message/content").and_then(Value::as_array) {
                    for block in content {
                        match string(block, "/type") {
                            Some("tool_result") => {
                                if let Some(id) = key(string(block, "/tool_use_id")) {
                                    // Native tool_result omits is_error on success.
                                    let outcome = match block.get("is_error") {
                                        None => Some(false),
                                        Some(value) => value.as_bool(),
                                    };
                                    self.finish_call(id, outcome);
                                } else {
                                    self.unknown();
                                }
                            }
                            Some("text") => (),
                            _ => self.unknown(),
                        }
                    }
                } else {
                    self.unknown();
                }
            }
            ("claude-code", "result") => self.ended = true,
            // Delta events are intentionally excluded: assistant messages carry
            // the consolidated content and IDs, so counting both would duplicate.
            ("claude-code", "stream_event") => (),
            ("opencode", "step_start") => self.started = true,
            ("opencode", "step_finish") => {
                self.step(string(v, "/part/id"));
                if string(v, "/part/reason") == Some("stop") {
                    self.ended = true;
                }
            }
            ("opencode", "tool_use") => {
                let outcome = match string(v, "/part/state/status") {
                    Some("completed") => Some(false),
                    Some("error") => Some(true),
                    _ => None,
                };
                self.call(
                    string(v, "/part/callID").or_else(|| string(v, "/part/id")),
                    string(v, "/part/tool"),
                    outcome,
                );
            }
            ("opencode", "text" | "reasoning") => (),
            _ => self.unknown(),
        }
        self.refresh();
    }
}

/// Optional case budgets. They score separately from answer correctness.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Budgets {
    pub max_steps: Option<u64>,
    pub max_tool_errors: Option<u64>,
    #[serde(default)]
    pub unknown_coverage: UnknownPolicy,
}
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownPolicy {
    #[default]
    Unknown,
    Fail,
    /// Explicitly judge observed lower bounds even when coverage is partial.
    ObservedOnly,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetStatus {
    NotApplicable,
    Pass,
    Fail,
    Unknown,
}
impl Budgets {
    pub fn score(&self, observation: Option<&Observation>) -> BudgetStatus {
        let o = observation;
        let checks = [
            (self.max_steps, o.and_then(|o| o.steps)),
            (self.max_tool_errors, o.and_then(|o| o.tool_errors)),
        ];
        if checks.iter().all(|(budget, _)| budget.is_none()) {
            return BudgetStatus::NotApplicable;
        }
        if checks
            .iter()
            .any(|(budget, count)| budget.zip(*count).is_some_and(|(b, c)| c > b))
        {
            return BudgetStatus::Fail;
        }
        let missing = checks
            .iter()
            .any(|(budget, count)| budget.is_some() && count.is_none());
        let complete = o.is_some_and(|o| o.coverage == Coverage::CompleteObservedStream);
        if complete && !missing {
            return BudgetStatus::Pass;
        }
        match self.unknown_coverage {
            UnknownPolicy::Fail => BudgetStatus::Fail,
            UnknownPolicy::ObservedOnly if !missing => BudgetStatus::Pass,
            _ => BudgetStatus::Unknown,
        }
    }
}

/// Fixed-size aggregation; means always carry their observation denominators.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub missing_runs: usize,
    pub coverage: BTreeMap<String, usize>,
    pub sources: BTreeMap<String, usize>,
    pub means: BTreeMap<String, f64>,
    pub observations: BTreeMap<String, usize>,
    pub budget_statuses: BTreeMap<String, usize>,
    pub limited_runs: usize,
}
impl Summary {
    pub fn collect<'a>(
        records: impl Iterator<Item = (Option<&'a Observation>, Option<BudgetStatus>)>,
    ) -> Self {
        let mut s = Self::default();
        for (observation, status) in records {
            let status =
                serde_json::to_value(status.unwrap_or(BudgetStatus::NotApplicable)).unwrap();
            *s.budget_statuses
                .entry(status.as_str().unwrap().into())
                .or_default() += 1;
            let Some(o) = observation else {
                s.missing_runs += 1;
                continue;
            };
            for (value, map) in [
                (serde_json::to_value(o.coverage).unwrap(), &mut s.coverage),
                (serde_json::to_value(o.source).unwrap(), &mut s.sources),
            ] {
                *map.entry(value.as_str().unwrap().into()).or_default() += 1;
            }
            s.limited_runs += usize::from(o.tracking_limited);
            for (name, value) in [
                ("steps", o.steps),
                ("tool_calls", o.tool_calls),
                ("tool_errors", o.tool_errors),
                ("repeated_tool_calls", o.repeated_tool_calls),
                ("repeated_tool_errors", o.repeated_tool_errors),
                ("tool_recoveries", o.tool_recoveries),
            ] {
                if let Some(value) = value {
                    *s.means.entry(name.into()).or_default() += value as f64;
                    *s.observations.entry(name.into()).or_default() += 1;
                }
            }
        }
        for (name, mean) in &mut s.means {
            *mean /= s.observations[name] as f64;
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn observe(observer: &mut Observer, harness: &str, events: &[Value]) {
        for event in events {
            observer.observe(harness, event);
        }
    }

    #[test]
    fn codex_deduplicates_lifecycle_and_keeps_error_recovery_evidence() {
        let mut o = Observer::default();
        observe(
            &mut o,
            "codex",
            &[
                json!({"type":"thread.started"}),
                json!({"type":"item.started","item":{"type":"mcp_tool_call","id":"a","server":"synthetic","tool":"probe","arguments":{"private":"secret"}}}),
                json!({"type":"item.completed","item":{"type":"mcp_tool_call","id":"a","server":"synthetic","tool":"probe","status":"completed","result":{"isError":true,"content":"secret"}}}),
                json!({"type":"item.completed","item":{"type":"mcp_tool_call","id":"a","server":"synthetic","tool":"probe","status":"completed","result":{"isError":true}}}),
                json!({"type":"item.completed","item":{"type":"mcp_tool_call","id":"b","server":"synthetic","tool":"probe","status":"failed","error":{"message":"secret"}}}),
                json!({"type":"item.completed","item":{"type":"mcp_tool_call","id":"c","server":"synthetic","tool":"probe","status":"completed"}}),
                json!({"type":"item.completed","item":{"type":"reasoning","text":"secret"}}),
                json!({"type":"turn.completed"}),
            ],
        );
        let s = &o.observation;
        assert_eq!(s.coverage, Coverage::CompleteObservedStream);
        assert_eq!(s.steps, Some(1));
        assert_eq!(s.tool_calls, Some(3));
        assert_eq!(s.tool_errors, Some(2));
        assert_eq!(s.repeated_tool_calls, Some(2));
        assert_eq!(s.repeated_tool_errors, Some(1));
        assert_eq!(s.tool_recoveries, Some(1));
        let stored = serde_json::to_string(&o).unwrap();
        for private in [
            "secret",
            "synthetic",
            "probe",
            "arguments",
            "content",
            "reasoning",
        ] {
            assert!(!stored.contains(private), "{stored}");
        }
        serde_json::from_str::<Observation>(&stored).unwrap();
    }

    #[test]
    fn claude_counts_unique_messages_and_correlated_results_not_deltas() {
        let mut o = Observer::default();
        let message = json!({"type":"assistant","message":{"id":"m1","content":[{"type":"tool_use","id":"a","name":"Read","input":{"path":"secret"}}]}});
        observe(
            &mut o,
            "claude-code",
            &[
                json!({"type":"system","subtype":"init"}),
                json!({"type":"stream_event","event":{"type":"content_block_delta"}}),
                message.clone(),
                message,
                json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"a","is_error":true,"content":"secret"}]}}),
                json!({"type":"assistant","message":{"id":"m2","content":[{"type":"tool_use","id":"b","name":"Read"}]}}),
                json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"b","content":"secret"}]}}),
                json!({"type":"result"}),
            ],
        );
        assert_eq!(o.observation.steps, Some(2));
        assert_eq!(o.observation.tool_calls, Some(2));
        assert_eq!(o.observation.tool_errors, Some(1));
        assert_eq!(o.observation.tool_recoveries, Some(1));
        assert_eq!(o.observation.coverage, Coverage::CompleteObservedStream);
    }

    #[test]
    fn opencode_deduplicates_steps_and_call_states() {
        let mut o = Observer::default();
        let finish = json!({"type":"step_finish","part":{"id":"s1","reason":"tool-calls"}});
        observe(
            &mut o,
            "opencode",
            &[
                json!({"type":"step_start"}),
                finish.clone(),
                finish,
                json!({"type":"tool_use","part":{"callID":"a","tool":"read","state":{"status":"running"}}}),
                json!({"type":"tool_use","part":{"callID":"a","tool":"read","state":{"status":"error","error":"secret"}}}),
                json!({"type":"tool_use","part":{"callID":"b","tool":"read","state":{"status":"completed","output":"secret"}}}),
                json!({"type":"step_finish","part":{"id":"s2","reason":"stop"}}),
            ],
        );
        assert_eq!(o.observation.steps, Some(2));
        assert_eq!(o.observation.tool_calls, Some(2));
        assert_eq!(o.observation.tool_errors, Some(1));
        assert_eq!(o.observation.tool_recoveries, Some(1));
        assert_eq!(o.observation.coverage, Coverage::CompleteObservedStream);
    }

    #[test]
    fn unsupported_missing_partial_malformed_and_zero_are_distinct() {
        let mut unsupported = Observer::default();
        unsupported.observe(
            "antigravity",
            &json!({"event":"step_update","step_update":{"state":"DONE"}}),
        );
        assert_eq!(unsupported.observation.coverage, Coverage::None);
        assert_eq!(unsupported.observation.tool_calls, None);
        let mut o = Observer::default();
        o.observe("codex", &json!({"type":"thread.started"}));
        assert_eq!(o.observation.tool_calls, Some(0));
        assert_eq!(o.observation.coverage, Coverage::Partial);
        o.observe("codex", &json!({"type":"turn.completed"}));
        assert_eq!(o.observation.coverage, Coverage::CompleteObservedStream);
        o.malformed();
        assert_eq!(o.observation.coverage, Coverage::Partial);
        assert_eq!(o.observation.malformed_events, 1);
        assert_eq!(o.observation.tool_errors, Some(0));
        let mut pending = Observer::default();
        observe(
            &mut pending,
            "codex",
            &[
                json!({"type":"thread.started"}),
                json!({"type":"item.started","item":{"type":"command_execution","id":"a"}}),
                json!({"type":"turn.completed"}),
            ],
        );
        assert_eq!(pending.observation.tool_calls, Some(1));
        assert_eq!(pending.observation.coverage, Coverage::Partial);
        pending.observe("codex", &json!({"type":"future_shape"}));
        assert_eq!(pending.observation.unclassified_events, 1);
    }

    #[test]
    fn bounds_mark_loss_without_retaining_unbounded_names_or_ids() {
        let mut o = Observer::default();
        for i in 0..MAX_IDENTITIES + 10 {
            o.observe("opencode", &json!({"type":"tool_use","part":{"callID":i.to_string(),"tool":"read","state":{"status":"completed"}}}));
        }
        assert_eq!(o.calls.len(), MAX_IDENTITIES);
        assert_eq!(o.observation.tool_calls, Some(MAX_IDENTITIES as u64));
        assert!(o.observation.tracking_limited);
        let mut o = Observer::default();
        for i in 0..MAX_TOOLS + 10 {
            o.call(Some(&i.to_string()), Some(&i.to_string()), Some(false));
        }
        assert_eq!(o.tools.len(), MAX_TOOLS);
        assert!(o.observation.tracking_limited);
        o.call(
            Some(&"x".repeat(MAX_IDENTIFIER_BYTES + 1)),
            Some("read"),
            None,
        );
        assert_eq!(o.observation.unclassified_events, 1);
    }

    #[test]
    fn helper_events_and_malformed_call_shapes_cannot_claim_complete_coverage() {
        let mut o = Observer::default();
        observe(
            &mut o,
            "claude-code",
            &[
                json!({"type":"system","subtype":"init"}),
                json!({"type":"assistant","parent_tool_use_id":"helper","message":{"id":"m1","content":[{"type":"tool_use","id":"a","name":"Read"}]}}),
                json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"unmatched"}]}}),
                json!({"type":"result"}),
            ],
        );
        assert_eq!(o.observation.steps, Some(0));
        assert_eq!(o.observation.tool_calls, Some(0));
        assert_eq!(o.observation.coverage, Coverage::Partial);
        assert_eq!(o.observation.unclassified_events, 2);
        let mut o = Observer::default();
        for i in 0..MAX_IDENTITIES + 1 {
            o.step(Some(&i.to_string()));
        }
        assert_eq!(o.steps.len(), MAX_IDENTITIES);
        assert!(o.observation.tracking_limited);
        // Call identity collisions leave uncertainty rather than inventing work.
        o.call(Some("call"), Some("Read"), None);
        o.call(Some("call"), Some("Write"), Some(true));
        assert_eq!(o.observation.tool_calls, Some(1));
        assert_eq!(o.observation.unclassified_events, 1);
    }

    #[test]
    fn budgets_fail_observed_excess_and_make_unknown_policy_explicit() {
        let mut budget = Budgets {
            max_steps: Some(2),
            max_tool_errors: Some(0),
            unknown_coverage: UnknownPolicy::Unknown,
        };
        assert_eq!(budget.score(None), BudgetStatus::Unknown);
        let mut o = Observation {
            steps: Some(2),
            tool_errors: Some(0),
            coverage: Coverage::Partial,
            ..Observation::default()
        };
        assert_eq!(budget.score(Some(&o)), BudgetStatus::Unknown);
        budget.unknown_coverage = UnknownPolicy::Fail;
        assert_eq!(budget.score(Some(&o)), BudgetStatus::Fail);
        budget.unknown_coverage = UnknownPolicy::ObservedOnly;
        assert_eq!(budget.score(Some(&o)), BudgetStatus::Pass);
        assert_eq!(budget.score(None), BudgetStatus::Unknown);
        o.tool_errors = Some(1);
        assert_eq!(budget.score(Some(&o)), BudgetStatus::Fail);
        budget.unknown_coverage = UnknownPolicy::Unknown;
        assert_eq!(budget.score(Some(&o)), BudgetStatus::Fail);
        o.tool_errors = Some(0);
        o.coverage = Coverage::CompleteObservedStream;
        assert_eq!(budget.score(Some(&o)), BudgetStatus::Pass);
    }
}

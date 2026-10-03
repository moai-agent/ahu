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
    /// Calls with an observed, classified terminal outcome. Absent in legacy records.
    #[serde(default)]
    pub completed_tool_calls: Option<u64>,
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

impl Observation {
    /// A rate needs a known numerator and completed-call denominator. Never
    /// infer completions from starts, including for legacy records.
    pub fn tool_error_rate(&self) -> Option<f64> {
        if self.coverage == Coverage::None || self.source == Source::Unsupported {
            return None;
        }
        let calls = self.tool_calls?;
        let completed = self.completed_tool_calls?;
        let errors = self.tool_errors?;
        if errors > completed || completed > calls {
            return None;
        }
        if self.complete() && completed != calls {
            return None;
        }
        if completed == 0 {
            return self.complete().then_some(0.0);
        }
        Some(errors as f64 / completed as f64)
    }

    pub fn complete(&self) -> bool {
        self.coverage == Coverage::CompleteObservedStream
            && self.source != Source::Unsupported
            && !self.tracking_limited
            && !self.stream_incomplete
            && self.malformed_events == 0
            && self.unclassified_events == 0
    }
}

/// Validated finite fraction; NaN cannot enter through CLI, YAML or JSON.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct ErrorRate(f64);
impl Eq for ErrorRate {}
impl TryFrom<f64> for ErrorRate {
    type Error = crate::util::Error;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        if value.is_finite() && (0.0..=1.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(
                crate::util::Error::new("max tool error rate must be finite and in 0..1")
                    .with_kind(crate::util::ErrorKind::Usage),
            )
        }
    }
}
impl From<ErrorRate> for f64 {
    fn from(value: ErrorRate) -> f64 {
        value.0
    }
}
impl std::str::FromStr for ErrorRate {
    type Err = crate::util::Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.parse::<f64>().map_err(|_| {
            crate::util::Error::new("max tool error rate must be finite and in 0..1")
                .with_kind(crate::util::ErrorKind::Usage)
        })?;
        Self::try_from(value)
    }
}

/// Opt-in CI limits. Case budgets remain separately scored under their own
/// policy; effective CLI limits can only tighten them and require coverage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Guardrails {
    pub max_steps: Option<u32>,
    pub max_tool_error_rate: Option<ErrorRate>,
}
impl Guardrails {
    pub fn enabled(&self) -> bool {
        self.max_steps.is_some() || self.max_tool_error_rate.is_some()
    }

    pub fn effective(&self, case: Option<&Budgets>) -> Option<Budgets> {
        if !self.enabled() {
            return None;
        }
        let mut effective = case.cloned().unwrap_or_default();
        if let Some(maximum) = self.max_steps {
            effective.max_steps = Some(
                effective
                    .max_steps
                    .map_or(u64::from(maximum), |case| case.min(u64::from(maximum))),
            );
        }
        if let Some(maximum) = self.max_tool_error_rate {
            effective.max_tool_error_rate = Some(
                effective
                    .max_tool_error_rate
                    .map_or(maximum, |case| ErrorRate(case.0.min(maximum.0))),
            );
        }
        // Unknown remains distinct in the record, and fails the CLI exit gate.
        // A case's observed_only policy never weakens CLI coverage requirements.
        effective.unknown_coverage = UnknownPolicy::Unknown;
        Some(effective)
    }
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
        increment(&mut self.observation.completed_tool_calls);
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
            self.observation.completed_tool_calls = Some(0);
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
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Budgets {
    pub max_steps: Option<u64>,
    pub max_tool_errors: Option<u64>,
    pub max_tool_error_rate: Option<ErrorRate>,
    #[serde(default)]
    pub unknown_coverage: UnknownPolicy,
}
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownPolicy {
    #[default]
    Unknown,
    Fail,
    /// Explicitly judge observed counts and rates even when coverage is partial.
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
        if checks.iter().all(|(budget, _)| budget.is_none()) && self.max_tool_error_rate.is_none() {
            return BudgetStatus::NotApplicable;
        }
        if checks
            .iter()
            .any(|(budget, count)| budget.zip(*count).is_some_and(|(b, c)| c > b))
        {
            return BudgetStatus::Fail;
        }
        let rate = o.and_then(Observation::tool_error_rate);
        let complete = o.is_some_and(Observation::complete);
        // Unlike counts, a partial-stream rate is not a lower bound.
        if (complete || matches!(self.unknown_coverage, UnknownPolicy::ObservedOnly))
            && self
                .max_tool_error_rate
                .zip(rate)
                .is_some_and(|(limit, rate)| rate > limit.0)
        {
            return BudgetStatus::Fail;
        }
        let missing = (self.max_tool_error_rate.is_some() && rate.is_none())
            || checks
                .iter()
                .any(|(budget, count)| budget.is_some() && count.is_none());
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
    pub guard_statuses: BTreeMap<String, usize>,
    /// Rate coverage is separate: old observations may lack completions.
    pub tool_error_rate_coverage: BTreeMap<String, usize>,
}
impl Summary {
    pub fn collect<'a>(
        records: impl Iterator<
            Item = (
                Option<&'a Observation>,
                Option<BudgetStatus>,
                Option<BudgetStatus>,
            ),
        >,
    ) -> Self {
        let mut s = Self::default();
        for (observation, status, guard) in records {
            let guard = serde_json::to_value(guard.unwrap_or(BudgetStatus::NotApplicable)).unwrap();
            *s.guard_statuses
                .entry(guard.as_str().unwrap().into())
                .or_default() += 1;
            let rate = observation.and_then(Observation::tool_error_rate);
            let rate_coverage = match (rate, observation) {
                (Some(_), Some(o)) if o.complete() => "complete_observed_stream",
                (Some(_), _) => "partial",
                _ => "unknown",
            };
            *s.tool_error_rate_coverage
                .entry(rate_coverage.into())
                .or_default() += 1;
            // Aggregate only complete rates. Partial rates are visible on records
            // with their coverage, never mixed into the complete-run mean.
            if rate_coverage == "complete_observed_stream" {
                *s.means.entry("tool_error_rate".into()).or_default() += rate.unwrap();
                *s.observations.entry("tool_error_rate".into()).or_default() += 1;
            }
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
                ("completed_tool_calls", o.completed_tool_calls),
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
            max_tool_error_rate: None,
            unknown_coverage: UnknownPolicy::Unknown,
        };
        assert_eq!(budget.score(None), BudgetStatus::Unknown);
        let mut o = Observation {
            source: Source::CodexTurns,
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

    fn complete(calls: u64, errors: u64) -> Observation {
        Observation {
            source: Source::CodexTurns,
            coverage: Coverage::CompleteObservedStream,
            steps: Some(2),
            tool_calls: Some(calls),
            completed_tool_calls: Some(calls),
            tool_errors: Some(errors),
            ..Default::default()
        }
    }

    #[test]
    fn rate_uses_completed_calls_and_deduplicates_terminal_events() {
        let mut o = Observer::default();
        observe(
            &mut o,
            "codex",
            &[
                json!({"type":"thread.started"}),
                json!({"type":"item.started","item":{"type":"command_execution","id":"pending"}}),
                json!({"type":"item.completed","item":{"type":"command_execution","id":"failed","exit_code":1}}),
                json!({"type":"item.completed","item":{"type":"command_execution","id":"failed","exit_code":1}}),
                json!({"type":"turn.completed"}),
            ],
        );
        assert_eq!(o.observation.tool_calls, Some(2));
        assert_eq!(o.observation.completed_tool_calls, Some(1));
        assert_eq!(o.observation.tool_error_rate(), Some(1.0));
        assert_eq!(o.observation.coverage, Coverage::Partial);
        o.observe("codex", &json!({"type":"item.completed","item":{"type":"command_execution","id":"pending","exit_code":0}}));
        assert_eq!(o.observation.completed_tool_calls, Some(2));
        assert_eq!(o.observation.tool_error_rate(), Some(0.5));
        assert!(o.observation.complete());
    }

    #[test]
    fn rate_rejects_invalid_counts_and_keeps_legacy_and_partial_zero_unknown() {
        assert_eq!(complete(0, 0).tool_error_rate(), Some(0.0));
        assert_eq!(complete(4, 1).tool_error_rate(), Some(0.25));
        for (calls, completed, errors) in [(0, 0, 1), (1, 2, 1), (2, 1, 2), (2, 1, 0)] {
            let mut o = complete(calls, errors);
            o.completed_tool_calls = Some(completed);
            assert_eq!(o.tool_error_rate(), None);
        }
        let mut o = complete(0, 0);
        o.coverage = Coverage::Partial;
        assert_eq!(o.tool_error_rate(), None);
        let mut unsupported = complete(2, 1);
        unsupported.source = Source::Unsupported;
        assert_eq!(unsupported.tool_error_rate(), None);
        unsupported.source = Source::CodexTurns;
        unsupported.coverage = Coverage::None;
        assert_eq!(unsupported.tool_error_rate(), None);
        let mut legacy = serde_json::to_value(complete(2, 1)).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("completed_tool_calls");
        let legacy: Observation = serde_json::from_value(legacy).unwrap();
        assert_eq!(legacy.tool_error_rate(), None);
        for field in ["tool_errors", "tool_calls"] {
            let mut value = serde_json::to_value(complete(2, 1)).unwrap();
            value[field] = Value::Null;
            assert_eq!(
                serde_json::from_value::<Observation>(value)
                    .unwrap()
                    .tool_error_rate(),
                None
            );
        }
    }

    #[test]
    fn guards_tighten_case_limits_and_require_complete_evidence() {
        let case = Budgets {
            max_steps: Some(2),
            max_tool_errors: Some(1),
            max_tool_error_rate: Some(ErrorRate(0.25)),
            unknown_coverage: UnknownPolicy::ObservedOnly,
        };
        for (steps, rate, expected_steps, expected_rate) in [(8, 0.75, 2, 0.25), (1, 0.1, 1, 0.1)] {
            let guards = Guardrails {
                max_steps: Some(steps),
                max_tool_error_rate: Some(ErrorRate(rate)),
            };
            let effective = guards.effective(Some(&case)).unwrap();
            assert_eq!(effective.max_steps, Some(expected_steps));
            assert_eq!(
                effective.max_tool_error_rate,
                Some(ErrorRate(expected_rate))
            );
            assert_eq!(effective.max_tool_errors, Some(1));
            assert!(matches!(effective.unknown_coverage, UnknownPolicy::Unknown));
        }
        assert!(Guardrails::default().effective(Some(&case)).is_none());
        let guards = Guardrails {
            max_steps: Some(8),
            max_tool_error_rate: Some(ErrorRate(0.5)),
        };
        let effective = guards.effective(Some(&case)).unwrap();
        assert_eq!(effective.score(Some(&complete(4, 1))), BudgetStatus::Pass);
        assert_eq!(effective.score(Some(&complete(2, 1))), BudgetStatus::Fail);
        assert_eq!(effective.score(None), BudgetStatus::Unknown);
        let mut partial = complete(4, 1);
        partial.coverage = Coverage::Partial;
        assert_eq!(case.score(Some(&partial)), BudgetStatus::Pass);
        assert_eq!(effective.score(Some(&partial)), BudgetStatus::Unknown);
        // An observed rate above the maximum may fall as pending calls finish.
        partial.completed_tool_calls = Some(1);
        assert_eq!(effective.score(Some(&partial)), BudgetStatus::Unknown);
        assert_eq!(case.score(Some(&partial)), BudgetStatus::Fail);
        partial.steps = Some(9);
        assert_eq!(effective.score(Some(&partial)), BudgetStatus::Fail);
        for loss in 0..4 {
            let mut incomplete = complete(4, 1);
            match loss {
                0 => incomplete.stream_incomplete = true,
                1 => incomplete.tracking_limited = true,
                2 => incomplete.malformed_events = 1,
                _ => incomplete.unclassified_events = 1,
            }
            assert_eq!(effective.score(Some(&incomplete)), BudgetStatus::Unknown);
        }
    }

    #[test]
    fn rates_validate_through_all_input_formats() {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1, 1.1] {
            assert!(ErrorRate::try_from(invalid).is_err());
        }
        for invalid in ["null", "-0.1", "1.1", "\"0.5\""] {
            assert!(serde_json::from_str::<ErrorRate>(invalid).is_err());
        }
        for valid in [0.0, 0.5, 1.0] {
            let rate = ErrorRate::try_from(valid).unwrap();
            assert_eq!(serde_json::to_value(rate).unwrap(), json!(valid));
        }
    }
}

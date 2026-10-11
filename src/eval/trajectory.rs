//! Bounded projections of native event streams. No payloads or identifiers are
//! serialized; a repeated tool is not necessarily redundant or unproductive.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const MAX_IDENTITIES: usize = 4096;
const MAX_TOOLS: usize = 256;
const MAX_KIRO_SUMMARIES: usize = 32;
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
    KiroSessionUpdates,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub source: Source,
    pub coverage: Coverage,
    /// Independent partial metering, never model steps, tokens or USD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kiro_metering: Option<KiroMetering>,
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

/// Kiro V3 session-info evidence. Amounts are sums of accepted observations,
/// not complete attempt/session accounting. Context is a window snapshot.
/// No native identifiers, tool names, breakdown contents or payloads persist.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KiroMetering {
    pub context_usage_percent: Option<f64>,
    pub peak_context_usage_percent: Option<f64>,
    /// Prompt completions may contain multiple model requests. Not `steps`.
    pub prompt_completions: Option<u64>,
    pub observed_credits: Option<f64>,
    pub credit_observations: u64,
    pub observed_elapsed_ms: Option<f64>,
    pub elapsed_observations: u64,
    pub duplicate_completions: u64,
    pub rejected_updates: u64,
    /// Includes replay before a live prompt marker and unanchored completions.
    pub unattributed_completions: u64,
}

impl KiroMetering {
    pub fn valid(&self) -> bool {
        [self.context_usage_percent, self.peak_context_usage_percent]
            .into_iter()
            .flatten()
            .all(|v| v.is_finite() && (0.0..=100.0).contains(&v))
            && [self.observed_credits, self.observed_elapsed_ms]
                .into_iter()
                .flatten()
                .all(|v| v.is_finite() && v >= 0.0)
            && (self.context_usage_percent.is_some() == self.peak_context_usage_percent.is_some())
            && self
                .context_usage_percent
                .zip(self.peak_context_usage_percent)
                .is_none_or(|(last, peak)| last <= peak)
            && (self.observed_credits.is_some() == (self.credit_observations > 0))
            && (self.observed_elapsed_ms.is_some() == (self.elapsed_observations > 0))
            && self.credit_observations <= self.prompt_completions.unwrap_or(0)
            && self.elapsed_observations <= self.prompt_completions.unwrap_or(0)
    }
}

#[derive(Debug, Default)]
struct KiroTracking {
    // A new user-message assignment is emitted by the live prompt path, not
    // historical replay. Each Observer belongs to one headless attempt.
    prompt: Option<Key>,
    active_turn: Option<Key>,
    turns: BTreeMap<Key, Option<Key>>,
    request_ids: BTreeSet<Key>,
    replay_turns: BTreeSet<Key>,
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
    #[serde(skip)]
    kiro_session: Option<Key>,
    #[serde(skip)]
    kiro_calls: BTreeMap<Key, Option<bool>>,
    #[serde(skip)]
    kiro_tracking: KiroTracking,
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
fn finite_nonnegative(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v >= 0.0)
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
        } else if o.source == Source::KiroSessionUpdates {
            // ACP call outcomes are observable, but model turns and native
            // helper attribution are not. Never certify whole-stream coverage.
            Coverage::Partial
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
    fn kiro_reject_metering(&mut self) {
        let m = self.observation.kiro_metering.get_or_insert_default();
        m.rejected_updates = m.rejected_updates.saturating_add(1);
    }
    fn kiro_info(&mut self, update: &Value) {
        let meta = &update["_meta"]["kiro"];
        let kind = string(meta, "/kind").unwrap_or("");
        if !matches!(
            kind,
            "context_usage" | "user_message_id_assigned" | "turn_start" | "turn_completion"
        ) {
            return;
        }
        if !self.started
            || self.ended
            || self.observation.stream_incomplete
            || meta.get("agentSubtaskId").is_some_and(|v| !v.is_null())
        {
            self.kiro_reject_metering();
            return;
        }
        match kind {
            "context_usage" => {
                let Some(percent) =
                    finite_nonnegative(meta.get("usagePercentage")).filter(|v| *v <= 100.0)
                else {
                    self.kiro_reject_metering();
                    return;
                };
                let m = self.observation.kiro_metering.get_or_insert_default();
                m.context_usage_percent = Some(percent);
                m.peak_context_usage_percent = Some(
                    m.peak_context_usage_percent
                        .map_or(percent, |v| v.max(percent)),
                );
            }
            "user_message_id_assigned" => {
                let Some(id) = key(string(meta, "/userMessageId")) else {
                    self.incomplete();
                    self.kiro_reject_metering();
                    return;
                };
                if self.kiro_tracking.prompt.is_some_and(|prior| prior != id) {
                    // Stream-json runs one submitted prompt. A second identity
                    // makes subsequent attempt attribution ambiguous.
                    self.incomplete();
                    self.kiro_reject_metering();
                    return;
                }
                self.kiro_tracking.prompt = Some(id);
            }
            "turn_start" => {
                // The installed ACP adapter emits a stable messageId for starts
                // (also on replay), but no executionId on turn_completion.
                let Some(id) = key(string(meta, "/messageId")) else {
                    self.kiro_tracking.active_turn = None;
                    self.kiro_reject_metering();
                    return;
                };
                if self.kiro_tracking.prompt.is_none() {
                    if self.kiro_tracking.replay_turns.len() < MAX_IDENTITIES {
                        self.kiro_tracking.replay_turns.insert(id);
                    } else {
                        self.observation.tracking_limited = true;
                        self.incomplete();
                    }
                    return;
                }
                if self.kiro_tracking.replay_turns.contains(&id) {
                    self.kiro_tracking.active_turn = None;
                    return;
                }
                if !self.kiro_tracking.turns.contains_key(&id) {
                    if self.kiro_tracking.turns.len() == MAX_IDENTITIES {
                        self.observation.tracking_limited = true;
                        self.kiro_tracking.active_turn = None;
                        self.kiro_reject_metering();
                        return;
                    }
                    self.kiro_tracking.turns.insert(id, None);
                }
                self.kiro_tracking.active_turn = Some(id);
            }
            "turn_completion" => self.kiro_completion(meta),
            _ => (),
        }
    }
    fn kiro_completion(&mut self, meta: &Value) {
        let Some(turn) = self.kiro_tracking.active_turn else {
            let m = self.observation.kiro_metering.get_or_insert_default();
            m.unattributed_completions = m.unattributed_completions.saturating_add(1);
            return;
        };
        let Some(status @ ("success" | "failed" | "aborted")) = string(meta, "/status") else {
            self.kiro_reject_metering();
            return;
        };
        // Parse only observed wire fields; absence is not zero. Empty summaries
        // occur on aborts and do not establish zero billed usage.
        let mut credits = None;
        if let Some(summaries) = meta.get("promptTurnSummaries") {
            let Some(summaries) = summaries
                .as_array()
                .filter(|s| s.len() <= MAX_KIRO_SUMMARIES)
            else {
                self.kiro_reject_metering();
                return;
            };
            for summary in summaries {
                // Native aggregation emits at most one entry per unit. Reject
                // duplicate units rather than adding an ambiguous amount.
                if summary.get("unit").and_then(Value::as_str) != Some("credit")
                    || credits.is_some()
                {
                    self.kiro_reject_metering();
                    return;
                }
                let Some(amount) = finite_nonnegative(summary.get("usage")) else {
                    self.kiro_reject_metering();
                    return;
                };
                credits = Some(amount);
            }
        }
        let elapsed = finite_nonnegative(meta.get("elapsedTime"));
        if meta.get("elapsedTime").is_some() && elapsed.is_none() {
            self.kiro_reject_metering();
            return;
        }
        let mut requests = BTreeSet::new();
        if let Some(ids) = meta.get("requestIds") {
            let Some(ids) = ids.as_array().filter(|ids| ids.len() <= MAX_IDENTITIES) else {
                self.kiro_reject_metering();
                return;
            };
            for id in ids {
                let Some(id) = key(id.as_str()) else {
                    self.kiro_reject_metering();
                    return;
                };
                if !requests.insert(id) {
                    self.kiro_reject_metering();
                    return;
                }
            }
        }
        // Fingerprint only bounded, projected fields. Unknown metadata and tool
        // names cannot cause duplicate billing or leak into the result.
        let mut digest = Sha256::new();
        digest.update(status.as_bytes());
        for amount in [credits, elapsed] {
            digest.update([u8::from(amount.is_some())]);
            digest.update(amount.unwrap_or(0.0).to_le_bytes());
        }
        for id in &requests {
            digest.update(id);
        }
        let digest: Key = digest.finalize().into();
        if let Some(prior) = self.kiro_tracking.turns[&turn] {
            if prior == digest {
                let m = self.observation.kiro_metering.get_or_insert_default();
                m.duplicate_completions = m.duplicate_completions.saturating_add(1);
            } else {
                self.kiro_reject_metering();
            }
            return;
        }
        if requests
            .iter()
            .any(|id| self.kiro_tracking.request_ids.contains(id))
        {
            self.kiro_reject_metering();
            return;
        }
        if self.kiro_tracking.request_ids.len() + requests.len() > MAX_IDENTITIES {
            self.observation.tracking_limited = true;
            self.kiro_reject_metering();
            return;
        }
        let m = self.observation.kiro_metering.get_or_insert_default();
        let credit_sum = credits.map(|v| m.observed_credits.unwrap_or(0.0) + v);
        let elapsed_sum = elapsed.map(|v| m.observed_elapsed_ms.unwrap_or(0.0) + v);
        if credit_sum.is_some_and(|v| !v.is_finite()) || elapsed_sum.is_some_and(|v| !v.is_finite())
        {
            self.kiro_reject_metering();
            return;
        }
        self.kiro_tracking.turns.insert(turn, Some(digest));
        self.kiro_tracking.request_ids.extend(requests);
        increment(&mut m.prompt_completions);
        if let Some(sum) = credit_sum {
            m.observed_credits = Some(sum);
            m.credit_observations += 1;
        }
        if let Some(sum) = elapsed_sum {
            m.observed_elapsed_ms = Some(sum);
            m.elapsed_observations += 1;
        }
    }
    fn observe_kiro(&mut self, v: &Value) {
        if self.observation.source == Source::Unsupported {
            self.observation.source = Source::KiroSessionUpdates;
            self.observation.tool_calls = Some(0);
            self.observation.completed_tool_calls = Some(0);
            self.observation.tool_errors = Some(0);
        } else if self.observation.source != Source::KiroSessionUpdates {
            self.incomplete();
            return;
        }
        let kind = string(v, "/type").unwrap_or("");
        if kind == "runStarted" {
            if v.pointer("/data/engine").and_then(Value::as_str) != Some("v3")
                || v.pointer("/data/payloadSchema").and_then(Value::as_str) != Some("acp")
                || self.started
            {
                self.incomplete();
            }
            self.started = true;
            return;
        }
        if matches!(kind, "sessionUpdate" | "runFinished" | "runError") {
            let session = string(v, "/data/sessionId");
            let Some(session_key) = session
                .filter(|s| {
                    s.starts_with("sess_")
                        && (6..=128).contains(&s.len())
                        && s.bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
                })
                .and_then(|s| key(Some(s)))
            else {
                self.unknown();
                self.incomplete();
                return;
            };
            if self.kiro_session.is_some_and(|prior| prior != session_key) {
                self.unknown();
                self.incomplete();
                return;
            }
            self.kiro_session = Some(session_key);
        }
        match kind {
            "runFinished" => {
                if self.ended
                    || !self.started
                    || v.pointer("/data/status").and_then(Value::as_str) != Some("success")
                    || v.pointer("/data/stopReason").and_then(Value::as_str) != Some("end_turn")
                    || !v.pointer("/data/finalText").is_some_and(Value::is_string)
                    || v.pointer("/data/finalTextTruncated")
                        .and_then(Value::as_bool)
                        != Some(false)
                {
                    self.incomplete();
                }
                self.ended = true;
            }
            "sessionUpdate" => {
                if !self.started || self.ended {
                    self.incomplete();
                }
                let update = &v["data"]["update"];
                // Historical replay is not work performed by this attempt,
                // including tool outcomes and live-prompt attribution markers.
                if update
                    .pointer("/_meta/kiro/replay")
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    return;
                }
                match string(update, "/sessionUpdate").unwrap_or("") {
                    "tool_call" | "tool_call_update" => {
                        // ACP updates may omit title/kind; correlate only by the
                        // required ID. Titles are display text, not tool identity
                        // or proof that a call delegated work to a native helper.
                        let Some(id) = key(string(update, "/toolCallId")) else {
                            self.unknown();
                            return;
                        };
                        if !self.kiro_calls.contains_key(&id) {
                            if self.kiro_calls.len() == MAX_IDENTITIES {
                                self.observation.tracking_limited = true;
                                return;
                            }
                            self.kiro_calls.insert(id, None);
                            increment(&mut self.observation.tool_calls);
                        }
                        let outcome = match string(update, "/status") {
                            Some("completed") => Some(false),
                            Some("failed") => Some(true),
                            Some("pending" | "in_progress") => None,
                            None if update.get("status").is_none() => None,
                            _ => {
                                self.unknown();
                                return;
                            }
                        };
                        if let Some(failed) = outcome {
                            match self.kiro_calls.get(&id).copied().flatten() {
                                Some(prior) if prior != failed => self.unknown(),
                                Some(_) => (),
                                None => {
                                    self.kiro_calls.insert(id, Some(failed));
                                    increment(&mut self.observation.completed_tool_calls);
                                    if failed {
                                        increment(&mut self.observation.tool_errors);
                                    }
                                }
                            }
                        }
                    }
                    "session_info_update" => self.kiro_info(update),
                    "agent_message_chunk"
                    | "agent_thought_chunk"
                    | "user_message_chunk"
                    | "available_commands_update"
                    | "config_option_update"
                    | "plan" => (),
                    _ => self.unknown(),
                }
            }
            "error" | "runError" => self.incomplete(),
            _ => self.unknown(),
        }
    }
    pub fn observe(&mut self, harness: &str, v: &Value) {
        let source = match harness {
            "codex" => Source::CodexTurns,
            "claude-code" => Source::ClaudeAssistantMessages,
            "opencode" => Source::OpencodeSteps,
            "kiro" => Source::KiroSessionUpdates,
            _ => Source::Unsupported,
        };
        if source == Source::Unsupported {
            return;
        }
        if harness == "kiro" {
            self.observe_kiro(v);
            self.refresh();
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
            if let Some(m) = &o.kiro_metering
                && o.source == Source::KiroSessionUpdates
                && m.valid()
            {
                for (name, value) in [
                    ("kiro_context_usage_percent", m.context_usage_percent),
                    (
                        "kiro_peak_context_usage_percent",
                        m.peak_context_usage_percent,
                    ),
                    (
                        "kiro_prompt_completions",
                        m.prompt_completions.map(|v| v as f64),
                    ),
                    ("kiro_observed_credits", m.observed_credits),
                    (
                        "kiro_credit_observations",
                        Some(m.credit_observations as f64),
                    ),
                    ("kiro_observed_elapsed_ms", m.observed_elapsed_ms),
                    (
                        "kiro_elapsed_observations",
                        Some(m.elapsed_observations as f64),
                    ),
                    (
                        "kiro_duplicate_completions",
                        Some(m.duplicate_completions as f64),
                    ),
                    ("kiro_rejected_updates", Some(m.rejected_updates as f64)),
                    (
                        "kiro_unattributed_completions",
                        Some(m.unattributed_completions as f64),
                    ),
                ] {
                    if let Some(value) = value {
                        *s.means.entry(name.into()).or_default() += value;
                        *s.observations.entry(name.into()).or_default() += 1;
                    }
                }
            }
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

    fn kiro_start(o: &mut Observer) {
        o.observe(
            "kiro",
            &json!({"type":"runStarted","data":{"engine":"v3","payloadSchema":"acp"}}),
        );
    }
    fn kiro_update(o: &mut Observer, update: Value) {
        o.observe(
            "kiro",
            &json!({"type":"sessionUpdate","data":{"sessionId":"sess_synthetic","update":update}}),
        );
    }
    #[test]
    fn kiro_counts_acp_ids_without_inventing_turns_or_tool_identity() {
        let mut o = Observer::default();
        kiro_start(&mut o);
        kiro_update(
            &mut o,
            json!({"sessionUpdate":"tool_call","toolCallId":"private-id","title":"Reading workflow documentation","status":"pending","rawInput":"private-payload"}),
        );
        kiro_update(
            &mut o,
            json!({"sessionUpdate":"tool_call_update","toolCallId":"private-id","status":"failed"}),
        );
        kiro_update(
            &mut o,
            json!({"sessionUpdate":"tool_call_update","toolCallId":"private-id","status":"failed"}),
        );
        kiro_update(
            &mut o,
            json!({"sessionUpdate":"tool_call","toolCallId":"second","title":"Reading workflow documentation","status":"completed"}),
        );
        o.observe("kiro", &json!({"type":"runFinished","data":{"sessionId":"sess_synthetic","status":"success","stopReason":"end_turn","finalText":"private-answer","finalTextTruncated":false}}));
        let result = &o.observation;
        assert_eq!(result.source, Source::KiroSessionUpdates);
        assert_eq!(result.coverage, Coverage::Partial);
        assert_eq!(result.tool_calls, Some(2));
        assert_eq!(result.completed_tool_calls, Some(2));
        assert_eq!(result.tool_errors, Some(1));
        assert_eq!(result.tool_error_rate(), Some(0.5));
        assert_eq!(result.steps, None);
        assert_eq!(result.repeated_tool_calls, None);
        assert_eq!(result.repeated_tool_errors, None);
        assert_eq!(result.tool_recoveries, None);
        let stored = serde_json::to_string(&o).unwrap();
        for secret in [
            "private-id",
            "private-payload",
            "private-answer",
            "sess_synthetic",
            "Reading workflow",
        ] {
            assert!(!stored.contains(secret));
        }
    }
    #[test]
    fn kiro_missing_ids_invalid_status_and_conflicting_results_are_unknown() {
        let mut o = Observer::default();
        kiro_start(&mut o);
        kiro_update(
            &mut o,
            json!({"sessionUpdate":"tool_call","callId":"wrong-field","status":"completed"}),
        );
        kiro_update(
            &mut o,
            json!({"sessionUpdate":"tool_call_update","toolCallId":"a","status":42}),
        );
        kiro_update(
            &mut o,
            json!({"sessionUpdate":"tool_call_update","toolCallId":"b","status":"failed"}),
        );
        kiro_update(
            &mut o,
            json!({"sessionUpdate":"tool_call_update","toolCallId":"b","status":"completed"}),
        );
        assert_eq!(o.observation.unclassified_events, 3);
        assert_eq!(o.observation.tool_calls, Some(2));
        assert_eq!(o.observation.completed_tool_calls, Some(1));
        assert_eq!(o.observation.tool_errors, Some(1));
        assert!(!o.observation.complete());
    }
    #[test]
    fn kiro_invalid_protocol_session_changes_and_errors_never_certify_coverage() {
        let mut o = Observer::default();
        o.observe("kiro", &json!({"type":"runStarted","data":{"engine":"v2"}}));
        kiro_update(
            &mut o,
            json!({"sessionUpdate":"tool_call","toolCallId":"a","status":"pending"}),
        );
        o.observe("kiro", &json!({"type":"sessionUpdate","data":{"sessionId":"sess_changed","update":{"sessionUpdate":"tool_call_update","toolCallId":"a","status":"completed"}}}));
        o.observe(
            "kiro",
            &json!({"type":"runError","data":{"sessionId":"sess_synthetic"}}),
        );
        assert!(o.observation.stream_incomplete);
        assert_eq!(o.observation.completed_tool_calls, Some(0));
        assert!(!o.observation.complete());
        o.malformed();
        assert_eq!(o.observation.malformed_events, 1);
    }
    #[test]
    fn kiro_tracking_is_bounded_and_cli_step_guards_remain_unknown() {
        let mut o = Observer::default();
        kiro_start(&mut o);
        for index in 0..=MAX_IDENTITIES {
            kiro_update(
                &mut o,
                json!({"sessionUpdate":"tool_call","toolCallId":format!("call-{index}"),"status":"completed"}),
            );
        }
        assert!(o.observation.tracking_limited);
        assert_eq!(o.observation.tool_calls, Some(MAX_IDENTITIES as u64));
        assert_eq!(o.observation.steps, None);
        assert!(!o.observation.complete());
    }

    fn kiro_info(o: &mut Observer, meta: Value) {
        kiro_update(
            o,
            json!({"sessionUpdate":"session_info_update", "_meta":{"kiro":meta}}),
        );
    }
    fn kiro_prompt(o: &mut Observer) {
        kiro_start(o);
        kiro_info(
            o,
            json!({"kind":"user_message_id_assigned", "userMessageId":"private-prompt"}),
        );
        kiro_turn(o, "private-turn");
    }
    fn kiro_turn(o: &mut Observer, id: &str) {
        kiro_info(o, json!({"kind":"turn_start", "messageId":id}));
    }
    fn kiro_completion(credits: f64) -> Value {
        json!({"kind":"turn_completion", "status":"success", "elapsedTime":20,
            "requestIds":["private-request-a","private-request-b"],
            "promptTurnSummaries":[{"unit":"credit","unitPlural":"credits","usage":credits,"usedTools":["private-tool"]}]})
    }
    #[test]
    fn kiro_metering_keeps_units_and_deduplicates_without_certifying_steps() {
        let mut o = Observer::default();
        kiro_prompt(&mut o);
        kiro_info(
            &mut o,
            json!({"kind":"context_usage", "usagePercentage":12.5,
            "breakdown":{"contextFiles":{"tokens":999, "items":["private-path"]}}}),
        );
        kiro_info(&mut o, json!({"kind":"context_usage", "usagePercentage":0}));
        kiro_info(&mut o, kiro_completion(0.25));
        // Reordered request IDs and unrelated future metadata do not rebill.
        let mut duplicate = kiro_completion(0.25);
        duplicate["requestIds"] = json!(["private-request-b", "private-request-a"]);
        duplicate["future"] = json!("private-extra");
        kiro_info(&mut o, duplicate);
        kiro_info(
            &mut o,
            json!({"kind":"turn_end","messageId":"private-turn-end","stopReason":"end_turn"}),
        );
        assert!(!o.ended);
        o.observe("kiro", &json!({"type":"runFinished","data":{"sessionId":"sess_synthetic","status":"success","stopReason":"end_turn","finalText":"private-text","finalTextTruncated":false}}));
        let m = o.observation.kiro_metering.as_ref().unwrap();
        assert_eq!(m.context_usage_percent, Some(0.0));
        assert_eq!(m.peak_context_usage_percent, Some(12.5));
        assert_eq!(m.observed_credits, Some(0.25));
        assert_eq!(m.observed_elapsed_ms, Some(20.0));
        assert_eq!(m.prompt_completions, Some(1));
        assert_eq!(m.credit_observations, 1);
        assert_eq!(m.duplicate_completions, 1);
        assert!(m.valid());
        assert_eq!(o.observation.steps, None);
        assert_eq!(o.observation.coverage, Coverage::Partial);
        assert!(!o.observation.complete());
        assert_eq!(
            Budgets {
                max_steps: Some(4),
                ..Default::default()
            }
            .score(Some(&o.observation)),
            BudgetStatus::Unknown
        );
        let serialized = serde_json::to_string(&o).unwrap();
        assert!(!serialized.contains("private-"));
        assert!(!serialized.contains("tokens"));
        assert!(!serialized.contains("usd"));
        let roundtrip: Observation = serde_json::from_str(&serialized).unwrap();
        assert_eq!(
            roundtrip.kiro_metering.unwrap().observed_credits,
            Some(0.25)
        );
    }
    #[test]
    fn kiro_metering_preserves_missing_and_zero_for_failed_and_aborted_turns() {
        let mut o = Observer::default();
        kiro_prompt(&mut o);
        kiro_info(
            &mut o,
            json!({"kind":"turn_completion", "status":"aborted", "promptTurnSummaries":[]}),
        );
        let m = o.observation.kiro_metering.as_ref().unwrap();
        assert_eq!(m.observed_credits, None);
        assert_eq!(m.observed_elapsed_ms, None);
        assert_eq!(m.prompt_completions, Some(1));
        kiro_turn(&mut o, "second-turn");
        let mut complete = kiro_completion(0.0);
        complete["status"] = json!("failed");
        complete["elapsedTime"] = json!(0);
        kiro_info(&mut o, complete);
        let m = o.observation.kiro_metering.as_ref().unwrap();
        assert_eq!(m.prompt_completions, Some(2));
        assert_eq!(m.observed_credits, Some(0.0));
        assert_eq!(m.observed_elapsed_ms, Some(0.0));
        assert_eq!(m.credit_observations, 1);
        assert_eq!(m.elapsed_observations, 1);
        assert!(m.valid());
    }
    #[test]
    fn kiro_metering_rejects_conflicts_request_reuse_and_helper_evidence() {
        let mut o = Observer::default();
        kiro_prompt(&mut o);
        kiro_info(&mut o, kiro_completion(0.5));
        kiro_info(&mut o, kiro_completion(0.75));
        kiro_turn(&mut o, "second-turn");
        kiro_info(&mut o, kiro_completion(0.5));
        let mut helper = kiro_completion(1.0);
        helper["agentSubtaskId"] = json!("private-helper");
        kiro_info(&mut o, helper);
        let m = o.observation.kiro_metering.as_ref().unwrap();
        assert_eq!(m.observed_credits, Some(0.5));
        assert_eq!(m.prompt_completions, Some(1));
        assert_eq!(m.rejected_updates, 3);
    }
    #[test]
    fn kiro_metering_rejects_invalid_numbers_units_shapes_and_identifiers() {
        for (field, value) in [
            ("status", json!("future-status")),
            ("elapsedTime", json!(-1)),
            ("elapsedTime", json!("20")),
            ("elapsedTime", Value::Null),
            ("requestIds", json!(["duplicate", "duplicate"])),
            ("requestIds", json!([""])),
            ("requestIds", json!(["x".repeat(MAX_IDENTIFIER_BYTES + 1)])),
            ("requestIds", json!("request")),
            ("requestIds", json!(vec!["request"; MAX_IDENTITIES + 1])),
            (
                "promptTurnSummaries",
                json!(vec![
                    json!({"unit":"credit", "usage":0});
                    MAX_KIRO_SUMMARIES + 1
                ]),
            ),
            (
                "promptTurnSummaries",
                json!([{"unit":"credit", "usage":-1}]),
            ),
            (
                "promptTurnSummaries",
                json!([{"unit":"credit", "usage":"1"}]),
            ),
            ("promptTurnSummaries", json!([{"unit":"credit"}])),
            ("promptTurnSummaries", json!([{"unit":"USD", "usage":1}])),
            (
                "promptTurnSummaries",
                json!([{"unit":"credit", "usage":1},{"unit":"credit", "usage":1}]),
            ),
            ("promptTurnSummaries", Value::Null),
        ] {
            let mut o = Observer::default();
            kiro_prompt(&mut o);
            let mut completion = kiro_completion(1.0);
            completion[field] = value;
            kiro_info(&mut o, completion);
            let m = o.observation.kiro_metering.as_ref().unwrap();
            assert_eq!(m.rejected_updates, 1, "{field}");
            assert_eq!(m.observed_credits, None, "{field}");
            assert_eq!(m.prompt_completions, None, "{field}");
        }
        for value in [json!(-1), json!(101), json!("1"), Value::Null] {
            let mut o = Observer::default();
            kiro_prompt(&mut o);
            kiro_info(
                &mut o,
                json!({"kind":"context_usage", "usagePercentage":value}),
            );
            assert_eq!(
                o.observation.kiro_metering.unwrap().context_usage_percent,
                None
            );
        }
    }
    #[test]
    fn kiro_metering_excludes_replay_before_live_prompt_and_resets_between_attempts() {
        let mut first = Observer::default();
        kiro_prompt(&mut first);
        kiro_info(&mut first, kiro_completion(0.5));
        let mut resumed = Observer::default();
        kiro_start(&mut resumed);
        kiro_turn(&mut resumed, "old-turn");
        kiro_info(&mut resumed, kiro_completion(0.5));
        assert_eq!(
            resumed
                .observation
                .kiro_metering
                .as_ref()
                .unwrap()
                .observed_credits,
            None
        );
        kiro_info(
            &mut resumed,
            json!({"kind":"user_message_id_assigned", "userMessageId":"new-prompt"}),
        );
        // A replayed start remains excluded even if repeated after the marker.
        kiro_turn(&mut resumed, "old-turn");
        kiro_info(&mut resumed, kiro_completion(0.5));
        kiro_turn(&mut resumed, "new-turn");
        kiro_info(&mut resumed, kiro_completion(0.25));
        let m = resumed.observation.kiro_metering.as_ref().unwrap();
        assert_eq!(m.observed_credits, Some(0.25));
        assert_eq!(m.unattributed_completions, 2);
        assert_eq!(m.prompt_completions, Some(1));
        assert_eq!(
            first.observation.kiro_metering.unwrap().observed_credits,
            Some(0.5)
        );
    }
    #[test]
    fn kiro_metering_rejects_changed_sessions_prompts_and_post_terminal_events() {
        for failure in ["session", "prompt", "terminal", "start", "malformed"] {
            let mut o = Observer::default();
            kiro_prompt(&mut o);
            match failure {
                "session" => o.observe("kiro", &json!({"type":"sessionUpdate","data":{"sessionId":"sess_other", "update":{"sessionUpdate":"session_info_update", "_meta":{"kiro":kiro_completion(1.0)}}}})),
                "prompt" => kiro_info(&mut o, json!({"kind":"user_message_id_assigned", "userMessageId":"other-prompt"})),
                "terminal" => o.observe("kiro", &json!({"type":"runFinished","data":{"sessionId":"sess_synthetic","status":"success","stopReason":"end_turn","finalText":"ok","finalTextTruncated":false}})),
                "start" => kiro_start(&mut o),
                _ => o.malformed(),
            }
            kiro_info(&mut o, kiro_completion(1.0));
            assert_eq!(
                o.observation
                    .kiro_metering
                    .as_ref()
                    .unwrap()
                    .observed_credits,
                None,
                "{failure}"
            );
            assert!(!o.observation.complete());
        }
    }
    #[test]
    fn kiro_metering_and_tool_counts_exclude_explicit_replay() {
        let mut o = Observer::default();
        kiro_start(&mut o);
        for update in [
            json!({"sessionUpdate":"tool_call","toolCallId":"old-call","status":"failed"}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"old-call","status":"failed"}),
            json!({"sessionUpdate":"session_info_update","_meta":{"kiro":{"kind":"user_message_id_assigned","userMessageId":"old-prompt"}}}),
            json!({"sessionUpdate":"session_info_update","_meta":{"kiro":{"kind":"turn_start","messageId":"old-turn"}}}),
            json!({"sessionUpdate":"session_info_update","_meta":{"kiro":kiro_completion(1.0)}}),
        ] {
            let mut replay = update;
            if replay.get("_meta").is_none() {
                replay["_meta"] = json!({"kiro":{}});
            }
            replay["_meta"]["kiro"]["replay"] = json!(true);
            kiro_update(&mut o, replay);
        }
        assert!(o.kiro_tracking.prompt.is_none());
        assert!(o.kiro_tracking.active_turn.is_none());
        assert_eq!(o.observation.tool_calls, Some(0));
        assert_eq!(o.observation.completed_tool_calls, Some(0));
        assert_eq!(o.observation.tool_errors, Some(0));
        assert!(o.observation.kiro_metering.is_none());
        kiro_info(
            &mut o,
            json!({"kind":"user_message_id_assigned","userMessageId":"new-prompt"}),
        );
        kiro_turn(&mut o, "new-turn");
        kiro_info(&mut o, json!({"kind":"context_usage","usagePercentage":10}));
        kiro_info(
            &mut o,
            json!({"kind":"context_usage","usagePercentage":99,"replay":true}),
        );
        kiro_info(
            &mut o,
            json!({"kind":"user_message_id_assigned","userMessageId":"old-prompt","replay":true}),
        );
        kiro_info(
            &mut o,
            json!({"kind":"turn_start","messageId":"old-turn","replay":true}),
        );
        for kind in ["tool_call", "tool_call_update"] {
            kiro_update(
                &mut o,
                json!({"sessionUpdate":kind,"toolCallId":"old-call","status":"failed","_meta":{"kiro":{"replay":true}}}),
            );
        }
        let mut replay = kiro_completion(2.0);
        replay["replay"] = json!(true);
        kiro_info(&mut o, replay);
        kiro_info(&mut o, kiro_completion(0.25));
        kiro_update(
            &mut o,
            json!({"sessionUpdate":"tool_call","toolCallId":"live-call","status":"completed","_meta":{"kiro":{"replay":false}}}),
        );
        assert_eq!(o.observation.tool_calls, Some(1));
        assert_eq!(o.observation.completed_tool_calls, Some(1));
        assert_eq!(o.observation.tool_errors, Some(0));
        assert!(!o.observation.stream_incomplete);
        let m = o.observation.kiro_metering.unwrap();
        assert_eq!(m.observed_credits, Some(0.25));
        assert_eq!(m.context_usage_percent, Some(10.0));
        assert_eq!(m.peak_context_usage_percent, Some(10.0));
        assert_eq!(m.prompt_completions, Some(1));
        assert_eq!(m.rejected_updates, 0);
    }

    #[test]
    fn kiro_metering_tracking_and_arithmetic_are_bounded() {
        let mut o = Observer::default();
        kiro_prompt(&mut o);
        for i in 0..MAX_IDENTITIES {
            kiro_turn(&mut o, &format!("turn-{i}"));
        }
        assert_eq!(o.kiro_tracking.turns.len(), MAX_IDENTITIES);
        assert!(o.observation.tracking_limited);
        kiro_info(&mut o, kiro_completion(1.0));
        assert_eq!(
            o.observation
                .kiro_metering
                .as_ref()
                .unwrap()
                .observed_credits,
            None
        );

        let mut o = Observer::default();
        kiro_prompt(&mut o);
        let mut completion = kiro_completion(1.0);
        completion["requestIds"] = json!(
            (0..MAX_IDENTITIES)
                .map(|i| format!("r-{i}"))
                .collect::<Vec<_>>()
        );
        kiro_info(&mut o, completion);
        kiro_turn(&mut o, "next-turn");
        kiro_info(&mut o, kiro_completion(1.0));
        assert_eq!(o.kiro_tracking.request_ids.len(), MAX_IDENTITIES);
        assert!(o.observation.tracking_limited);
        assert_eq!(
            o.observation
                .kiro_metering
                .as_ref()
                .unwrap()
                .observed_credits,
            Some(1.0)
        );

        let mut o = Observer::default();
        kiro_prompt(&mut o);
        kiro_info(&mut o, kiro_completion(f64::MAX));
        kiro_turn(&mut o, "overflow-turn");
        let mut completion = kiro_completion(f64::MAX);
        completion["requestIds"] = json!(["new-request"]);
        kiro_info(&mut o, completion);
        let m = o.observation.kiro_metering.unwrap();
        assert_eq!(m.observed_credits, Some(f64::MAX));
        assert_eq!(m.rejected_updates, 1);
        assert!(m.valid());
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

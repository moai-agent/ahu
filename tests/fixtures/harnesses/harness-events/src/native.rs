use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
const BOUNDED_HARNESS: &str = "claude-code";
const TASK_TYPE_AGENT: &str = "local_agent";
/// A background shell task. Not a helper: it is the parent's own Bash tool
/// running detached, and counting it as native delegation turned an ordinary
/// background command into a policy violation.
const TASK_TYPE_SHELL: &str = "local_bash";

/// What a `task_id` seen on this stream refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum TaskKind {
    /// A native helper, so its lifecycle is the owner's to join.
    Helper,
    /// A background shell task, whose lifecycle is not native delegation.
    Shell,
    /// Something this version cannot place. Counted once, never guessed at.
    Unclassified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperStatus {
    Started,
    Progressing,
    Completed,
    Failed,
    Killed,
    /// Reported by the harness under a name this version does not recognise.
    Unknown,
}

impl HelperStatus {
    /// Whether the harness has said anything final about this helper.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            HelperStatus::Completed | HelperStatus::Failed | HelperStatus::Killed
        )
    }
}

/// One native helper, identified by the harness's own task id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Helper {
    pub task_id: String,
    /// The role the harness reported, which may not be the requested one.
    pub role: String,
    pub depth: u32,
    pub backgrounded: bool,
    pub status: HelperStatus,
    pub summary: String,
    /// Where the harness put the helper's full output, when it says.
    pub output_file: Option<String>,
    pub total_tokens: Option<u64>,
}

/// Helper attempts the harness refused, by the limit that refused them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusals {
    pub depth_limit: u64,
    pub concurrency_limit: u64,
    pub budget: u64,
}

impl Refusals {
    pub fn total(&self) -> u64 {
        self.depth_limit + self.concurrency_limit + self.budget
    }
}

/// What the event stream said about native helpers.
///
/// Every field is something the harness stated. Nothing is inferred from the
/// parent's prose, and an event shape this version does not recognise is
/// counted rather than guessed at.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Observations {
    helpers: Vec<Helper>,
    /// What each `task_id` on this stream turned out to be. Only the start event
    /// carries the kind, so later events for the same id are resolved here.
    task_kinds: BTreeMap<String, TaskKind>,
    /// Background shell tasks, kept because the attempt did run them and the
    /// record should say so — but they are not helpers and bound no policy.
    shell_tasks: Vec<String>,
    refusals: Refusals,
    /// Statistics from the harness's terminal event, when it reported them.
    reported_spawned: Option<u64>,
    reported_completed: Option<u64>,
    reported_max_depth: Option<u64>,
    reported_by_type: BTreeMap<String, u64>,
    /// Helper-related events whose shape this version does not recognise.
    pub unknown_events: u64,
    /// Whether the harness emitted a terminal event for the attempt at all.
    terminal_seen: bool,
    stream_incomplete: bool,
}

impl Observations {
    pub(crate) fn incomplete(&mut self) {
        self.stream_incomplete = true;
    }

    #[cfg(test)]
    pub fn helpers(&self) -> &[Helper] {
        &self.helpers
    }

    #[cfg(test)]
    pub fn refusals(&self) -> &Refusals {
        &self.refusals
    }

    #[cfg(test)]
    pub fn observed_roles(&self) -> &BTreeMap<String, u64> {
        &self.reported_by_type
    }

    /// Background shell tasks the attempt ran. Evidence about the attempt, not
    /// about native delegation: no policy is expressed in terms of these.
    #[cfg(test)]
    pub fn shell_tasks(&self) -> &[String] {
        &self.shell_tasks
    }

    /// Feed one decoded stream event.
    ///
    /// Takes an already-parsed value because the caller is draining the same
    /// stream for its own purposes; a second parse of every line would be pure
    /// cost. Events that say nothing about helpers are ignored, not counted as
    /// unknown: only a helper-shaped event with an unreadable shape counts.
    pub fn observe(&mut self, harness: &str, event: &Value) {
        if harness != BOUNDED_HARNESS {
            self.observe_foreign(harness, event);
            return;
        }
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let subtype = event.get("subtype").and_then(Value::as_str).unwrap_or("");
        match (kind, subtype) {
            ("system", "task_started") => self.start(event),
            ("system", "task_progress") => self.progress(event),
            ("system", "task_notification") => self.notify(event),
            // `task_updated` carries a patch this version does not interpret. It
            // is a helper event, but it changes nothing here, so it is neither
            // applied nor counted as unreadable.
            ("system", "task_updated") => (),
            (_, _) if kind == "result" => self.terminal(event),
            _ => (),
        }
    }

    /// Harnesses whose helper events ahu cannot read yet.
    ///
    /// Codex `exec --json` emits `collab_tool_call` items whose
    /// `receiver_thread_ids` and `agents_states` are empty, so a helper cannot
    /// be identified or joined. Counting those events keeps the gap visible
    /// instead of letting an unreadable stream look like an idle one.
    fn observe_foreign(&mut self, harness: &str, event: &Value) {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        // Codex reports completion with a turn event, not Claude's `result`.
        // This does not waive unreadable helper events or prove task acceptance.
        if harness == "codex" && kind == "turn.completed" {
            self.terminal_seen = true;
        }
        let item = event
            .pointer("/item/type")
            .and_then(Value::as_str)
            .unwrap_or("");
        if kind.contains("collab") || item.contains("collab") {
            self.unknown_events += 1;
        }
    }

    fn slot(&mut self, task_id: &str) -> &mut Helper {
        if let Some(index) = self
            .helpers
            .iter()
            .position(|helper| helper.task_id == task_id)
        {
            return &mut self.helpers[index];
        }
        self.helpers.push(Helper {
            task_id: task_id.to_string(),
            role: String::new(),
            depth: 0,
            backgrounded: false,
            status: HelperStatus::Started,
            summary: String::new(),
            output_file: None,
            total_tokens: None,
        });
        self.helpers.last_mut().expect("a helper was just pushed")
    }

    /// Decide what a starting task is, and remember it for its later events.
    ///
    /// `task_type` is the harness's own answer and is taken when it is one this
    /// version knows. When it is not, a `subagent_type` is decisive evidence of
    /// a helper — that is what keeps an agent variant this version has never
    /// seen from being waved through as ordinary background work. A task that
    /// offers neither is counted as unclassified rather than assumed either way.
    fn classify(&mut self, task_id: &str, event: &Value) -> TaskKind {
        let task_type = event.get("task_type").and_then(Value::as_str).unwrap_or("");
        let has_role = event
            .get("subagent_type")
            .and_then(Value::as_str)
            .is_some_and(|role| !role.is_empty());
        let kind = match task_type {
            TASK_TYPE_AGENT => TaskKind::Helper,
            TASK_TYPE_SHELL => TaskKind::Shell,
            _ if has_role => TaskKind::Helper,
            _ => TaskKind::Unclassified,
        };
        if kind == TaskKind::Unclassified {
            // Counted once, here, so the task's own progress and notification
            // events do not each report the same gap again.
            self.unknown_events += 1;
        }
        self.task_kinds.insert(task_id.to_string(), kind);
        kind
    }

    /// The kind of a task seen after its start event.
    ///
    /// Only the start event carries `task_type`, so an id with no recorded kind
    /// means ahu never saw this task begin. That is a gap in its own right: the
    /// task cannot be placed, and inventing a helper for it is exactly the
    /// mistake that made background shell work look like native delegation.
    fn kind_of(&mut self, task_id: &str) -> TaskKind {
        if let Some(kind) = self.task_kinds.get(task_id) {
            return *kind;
        }
        self.unknown_events += 1;
        self.task_kinds
            .insert(task_id.to_string(), TaskKind::Unclassified);
        TaskKind::Unclassified
    }

    fn start(&mut self, event: &Value) {
        let Some(task_id) = task_id(event) else {
            self.unknown_events += 1;
            return;
        };
        match self.classify(&task_id, event) {
            TaskKind::Helper => (),
            TaskKind::Shell => {
                self.shell_tasks.push(task_id);
                return;
            }
            TaskKind::Unclassified => return,
        }
        let role = event
            .get("subagent_type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let depth = event
            .get("spawn_depth")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(u32::MAX as u64) as u32;
        let backgrounded = event
            .get("is_backgrounded")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let helper = self.slot(&task_id);
        helper.role = role;
        helper.depth = depth;
        helper.backgrounded = backgrounded;
    }

    fn progress(&mut self, event: &Value) {
        let Some(task_id) = task_id(event) else {
            self.unknown_events += 1;
            return;
        };
        if self.kind_of(&task_id) != TaskKind::Helper {
            return;
        }
        let tokens = event.pointer("/usage/total_tokens").and_then(Value::as_u64);
        let helper = self.slot(&task_id);
        // Progress never overrides a terminal status: the harness can emit a
        // late progress event for a helper it has already finished.
        if !helper.status.is_terminal() {
            helper.status = HelperStatus::Progressing;
        }
        if tokens.is_some() {
            helper.total_tokens = tokens;
        }
    }

    fn notify(&mut self, event: &Value) {
        let Some(task_id) = task_id(event) else {
            self.unknown_events += 1;
            return;
        };
        if self.kind_of(&task_id) != TaskKind::Helper {
            return;
        }
        let raw = event.get("status").and_then(Value::as_str).unwrap_or("");
        let status = match raw {
            "completed" | "success" => HelperStatus::Completed,
            "failed" | "error" => HelperStatus::Failed,
            "killed" | "cancelled" | "canceled" | "stopped" => HelperStatus::Killed,
            _ => HelperStatus::Unknown,
        };
        if status == HelperStatus::Unknown {
            self.unknown_events += 1;
        }
        let output_file = event
            .get("output_file")
            .and_then(Value::as_str)
            .map(str::to_string);
        let tokens = event.pointer("/usage/total_tokens").and_then(Value::as_u64);
        let helper = self.slot(&task_id);
        helper.status = status;
        if output_file.is_some() {
            helper.output_file = output_file;
        }
        if tokens.is_some() {
            helper.total_tokens = tokens;
        }
    }

    fn terminal(&mut self, event: &Value) {
        self.terminal_seen = true;
        let Some(stats) = event.get("subagent_stats") else {
            return;
        };
        self.reported_spawned = stats.get("spawned").and_then(Value::as_u64);
        self.reported_completed = stats.get("completed").and_then(Value::as_u64);
        self.reported_max_depth = stats.get("max_depth").and_then(Value::as_u64);
        for (limit, slot) in [
            ("depth_limit", &mut self.refusals.depth_limit),
            ("concurrency_limit", &mut self.refusals.concurrency_limit),
            ("budget", &mut self.refusals.budget),
        ] {
            if let Some(count) = stats
                .pointer(&format!("/refused/{limit}"))
                .and_then(Value::as_u64)
            {
                *slot = count;
            }
        }
        if let Some(by_type) = stats.get("by_type").and_then(Value::as_object) {
            for (role, count) in by_type {
                if let Some(count) = count.as_u64() {
                    if self.reported_by_type.len() < 256 || self.reported_by_type.contains_key(role)
                    {
                        self.reported_by_type.insert(role.clone(), count);
                    } else {
                        self.stream_incomplete = true;
                    }
                }
            }
        }
    }

    /// Assess whether the owning task may report this attempt as finished work.
    ///
    /// `complete` is false whenever a helper started without reaching a terminal
    /// event, whenever an observation contradicts the frozen policy, whenever the
    /// native helper stream ended without its terminal event, or whenever the harness
    /// accounted for helper work that ahu never saw. A parent's final message
    /// does not join a helper, so none of those conditions is waived by the
    /// attempt having produced a result.
    ///
    /// A refusal is none of those things. The harness saying it declined to start
    /// a helper is complete information about work that did not happen, so it is
    /// reported without holding the attempt open.
    pub fn completeness(&self) -> Completeness {
        let mut joined = Vec::new();
        let mut unjoined = Vec::new();
        let violations = Vec::new();
        let mut unobserved = Vec::new();
        let mut unknown = Vec::new();

        for helper in &self.helpers {
            if helper.status.is_terminal() {
                joined.push(helper.task_id.clone());
            } else {
                unjoined.push(helper.task_id.clone());
            }
            if helper.status == HelperStatus::Killed {
                unknown.push(format!(
                    "helper {} was stopped; whether its provider-side work also stopped is not observable",
                    helper.task_id
                ));
            }
            if helper.status == HelperStatus::Unknown {
                unknown.push(format!(
                    "helper {} reported a status this version does not recognise",
                    helper.task_id
                ));
            }
        }

        // The harness counts helpers it started. A helper ahu never saw an event
        // for is work that demonstrably happened out of sight, which is a
        // different thing from a question ahu merely cannot answer: it is
        // recorded as unobserved so it cannot be reported as complete.
        if let Some(spawned) = self.reported_spawned
            && spawned > self.helpers.len() as u64
        {
            unobserved.push(format!(
                "the harness reported {spawned} helper(s) but emitted events for {}; the remainder cannot be joined",
                self.helpers.len()
            ));
        }
        if self.refusals.total() > 0 {
            unknown.push(format!(
                "the harness refused {} helper request(s) at its limits (depth {}, concurrency {}, budget {}); work those helpers would have done was not performed",
                self.refusals.total(),
                self.refusals.depth_limit,
                self.refusals.concurrency_limit,
                self.refusals.budget
            ));
        }
        if self.stream_incomplete {
            unknown.push("native helper stream evidence is incomplete".into());
        }
        if !self.terminal_seen {
            unknown.push(
                "native helper lifecycle coverage has no terminal event; helper completion is unknown"
                    .to_string(),
            );
        }

        // Two separate questions, kept apart because conflating them is what let
        // an attempt report itself complete while naming helpers it never saw.
        // The attempt can end cleanly and still rest on evidence ahu does not
        // have; only when both hold is there nothing outstanding.
        let terminated_cleanly = unjoined.is_empty() && violations.is_empty() && self.terminal_seen;
        let evidence_complete =
            unobserved.is_empty() && self.unknown_events == 0 && !self.stream_incomplete;
        Completeness {
            complete: terminated_cleanly && evidence_complete,
            terminated_cleanly,
            evidence_complete,
            joined,
            unjoined,
            violations,
            unobserved,
            unknown,
            unknown_events: self.unknown_events,
        }
    }
}

/// The terminal assessment of an attempt's native helpers.
///
/// The two halves are named separately because they fail for different reasons
/// and a caller may care about only one. An attempt can end tidily while resting
/// on helper work ahu never saw, and reporting that as complete is what this
/// split exists to prevent: [`Completeness::complete`] requires both.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Completeness {
    /// Both halves hold: nothing is outstanding and nothing is missing.
    pub complete: bool,
    /// Every helper ahu saw joined, nothing contradicted the policy, and the
    /// native helper stream contained no unresolved helper or policy violation,
    /// and its terminal event was observed. Root process completion is recorded
    /// separately in the headless result.
    pub terminated_cleanly: bool,
    /// Everything that happened was visible: no helper work reached ahu only as
    /// a counter, and no helper event was unreadable.
    pub evidence_complete: bool,
    pub joined: Vec<String>,
    /// Helpers that started and never reached a terminal event.
    pub unjoined: Vec<String>,
    /// Observations that contradict the frozen policy.
    pub violations: Vec<String>,
    /// Helper work the harness accounted for but ahu never saw happen. Distinct
    /// from a refusal, which is work the harness accounted for and declined to
    /// start, and from an unjoined helper, whose start ahu did see.
    pub unobserved: Vec<String>,
    /// Things that are genuinely not knowable from this stream.
    pub unknown: Vec<String>,
    pub unknown_events: u64,
}

impl Completeness {
    /// Blockers for the attempt's result, in the order a reader needs them.
    ///
    /// Unknowns are not blockers: they are reported separately so that "ahu
    /// cannot tell" never reads as "ahu found a failure".
    pub fn blockers(&self) -> Vec<String> {
        let mut blockers = self.violations.clone();
        for task_id in &self.unjoined {
            blockers.push(format!(
                "native helper {task_id} started and never reported an outcome; the attempt's work is partial"
            ));
        }
        blockers.extend(self.unobserved.iter().cloned());
        if self.unknown_events > 0 {
            blockers.push(format!(
                "{} native helper event(s) could not be read; helper coverage for this attempt is incomplete",
                self.unknown_events
            ));
        }
        blockers
    }
}

fn task_id(event: &Value) -> Option<String> {
    event
        .get("task_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;

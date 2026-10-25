//! Normalized native helper observations and host-owned policy assessment.
//! Provider flags, version rules and native event parsing live in runtime plugins.
use crate::util::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub const DISABLED: &str = "disabled";
pub const BOUNDED: &str = "bounded";
#[derive(Serialize)]
pub struct Request<'a> {
    pub harness: &'a str,
    pub harness_version: &'a str,
    pub policy: &'a str,
    /// The owning ahu agent's frozen model. Helpers fall back to it.
    pub session_model: &'a str,
    /// Exact helper model id. `None` pins helpers to `session_model`.
    pub helper_model: Option<&'a str>,
    /// Declared helper role. Requested only: see the role gap in [`profile_at`].
    pub helper_role: &'a str,
    pub max_concurrent: u32,
    /// Helper spawn depth. Only `1` is enforceable on any validated version.
    pub max_depth: u32,
    pub budget_usd: Option<f64>,
    /// Whether the owning assignment needs to change files or run commands.
    ///
    /// The bounded tool ceiling binds the parent as well as its helpers, so a
    /// bounded attempt is read-only for everyone in it. A caller that declares
    /// a writing assignment is refused rather than quietly given a session that
    /// cannot carry it out.
    pub assignment_writes: bool,
}

/// How a control is imposed.
///
/// There is deliberately no prompt variant. A restriction that exists only as
/// instruction text is not a control and does not belong in [`Profile::enforced`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mechanism {
    /// A command-line flag the harness parses.
    Flag,
    /// An environment variable the harness reads at startup.
    Env,
    /// A capability the helper does not have, because the tool is not in the
    /// session at all. Nothing has to refuse it; there is nothing to call.
    Structural,
}

impl Mechanism {
    pub fn as_str(self) -> &'static str {
        match self {
            Mechanism::Flag => "flag",
            Mechanism::Env => "env",
            Mechanism::Structural => "structural",
        }
    }
}

/// One boundary the profile actually imposes, with how it is imposed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Control {
    pub id: String,
    pub mechanism: Mechanism,
    pub detail: String,
}

/// The native argument, environment, and disclosure surface for one attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub policy: String,
    pub harness: String,
    /// Arguments to splice into the harness argv ahead of the prompt separator.
    pub args: Vec<String>,
    /// Variables to set on the child.
    pub env: Vec<(String, String)>,
    /// Variables to remove from the inherited environment before the child runs.
    pub env_remove: Vec<String>,
    /// The enforced tool ceiling, recorded so the frozen policy is reconstructible.
    pub helper_tools: Vec<String>,
    pub helper_model: String,
    pub max_concurrent: u32,
    pub max_depth: u32,
    pub enforced: Vec<Control>,
    pub gaps: Vec<String>,
}

impl Profile {
    /// Stable `id=mechanism` strings for the attempt's integrity record.
    pub fn control_ids(&self) -> Vec<String> {
        self.enforced
            .iter()
            .map(|control| format!("{}={}", control.id, control.mechanism.as_str()))
            .collect()
    }

    /// Whether this profile permits any native delegation at all.
    pub fn helpers_permitted(&self) -> bool {
        self.policy == BOUNDED
    }
}

/// Invocation features admitted by the selected runtime for this exact request.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExecutionCapabilities {
    pub structured_output: bool,
    pub broker_transport: bool,
}

pub(crate) fn execution_capabilities(
    repo: &crate::git::Repo,
    harness: &str,
    version: &str,
    permissions: crate::agent::Permissions,
    work_mode: bool,
) -> Result<ExecutionCapabilities> {
    let runtime = crate::harness_runtime::load(repo, harness)?;
    let value = runtime.call(
        "admission",
        serde_json::json!({
            "action":"execution_capabilities", "harness":harness,
            "native_version":version, "permissions":permissions, "work_mode":work_mode,
        }),
    )?;
    serde_json::from_value(value).map_err(|_| Error::new("invalid runtime execution capabilities"))
}

/// Repository-scoped native helper profile supplied by the installed runtime.
pub fn profile_at(repo: &crate::git::Repo, request: &Request<'_>) -> Result<Profile> {
    let runtime = crate::harness_runtime::load(repo, request.harness)?;
    let result = runtime.call(
        "admission",
        serde_json::json!({
            "action":"native_profile", "project":repo.root, "request":request,
        }),
    )?;
    validate_profile(result, request)
}

fn validate_profile(result: serde_json::Value, request: &Request<'_>) -> Result<Profile> {
    let profile: Profile =
        serde_json::from_value(result).map_err(|_| Error::new("invalid runtime native profile"))?;
    if !matches!(request.policy, DISABLED | BOUNDED)
        || profile.harness != request.harness
        || profile.policy != request.policy
        || profile.max_depth > request.max_depth
        || profile.max_concurrent > request.max_concurrent
        || if request.policy == DISABLED {
            !profile.helper_model.is_empty()
                || profile.max_depth != 0
                || profile.max_concurrent != 0
        } else {
            profile.helper_model != request.helper_model.unwrap_or(request.session_model)
        }
        || profile
            .env
            .iter()
            .any(|(key, value)| !profile_env_key(key) || value.len() > 4096 || value.contains('\0'))
        || profile.env_remove.iter().any(|key| !profile_env_key(key))
        || profile
            .args
            .iter()
            .any(|arg| arg.contains('\0') || arg.len() > 65536)
        || profile.args.len() > 256
        || profile.env.len() > 64
        || profile.env_remove.len() > 64
        || serde_json::to_vec(&profile)?.len() > 64 * 1024
    {
        return Err(Error::new("runtime native profile exceeds frozen request"));
    }
    Ok(profile)
}

fn profile_env_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && !key.starts_with("AHU_")
        && !key.starts_with("OTEL_")
        && !matches!(key, "HOME" | "PATH" | "PWD")
}

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
    #[serde(default)]
    pub stream_incomplete: bool,
}

impl Observations {
    pub(crate) fn incomplete(&mut self) {
        self.stream_incomplete = true;
    }
    pub(crate) fn validate(&self) -> bool {
        self.helpers.len() <= 256
            && self.task_kinds.len() <= 256
            && self.shell_tasks.len() <= 256
            && self.reported_by_type.len() <= 256
            && self.helpers.iter().all(|h| h.summary.is_empty())
            && [
                self.refusals.depth_limit,
                self.refusals.concurrency_limit,
                self.refusals.budget,
            ]
            .into_iter()
            .all(|n| n <= u64::MAX / 3)
    }

    pub fn helpers(&self) -> &[Helper] {
        &self.helpers
    }

    pub fn refusals(&self) -> &Refusals {
        &self.refusals
    }

    pub fn observed_roles(&self) -> &BTreeMap<String, u64> {
        &self.reported_by_type
    }

    /// Background shell tasks the attempt ran. Evidence about the attempt, not
    /// about native delegation: no policy is expressed in terms of these.
    pub fn shell_tasks(&self) -> &[String] {
        &self.shell_tasks
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
    pub fn completeness(&self, profile: &Profile) -> Completeness {
        let mut joined = Vec::new();
        let mut unjoined = Vec::new();
        let mut violations = Vec::new();
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
            if helper.depth > profile.max_depth {
                violations.push(format!(
                    "helper {} ran at depth {}, beyond the frozen depth of {}",
                    helper.task_id, helper.depth, profile.max_depth
                ));
            }
        }

        // A policy that permits no helpers is contradicted by any evidence that
        // one ran, and the harness's own spawn counter is such evidence even
        // when not one helper event reached ahu. Taking the larger of the two
        // counts means a helper that was visible only in the terminal totals
        // still lands as a violation rather than disappearing.
        if !profile.helpers_permitted() {
            let observed = self.helpers.len() as u64;
            let reported = self.reported_spawned.unwrap_or(0);
            let ran = observed.max(reported);
            if ran > 0 {
                let visibility = if observed == 0 {
                    ", visible only in the harness's terminal statistics"
                } else {
                    ""
                };
                violations.push(format!(
                    "{ran} native helper(s) ran under the {} policy, which permits none{visibility}",
                    profile.policy
                ));
            }
        }
        if let Some(depth) = self.reported_max_depth
            && depth > profile.max_depth as u64
        {
            violations.push(format!(
                "the harness reported a helper depth of {depth}, beyond the frozen depth of {}",
                profile.max_depth
            ));
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
            unknown.push("native helper evidence is incomplete".into());
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

#[cfg(test)]
mod profile_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn capability_evidence_requires_both_explicit_booleans_and_no_extra_fields() {
        for value in [
            json!({}),
            json!({"structured_output":true}),
            json!({"structured_output":true,"broker_transport":1}),
            json!({"structured_output":true,"broker_transport":true,"raw":"PRIVATE"}),
        ] {
            assert!(serde_json::from_value::<ExecutionCapabilities>(value).is_err());
        }
        let denied: ExecutionCapabilities =
            serde_json::from_value(json!({"structured_output":false,"broker_transport":false}))
                .unwrap();
        assert!(!denied.structured_output && !denied.broker_transport);
    }

    #[test]
    fn normalized_profile_cannot_expand_policy_or_change_host_environment() {
        let request = Request {
            harness: "invented",
            harness_version: "1.0.0",
            policy: BOUNDED,
            session_model: "exact",
            helper_model: None,
            helper_role: "reader",
            max_concurrent: 1,
            max_depth: 1,
            budget_usd: Some(5.0),
            assignment_writes: false,
        };
        let good = json!({"harness":"invented","policy":"bounded","args":[],"env":[],
            "env_remove":[],"helper_tools":["Read"],"helper_model":"exact","max_concurrent":1,
            "max_depth":1,"enforced":[],"gaps":[]});
        assert!(validate_profile(good.clone(), &request).is_ok());
        for (key, value) in [
            ("harness", json!("other")),
            ("policy", json!("disabled")),
            ("helper_model", json!("other")),
            ("max_depth", json!(2)),
            ("max_concurrent", json!(2)),
            ("unknown", json!(true)),
        ] {
            let mut invalid = good.clone();
            invalid[key] = value;
            assert!(validate_profile(invalid, &request).is_err(), "{key}");
        }
        for key in [
            "AHU_TASK_ID",
            "AHU_MCP_OTEL_ENDPOINT",
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "HOME",
            "PATH",
            "PWD",
        ] {
            for remove in [false, true] {
                let mut invalid = good.clone();
                if remove {
                    invalid["env_remove"] = json!([key]);
                } else {
                    invalid["env"] = json!([[key, "injected"]]);
                }
                assert!(validate_profile(invalid, &request).is_err(), "{key}");
            }
        }
        let mut disabled = good.clone();
        disabled["policy"] = json!("disabled");
        disabled["helper_model"] = json!("");
        disabled["max_depth"] = json!(0);
        disabled["max_concurrent"] = json!(0);
        let disabled_request = Request {
            policy: DISABLED,
            ..request
        };
        assert!(validate_profile(disabled.clone(), &disabled_request).is_ok());
        disabled["max_concurrent"] = json!(1);
        assert!(validate_profile(disabled, &disabled_request).is_err());
        let mut allowed = good;
        allowed["env"] = json!([["INVENTED_HELPER_MODEL", "exact"]]);
        assert!(validate_profile(allowed, &request).is_ok());
    }
}

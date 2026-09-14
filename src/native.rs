//! Bounded native helpers: what a harness can actually be made to enforce.
//!
//! A native helper is a harness-managed worker inside one ahu task attempt. It
//! is not an ahu child: it has no manifest, no worktree, no task record, and it
//! can never satisfy a request for a registered agent. This module decides
//! whether a harness version can be held to the bounded profile at all, and
//! produces the arguments and environment that hold it there.
//!
//! Two rules shape everything here:
//!
//!   - a control is only reported when a flag, an environment variable, or the
//!     structural absence of a tool imposes it. Prompt text is not enforcement,
//!     so [`Mechanism`] has no variant for it and no wording in this module
//!     claims that instructions restrain a helper;
//!   - a boundary that cannot be imposed is named in [`Profile::gaps`] rather
//!     than quietly dropped. If a boundary cannot be imposed *and* its absence
//!     would widen what a helper can do, [`profile`] refuses instead.
//!
//! The bounded profile is read-only for everyone inside it. The tool ceiling
//! that bounds the helpers is a session ceiling, so it bounds the owning agent
//! too: a bounded attempt has no shell and no file-writing tool. That makes
//! `bounded` a policy for read-only assignments — a review or an investigation
//! whose owner wants internal helpers — and not one an implementing agent can
//! run under.
//!
//! Not everything the harness reports as a task is a helper. Background shell
//! tasks travel on the same events and are told apart by what the harness calls
//! each one, because treating the parent's own detached Bash command as native
//! delegation turns ordinary background work into a policy violation.
//!
//! The observation half is the other side of the same contract. The owning ahu
//! task must join its helpers before it reports, so [`Observations`] tracks each
//! helper by its native task id and [`Observations::completeness`] reports a
//! helper that started and never reached a terminal event as unjoined work, not
//! as success.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bail;
use crate::util::Result;

/// No native delegation at all: the delegation tool is removed from the session.
pub const DISABLED: &str = "disabled";
/// Read-only helpers under the enforced ceiling described in [`profile`].
pub const BOUNDED: &str = "bounded";

/// The only harness/version pair whose bounded controls have been validated live.
const BOUNDED_HARNESS: &str = "claude-code";
const BOUNDED_VERSIONS: &[&str] = &["2.1.270"];

/// The tool ceiling a bounded helper runs under.
///
/// `Task` is the name the CLI allowlist uses for the delegation tool; `Agent` is
/// the name the same tool carries in `tool_use` events. Both spellings appear in
/// this module for that reason, and neither is a typo for the other.
const HELPER_TOOLS: &[&str] = &["Read", "Grep", "Glob"];
const SESSION_TOOLS: &[&str] = &["Read", "Grep", "Glob", "Task"];

/// The `task_type` the harness puts on a delegation task's start event.
///
/// The same `task_*` events carry both native helpers and background shell
/// tasks, and only the start event says which. Everything after it — progress,
/// notification — is identified by `task_id` alone, so the kind established here
/// has to be remembered for the rest of the stream.
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

/// Native settings that would otherwise reach the child from the ambient
/// environment and move a boundary the profile just fixed.
const SCRUBBED_ENV: &[&str] = &[
    "CLAUDE_CODE_ENABLE_TASKS",
    "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS",
    "CLAUDE_CODE_FORK_SUBAGENT",
    "CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS",
    "CLAUDE_CODE_MAX_SUBAGENTS_PER_SESSION",
    "CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH",
    "CLAUDE_CODE_SUBAGENT_MODEL",
    "CLAUDE_CODE_SUBAGENT_MODEL_FORCE",
];

/// What the caller wants from the native layer.
///
/// Every field is plain data supplied by the caller. Nothing here is read from
/// the process environment: the effective policy is frozen into the task's
/// integrity record, and a profile that consulted ambient state could not be
/// reconstructed from that record later.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    pub harness: &'a str,
    pub harness_version: &'a str,
    pub policy: &'a str,
    /// The owning ahu agent's frozen model. Helpers fall back to it.
    pub session_model: &'a str,
    /// Exact helper model id. `None` pins helpers to `session_model`.
    pub helper_model: Option<&'a str>,
    /// Declared helper role. Requested only: see the role gap in [`profile`].
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

impl Request<'_> {
    /// The model a helper actually runs on once the profile is applied.
    fn effective_helper_model(&self) -> &str {
        self.helper_model.unwrap_or(self.session_model)
    }
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

impl Control {
    fn new(id: &str, mechanism: Mechanism, detail: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            mechanism,
            detail: detail.into(),
        }
    }
}

/// The native argument, environment, and disclosure surface for one attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Build the native profile for one attempt, or refuse with the unmet boundary.
///
/// The bounded profile is only offered for harness versions whose controls were
/// checked against the installed CLI. A refusal names the specific boundary so a
/// caller can report it; callers must not answer a refusal by weakening the
/// policy, which is why this returns an error rather than a downgraded profile.
///
/// What the bounded profile enforces on `claude-code` 2.1.270, and how:
///
///   - the session tool allowlist binds the parent *and* every helper at every
///     depth, overriding a built-in role's nominal unrestricted tool set. That
///     is what removes shell execution, writes, native worktrees, agent teams,
///     and with them any route to cmux;
///   - a spawn-depth cap of one, imposed by removing the delegation tool from
///     helpers rather than by refusing their calls;
///   - a helper concurrency cap, whose excess attempts are counted in the
///     harness's own terminal statistics;
///   - a helper model pin that applies even when a built-in role is chosen;
///   - MCP servers excluded, so helpers cannot reach tools the ceiling never
///     listed, while the repository's own settings — and the hooks and deny
///     rules in them — stay in force, because settings cannot widen the ceiling;
///   - an optional spend ceiling.
///
/// Because the ceiling is a session ceiling, it applies to the owning agent as
/// well. A bounded attempt cannot write files or run commands, so [`profile`]
/// refuses `bounded` for an assignment the caller declares as writing.
///
/// Two boundaries are not available and are reported as gaps. The built-in
/// helper roles cannot be removed from the roster, so `helper_role` is a
/// requested role and the observed role is recorded separately; and no total
/// helper count per session can be imposed, so the spend ceiling is the only
/// real cap on how many helpers run. Neither gap widens a helper's tools or
/// model, because the ceiling and the model pin bind every role.
pub fn profile(request: &Request<'_>) -> Result<Profile> {
    match request.policy {
        DISABLED => disabled(request),
        BOUNDED => bounded(request),
        other => bail!(
            "unknown native helper policy {other:?}; expected {DISABLED:?} or {BOUNDED:?}. No policy was selected."
        ),
    }
}

/// Turning delegation off is itself a control, so it is reported as one.
fn disabled(request: &Request<'_>) -> Result<Profile> {
    let (args, enforced, gaps) = match request.harness {
        "claude-code" => (
            vec!["--disallowedTools".to_string(), "Task".to_string()],
            vec![Control::new(
                "native.delegation.off",
                Mechanism::Structural,
                "Task is withheld from the session tool set, so no delegation tool exists to call",
            )],
            Vec::new(),
        ),
        "codex" => (
            vec![],
            vec![Control::new(
                "native.delegation.off",
                Mechanism::Flag,
                "the adapter's own `-c agents.enabled=false` withholds native agents",
            )],
            Vec::new(),
        ),
        // Other harnesses get no native arguments and no claim that delegation
        // is off: the default may well permit helpers, and saying otherwise
        // would be the kind of unchecked assurance this module exists to avoid.
        other => (
            vec![],
            vec![],
            vec![format!(
                "native delegation for {other} is not disabled by any control ahu passes; its default behaviour has not been checked"
            )],
        ),
    };
    Ok(Profile {
        policy: DISABLED.into(),
        harness: request.harness.into(),
        args,
        env: Vec::new(),
        env_remove: SCRUBBED_ENV
            .iter()
            .map(|name| (*name).to_string())
            .collect(),
        helper_tools: Vec::new(),
        helper_model: String::new(),
        max_concurrent: 0,
        max_depth: 0,
        enforced,
        gaps,
    })
}

fn bounded(request: &Request<'_>) -> Result<Profile> {
    if request.harness != BOUNDED_HARNESS {
        bail!(
            "bounded native helpers are not available for {}: only {BOUNDED_HARNESS} has a validated helper tool ceiling, depth cap, and joinable helper outcomes. Use {DISABLED:?}; ahu will not substitute a different helper backend.",
            request.harness
        );
    }
    if !BOUNDED_VERSIONS.contains(&request.harness_version) {
        bail!(
            "bounded native helpers are not validated for {BOUNDED_HARNESS} {:?}; validated versions: {BOUNDED_VERSIONS:?}. Re-run the control validation for this version before enabling bounded helpers.",
            request.harness_version
        );
    }
    if request.assignment_writes {
        bail!(
            "bounded native helpers cannot be used for an assignment that changes files or runs commands: the tool ceiling that bounds the helpers bounds the owning agent too, leaving it {HELPER_TOOLS:?} and no shell. Use {DISABLED:?} for implementation work, or declare the assignment read-only."
        );
    }
    if request.max_depth != 1 {
        bail!(
            "bounded native helpers allow a helper spawn depth of exactly 1, not {}: depth 1 is enforced by withholding the delegation tool from helpers, and no deeper limit has been shown to hold.",
            request.max_depth
        );
    }
    if request.max_concurrent == 0 {
        bail!("bounded native helpers need a concurrency cap of at least 1");
    }
    let helper_model = request.effective_helper_model();
    if helper_model.is_empty() || helper_model.starts_with('-') {
        bail!("an exact helper model is required for bounded native helpers");
    }
    if request.helper_role.is_empty() || !is_role_name(request.helper_role) {
        bail!(
            "a bounded helper role must be a non-empty name of letters, digits, '-' or '_'; got {:?}",
            request.helper_role
        );
    }
    if let Some(budget) = request.budget_usd
        && !(budget.is_finite() && budget > 0.0)
    {
        bail!("a native helper budget must be a positive finite dollar amount");
    }

    let mut args = vec![
        "--tools".to_string(),
        SESSION_TOOLS.join(","),
        "--agents".to_string(),
        helper_definition(request.helper_role, helper_model)?,
        "--strict-mcp-config".to_string(),
        "--disable-slash-commands".to_string(),
    ];
    if let Some(budget) = request.budget_usd {
        args.push("--max-budget-usd".to_string());
        args.push(format!("{budget}"));
    }

    let env = vec![
        (
            "CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH".to_string(),
            "1".to_string(),
        ),
        (
            "CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS".to_string(),
            request.max_concurrent.to_string(),
        ),
        (
            "CLAUDE_CODE_SUBAGENT_MODEL".to_string(),
            helper_model.to_string(),
        ),
        (
            "CLAUDE_CODE_SUBAGENT_MODEL_FORCE".to_string(),
            "1".to_string(),
        ),
    ];

    let enforced = vec![
        Control::new(
            "native.assignment.read_only",
            Mechanism::Structural,
            format!(
                "the ceiling binds the owning agent too, not only its helpers: a bounded attempt has {SESSION_TOOLS:?} and nothing else, so the owner cannot change files, run commands, or launch ahu children during it"
            ),
        ),
        Control::new(
            "native.helper.tools",
            Mechanism::Flag,
            format!(
                "--tools {} binds the parent and every helper at every depth, including built-in roles whose nominal tool set is unrestricted",
                SESSION_TOOLS.join(",")
            ),
        ),
        Control::new(
            "native.helper.no_write",
            Mechanism::Structural,
            "Write, Edit, NotebookEdit and Bash are outside the ceiling, so a helper has no tool that changes the worktree",
        ),
        Control::new(
            "native.helper.no_worktree_or_team",
            Mechanism::Structural,
            "EnterWorktree, ExitWorktree, Workflow, SendMessage and ListAgents are outside the ceiling, so helpers create no worktrees and no agent teams",
        ),
        Control::new(
            "native.helper.no_cmux",
            Mechanism::Structural,
            "no shell tool is in the ceiling, so a helper has no route to invoke cmux",
        ),
        Control::new(
            "native.helper.depth",
            Mechanism::Env,
            "CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH=1 withholds the delegation tool from helpers, so depth 1 holds without relying on a refusal",
        ),
        Control::new(
            "native.helper.concurrency",
            Mechanism::Env,
            format!(
                "CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS={}; attempts beyond the cap are refused and counted in the harness's terminal statistics",
                request.max_concurrent
            ),
        ),
        Control::new(
            "native.helper.model",
            Mechanism::Env,
            format!(
                "CLAUDE_CODE_SUBAGENT_MODEL={helper_model} with CLAUDE_CODE_SUBAGENT_MODEL_FORCE=1 pins the helper model even when a built-in role is chosen"
            ),
        ),
        Control::new(
            "native.helper.no_mcp",
            Mechanism::Flag,
            "--strict-mcp-config with no --mcp-config keeps MCP servers out of the session, and holds even against a settings file that enables project servers",
        ),
        Control::new(
            "native.settings.retained",
            Mechanism::Flag,
            "user, project and local settings are left in place, so their permission deny rules still apply and still produce denial events; they cannot widen the ceiling, because a settings file allowing shell and writes still produced a session with only the allowlisted tools",
        ),
        Control::new(
            "native.helper.no_skills",
            Mechanism::Flag,
            "--disable-slash-commands keeps skills, which can define their own delegation, out of the session",
        ),
        Control::new(
            "native.env.frozen",
            Mechanism::Env,
            "ambient native helper settings are removed from the child environment so the frozen policy is what runs",
        ),
    ];

    let mut enforced = enforced;
    if request.budget_usd.is_some() {
        enforced.push(Control::new(
            "native.helper.budget",
            Mechanism::Flag,
            "--max-budget-usd ends the attempt when the ceiling is reached, reported as a budget-exhausted terminal outcome",
        ));
    }

    let gaps = vec![
        format!(
            "a bounded attempt is read-only for the owning agent as well as its helpers: it has {SESSION_TOOLS:?} and no shell, so it cannot implement changes, run builds or tests, or shell-launch ahu children. Bounded suits a read-only assignment such as a review or an investigation that uses internal helpers; implementation work needs the {DISABLED:?} policy."
        ),
        "whether settings-defined hooks run in this print-mode configuration was not demonstrated: deny rules were observed to take effect, but no hook execution was observed, so ahu does not claim hooks constrain a bounded attempt."
            .to_string(),
        "settings-defined agent files add roles to the session roster, so the roster is wider than the requested role in a repository that ships agent definitions. Those roles run under the same ceiling and the same pinned model."
            .to_string(),
        format!(
            "the helper role {:?} is requested and the role a helper actually ran under is observed; neither is guaranteed. The built-in roles cannot be removed from the session roster, so a helper may run under one of those instead. The tool ceiling and the model pin apply to every role, so this cannot widen a helper's tools or model; only the role's own system prompt differs.",
            request.helper_role
        ),
        "the number of helpers per attempt is observed, not capped: the harness's per-session limit did not refuse an additional helper when tested, so ahu does not set it and claims no count cap. Only the concurrency cap and the spend ceiling bound helper use."
            .to_string(),
        "whether a helper's provider-side work stops when the attempt is cancelled cannot be observed: no local process survives the attempt's process group, but the harness exposes no disposition for provider-managed helper threads. Cancelled helpers are reported as unknown, never as stopped."
            .to_string(),
        "helpers may be started in the background without being asked for, so a helper's outcome must be joined by its native task id rather than inferred from the parent's final message."
            .to_string(),
    ];

    Ok(Profile {
        policy: BOUNDED.into(),
        harness: request.harness.into(),
        args,
        env,
        env_remove: SCRUBBED_ENV
            .iter()
            .map(|name| (*name).to_string())
            .collect(),
        helper_tools: HELPER_TOOLS
            .iter()
            .map(|tool| (*tool).to_string())
            .collect(),
        helper_model: helper_model.to_string(),
        max_concurrent: request.max_concurrent,
        max_depth: 1,
        enforced,
        gaps,
    })
}

/// Role names go into a JSON object key and into event comparisons; keeping them
/// to this alphabet means neither has to worry about quoting or normalisation.
fn is_role_name(role: &str) -> bool {
    role.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// The `--agents` definition for the declared helper role.
///
/// The per-role `tools` and `model` here agree with the session ceiling and the
/// model pin rather than replacing them. They matter when the declared role is
/// the one actually chosen; the enforced boundary does not depend on that.
fn helper_definition(role: &str, model: &str) -> Result<String> {
    let definition = serde_json::json!({
        role: {
            "description": "Bounded read-only helper for locating and quoting code within this assignment",
            "prompt": "You locate and read files and report what they contain, quoting exactly. You do not modify anything, and you report what you could not determine rather than filling it in.",
            "tools": HELPER_TOOLS,
            "model": model,
        }
    });
    Ok(serde_json::to_string(&definition)?)
}

/// How far a helper got, as the harness reported it.
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
}

impl Observations {
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
    fn observe_foreign(&mut self, _harness: &str, event: &Value) {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
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
        let summary = event
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let output_file = event
            .get("output_file")
            .and_then(Value::as_str)
            .map(str::to_string);
        let tokens = event.pointer("/usage/total_tokens").and_then(Value::as_u64);
        let helper = self.slot(&task_id);
        helper.status = status;
        if !summary.is_empty() {
            helper.summary = summary;
        }
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
                    self.reported_by_type.insert(role.clone(), count);
                }
            }
        }
    }

    /// Assess whether the owning task may report this attempt as finished work.
    ///
    /// `complete` is false whenever a helper started without reaching a terminal
    /// event, whenever an observation contradicts the frozen policy, whenever the
    /// stream ended without a terminal harness event, or whenever the harness
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
        if !self.terminal_seen {
            unknown.push("the attempt ended without a terminal harness event".to_string());
        }

        // Two separate questions, kept apart because conflating them is what let
        // an attempt report itself complete while naming helpers it never saw.
        // The attempt can end cleanly and still rest on evidence ahu does not
        // have; only when both hold is there nothing outstanding.
        let terminated_cleanly = unjoined.is_empty() && violations.is_empty() && self.terminal_seen;
        let evidence_complete = unobserved.is_empty() && self.unknown_events == 0;
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
    /// stream ended with a terminal harness event.
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

use serde::{Deserialize, Serialize};

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
///     and with them shell-launched subprocesses;
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
            "native.helper.no_shell",
            Mechanism::Structural,
            "no shell tool is in the ceiling; this does not constrain hook or plugin subprocesses",
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

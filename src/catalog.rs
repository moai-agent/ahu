//! The compatibility catalog: which harness/model pairs ahu knows how to launch.
//!
//! The catalog is data, not policy. It records what a pair *is* — a verified
//! identifier, the harness versions it was checked against, and the basis for
//! its quality rank. Which pair a project actually uses is decided by the
//! project's own rankings in `.agents/ahu/config.toml`.
//!
//! The catalog is pinned by version in project configuration so that installing
//! a newer ahu cannot silently change a project's selection.

use crate::bail;
use crate::util::Result;

/// The catalog revision shipped with this ahu build.
pub const CATALOG_VERSION: &str = "2026-09-12";

/// A harness ahu can name. Only harnesses with a validated adapter can launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessEntry {
    pub id: &'static str,
    pub display_name: &'static str,
    /// Whether this ahu build ships a validated adapter for the harness.
    pub adapter_available: bool,
    /// Executable ahu probes to decide whether the harness is installed here.
    pub executable: &'static str,
    /// Harness versions the adapter was verified against.
    pub verified_versions: &'static str,
    /// Whether the adapter can hold the configured model for a whole session.
    pub enforces_model_for_session: bool,
    /// What the adapter cannot control, shown with the reliability warning.
    pub enforcement_gaps: &'static [&'static str],
}

/// A verified harness/model pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelEntry {
    pub harness: &'static str,
    /// The exact identifier passed to the harness. Never an alias.
    pub model: &'static str,
    pub display_name: &'static str,
    /// Lower is better. This is a reviewed project-facing ranking hint, not a
    /// universal intelligence claim.
    pub quality_rank: u8,
    pub evaluation_basis: &'static str,
    pub reviewed_on: &'static str,
    /// `true` when the identifier is known to be a moving provider alias.
    pub is_moving_alias: bool,
}

pub const HARNESSES: &[HarnessEntry] = &[
    HarnessEntry {
        id: "claude-code",
        display_name: "Claude Code",
        adapter_available: true,
        executable: "claude",
        verified_versions: "2.1.269",
        enforces_model_for_session: false,
        enforcement_gaps: &[
            "the model is set at launch with --model, but an interactive session can change it with /model",
            "ahu cannot disable in-session model switching or provider-side routing",
        ],
    },
    HarnessEntry {
        id: "codex",
        display_name: "Codex",
        adapter_available: true,
        executable: "codex",
        verified_versions: "0.154.0",
        enforces_model_for_session: false,
        enforcement_gaps: &[
            "the model is set at launch with -m, but ahu cannot stop an interactive session changing it",
            "Codex has no per-agent selection: there is no --agent flag and no instructions-file option, so a named agent's system prompt is NOT delivered by ahu. Put the task's instructions in the prompt",
            "`codex agents` lists running sessions, not definitions, so .codex/agents/<name>.toml is not a launchable source for this CLI version",
        ],
    },
    HarnessEntry {
        id: "antigravity",
        display_name: "Antigravity CLI",
        adapter_available: true,
        executable: "agy",
        verified_versions: "1.2.2",
        enforces_model_for_session: false,
        enforcement_gaps: &[
            "the model is set at launch with --model, but ahu cannot stop an interactive session changing it",
            "--agent is accepted but not validated: a nonexistent agent name produced a normal reply instead of an error, and a workspace agent whose instructions were unmistakable did not change the response. ahu requests the agent but cannot confirm the harness loaded it, so treat the system prompt as NOT enforced and put the task's instructions in the prompt",
            "ahu verifies the agent definition exists in the repository; the harness does not confirm it",
        ],
    },
];

/// Claude Code model identifiers, checked against the Claude Code CLI's
/// `--model` documentation and Anthropic's published model list on the review
/// date below. Aliases such as `opus` are deliberately absent: a named agent
/// must pin an exact identifier.
pub const MODELS: &[ModelEntry] = &[
    ModelEntry {
        harness: "claude-code",
        model: "claude-opus-5",
        display_name: "Claude Opus 5",
        quality_rank: 0,
        evaluation_basis: "published vendor capability tier; not evaluated on this project's tasks",
        reviewed_on: "2026-09-12",
        is_moving_alias: false,
    },
    ModelEntry {
        harness: "claude-code",
        model: "claude-sonnet-5",
        display_name: "Claude Sonnet 5",
        quality_rank: 1,
        evaluation_basis: "published vendor capability tier; not evaluated on this project's tasks",
        reviewed_on: "2026-09-12",
        is_moving_alias: false,
    },
    ModelEntry {
        harness: "claude-code",
        model: "claude-haiku-4-5",
        display_name: "Claude Haiku 4.5",
        quality_rank: 2,
        evaluation_basis: "published vendor capability tier; not evaluated on this project's tasks",
        reviewed_on: "2026-09-12",
        is_moving_alias: false,
    },
    // Observed in the installed Codex CLI's own model-availability state on the
    // review date. ahu does not verify account entitlement for either.
    ModelEntry {
        harness: "codex",
        model: "gpt-6-astra",
        display_name: "GPT-6 Astra",
        quality_rank: 0,
        evaluation_basis: "offered by the installed codex-cli; entitlement not verified by ahu",
        reviewed_on: "2026-09-12",
        is_moving_alias: false,
    },
    ModelEntry {
        harness: "codex",
        model: "gpt-5.5",
        display_name: "GPT-5.5",
        quality_rank: 1,
        evaluation_basis: "offered by the installed codex-cli; entitlement not verified by ahu",
        reviewed_on: "2026-09-12",
        is_moving_alias: false,
    },
    // Identifiers reported by `agy models` on the review date.
    ModelEntry {
        harness: "antigravity",
        model: "gemini-3.1-pro-high",
        display_name: "Gemini 3.1 Pro (High)",
        quality_rank: 0,
        evaluation_basis: "listed by `agy models` on the installed CLI; not evaluated on this project's tasks",
        reviewed_on: "2026-09-12",
        is_moving_alias: false,
    },
    ModelEntry {
        harness: "antigravity",
        model: "gemini-3.1-pro-low",
        display_name: "Gemini 3.1 Pro (Low)",
        quality_rank: 1,
        evaluation_basis: "listed by `agy models` on the installed CLI; not evaluated on this project's tasks",
        reviewed_on: "2026-09-12",
        is_moving_alias: false,
    },
    ModelEntry {
        harness: "antigravity",
        model: "gemini-3.8-flash-high",
        display_name: "Gemini 3.8 Flash (High)",
        quality_rank: 2,
        evaluation_basis: "listed by `agy models` on the installed CLI; not evaluated on this project's tasks",
        reviewed_on: "2026-09-12",
        is_moving_alias: false,
    },
];

pub fn harness(id: &str) -> Option<&'static HarnessEntry> {
    HARNESSES.iter().find(|h| h.id == id)
}

pub fn model(harness_id: &str, model_id: &str) -> Option<&'static ModelEntry> {
    MODELS
        .iter()
        .find(|m| m.harness == harness_id && m.model == model_id)
}

/// Catalog models for one harness, best-ranked first.
pub fn models_for(harness_id: &str) -> Vec<&'static ModelEntry> {
    let mut found: Vec<_> = MODELS.iter().filter(|m| m.harness == harness_id).collect();
    found.sort_by_key(|m| m.quality_rank);
    found
}

/// Reject a project config pinned to a catalog this build does not ship.
///
/// A mismatch is reported rather than silently upgraded: changing the catalog
/// changes which model a project's automatic launches resolve to, and that is a
/// shared project decision.
pub fn require_version(pinned: &str) -> Result<()> {
    if pinned != CATALOG_VERSION {
        // `{pinned:?}` rather than `{pinned}`: this value is chosen by the
        // repository, and every renderer that prints it should escape it. The
        // callers do, but a message that carries repository bytes verbatim is
        // one refactor away from reaching a terminal that does not.
        bail!(
            "this project pins compatibility catalog {pinned:?}, but this ahu build ships {CATALOG_VERSION}.\n\
             Selection behaviour differs between catalogs, so ahu will not substitute one for the other.\n\
             Install the ahu release carrying catalog {pinned:?}, or agree a project change to catalog {CATALOG_VERSION} \
             by editing `catalog_version` in .agents/ahu/config.toml."
        );
    }
    Ok(())
}

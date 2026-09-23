//! The compatibility catalog: which harness/model pairs ahu knows how to launch.
//!
//! The catalog is data, not policy. It records what a pair *is* — a verified
//! identifier, the harness versions it was checked against, and the basis for
//! its quality rank. Which pair a project actually uses is decided by the
//! project's own rankings in `.agents/ahu/config.toml`.
//!
//! The catalog revision is fixed in project configuration so that installing
//! a newer ahu cannot silently change a project's selection.

use crate::bail;
use crate::util::Result;

/// The catalog revision shipped with this ahu build.
pub const CATALOG_VERSION: &str = "2026-09-13";

/// One capability a harness adapter has actually been validated to deliver.
///
/// A feature is admitted only after live confirmation on a verified CLI
/// version, never from documentation alone. It describes what the harness *can* do; the
/// project decides what a launch uses, and harness configuration itself is a
/// project-level matter ahu never writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// The adapter can start an interactive session.
    InteractiveLaunch,
    /// The adapter has a validated batch (`--headless`/`launch --headless`) mode.
    HeadlessLaunch,
    /// A finished batch session can be resumed by id.
    HeadlessResume,
    /// The harness can be told to ask for approval on tool use.
    PromptApprovals,
    /// The harness offers an accept-edits approval mode.
    AcceptEditsApprovals,
    /// The harness offers a mode that skips approvals entirely.
    AutoApprovals,
    /// ahu can enumerate the harness's hook configuration.
    HookInventory,
    /// ahu can enumerate plugin modules the harness installs at startup.
    StartupPluginInventory,
    /// Models are qualified by provider (for example `provider/model:tag`).
    ProviderQualifiedModels,
    /// The harness CLI supports a deterministic external log destination.
    ExternalLogDestination,
    /// The harness supports bounded native helper delegation.
    BoundedNativeHelpers,
}

impl Feature {
    pub fn as_str(self) -> &'static str {
        match self {
            Feature::InteractiveLaunch => "interactive-launch",
            Feature::HeadlessLaunch => "headless-launch",
            Feature::HeadlessResume => "headless-resume",
            Feature::PromptApprovals => "prompt-approvals",
            Feature::AcceptEditsApprovals => "accept-edits-approvals",
            Feature::AutoApprovals => "auto-approvals",
            Feature::HookInventory => "hook-inventory",
            Feature::StartupPluginInventory => "startup-plugin-inventory",
            Feature::ProviderQualifiedModels => "provider-qualified-models",
            Feature::ExternalLogDestination => "external-log-destination",
            Feature::BoundedNativeHelpers => "bounded-native-helpers",
        }
    }

    /// What claiming the feature means, rendered beside it in the feature
    /// matrix so the matrix never asserts a stronger claim than ahu makes.
    pub fn gloss(self) -> &'static str {
        match self {
            Feature::InteractiveLaunch => "ahu can start an interactive session of this harness",
            Feature::HeadlessLaunch => "ahu has a validated batch launch profile for this harness",
            Feature::HeadlessResume => "a finished batch session can be resumed by id",
            Feature::PromptApprovals => "the harness can be told to ask for approval on tool use",
            Feature::AcceptEditsApprovals => "the harness offers an accept-edits approval mode",
            Feature::AutoApprovals => "the harness offers a mode that skips approvals entirely",
            Feature::HookInventory => "ahu can enumerate this harness's hook configuration",
            Feature::StartupPluginInventory => {
                "ahu can enumerate plugin modules this harness installs at startup"
            }
            Feature::ProviderQualifiedModels => "model identifiers are qualified by provider",
            Feature::ExternalLogDestination => {
                "the CLI supports a deterministic external log destination"
            }
            Feature::BoundedNativeHelpers => {
                "the harness supports bounded native helper delegation"
            }
        }
    }
}

/// All features in stable matrix order.
pub const FEATURES: &[Feature] = &[
    Feature::InteractiveLaunch,
    Feature::HeadlessLaunch,
    Feature::HeadlessResume,
    Feature::PromptApprovals,
    Feature::AcceptEditsApprovals,
    Feature::AutoApprovals,
    Feature::HookInventory,
    Feature::StartupPluginInventory,
    Feature::ProviderQualifiedModels,
    Feature::ExternalLogDestination,
    Feature::BoundedNativeHelpers,
];

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
    /// Harness versions the headless (batch) profile was verified against.
    pub headless_verified_versions: &'static [&'static str],
    /// Whether the adapter can hold the configured model for a whole session.
    pub enforces_model_for_session: bool,
    /// Capabilities this harness's adapter has been validated to deliver.
    pub features: &'static [Feature],
    /// What the adapter cannot control, shown with the reliability warning.
    pub enforcement_gaps: &'static [&'static str],
}

impl HarnessEntry {
    pub fn supports(&self, feature: Feature) -> bool {
        self.features.contains(&feature)
    }
}

/// Whether a named harness supports a feature. `false` for an unknown harness:
/// an unvalidated adapter supports nothing.
pub fn supports(harness_id: &str, feature: Feature) -> bool {
    harness(harness_id)
        .map(|h| h.supports(feature))
        .unwrap_or(false)
}

/// Static capability and isolation contract, separate from local evidence.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IsolationProfile {
    pub harness: &'static str,
    pub headless_verified_versions: &'static [&'static str],
    pub version_policy: &'static str,
    pub evidence: &'static str,
    pub unknown_integration_opt_in: bool,
    pub interactive_supported: bool,
    pub limitations: &'static str,
}

pub fn isolation_profile(id: &str) -> Option<IsolationProfile> {
    let entry = harness(id)?;
    let evidence = match id {
        "codex" => {
            "Absent native sources or exact reviewed hook commands with disable guards; plugins, cloud/authentication and managed sources must be resolved."
        }
        "opencode" => {
            "Absent native sources or the exact reviewed guarded Session plugin; Feed is unsafe, and authentication/account stores, substitutions and declared modules are unresolved."
        }
        "claude-code" => {
            "Direct executable bypasses the cmux wrapper; separately configured hooks, enabled plugins and managed settings must be resolved."
        }
        "antigravity" => {
            "Absent inspected hook configuration; custom hooks, extensions and configuration overrides are unverified."
        }
        _ => return None,
    };
    Some(IsolationProfile {
        harness: entry.id,
        headless_verified_versions: entry.headless_verified_versions,
        version_policy: "exact reviewed CLI version; missing or unvalidated versions refuse headless execution",
        evidence,
        unknown_integration_opt_in: false,
        interactive_supported: entry.supports(Feature::InteractiveLaunch),
        limitations: "Bounded local inspection is not live conformance, authentication validation or a sandbox. Interactive support still requires normal launch prerequisites and approval checks.",
    })
}

/// Shared by preflight disclosure and the execution gate. Never widens a version
/// range based on an installed CLI or a successful interactive launch.
pub fn check_headless_version(harness: &str, version: &str) -> Result<()> {
    let supported = isolation_profile(harness)
        .map(|p| p.headless_verified_versions)
        .unwrap_or(&[]);
    // Preserve native CLI-name prefixes and whitespace-delimited decoration;
    // empty, malformed, and unreviewed version tokens still fail closed.
    let token = version
        .split_whitespace()
        .find(|v| crate::util::is_semver(v))
        .unwrap_or_else(|| version.split_whitespace().next().unwrap_or(""));
    if !supported.contains(&token) {
        bail!(
            "unvalidated headless {harness} version {version:?}; supported CLI profiles: {supported:?}. Update the compatibility validation before launching; no fallback was selected."
        );
    }
    Ok(())
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
        // Interactive and headless surfaces were validated on different sets:
        // the interactive adapter was checked on 2.1.269, the batch argument
        // surface and event stream on both.
        headless_verified_versions: &["2.1.269", "2.1.270"],
        enforces_model_for_session: false,
        features: &[
            Feature::InteractiveLaunch,
            Feature::HeadlessLaunch,
            Feature::HeadlessResume,
            Feature::PromptApprovals,
            Feature::AcceptEditsApprovals,
            Feature::AutoApprovals,
            Feature::HookInventory,
            Feature::BoundedNativeHelpers,
        ],
        enforcement_gaps: &[
            "the model is set at launch with --model, but an interactive session can change it with /model",
            "ahu cannot disable in-session model switching or provider-side routing",
            "--agent <name> selects through Claude Code's own agent search, which resolves a name and is not bound to the file ahu reads and digests. ahu does not pass it, because it cannot check the binding it would be asserting",
        ],
    },
    HarnessEntry {
        id: "codex",
        display_name: "Codex",
        adapter_available: true,
        executable: "codex",
        verified_versions: "0.154.0",
        // 0.155.1 retains the batch argv surface; a live JSON launch and
        // native-session resume both emitted the expected identity and terminal events.
        headless_verified_versions: &["0.154.0", "0.155.1"],
        enforces_model_for_session: false,
        features: &[
            Feature::InteractiveLaunch,
            Feature::HeadlessLaunch,
            Feature::HeadlessResume,
            Feature::PromptApprovals,
            Feature::AcceptEditsApprovals,
            Feature::AutoApprovals,
        ],
        enforcement_gaps: &[
            "the model is set at launch with -m, but ahu cannot stop an interactive session changing it",
            "Codex has no per-agent selection: no --agent flag and no instructions-file option, and -p/--profile layers model and sandbox config rather than instructions",
            "`codex agents` lists running sessions, not definitions, so .codex/agents/<name>.toml is not a launchable source for this CLI version",
        ],
    },
    HarnessEntry {
        id: "antigravity",
        display_name: "Antigravity CLI",
        adapter_available: true,
        executable: "agy",
        verified_versions: "1.2.2",
        headless_verified_versions: &["1.2.2"],
        enforces_model_for_session: false,
        features: &[
            Feature::InteractiveLaunch,
            Feature::HeadlessLaunch,
            Feature::HeadlessResume,
            Feature::PromptApprovals,
            Feature::AcceptEditsApprovals,
            Feature::AutoApprovals,
            Feature::ExternalLogDestination,
        ],
        enforcement_gaps: &[
            "the model is set at launch with --model, but ahu cannot stop an interactive session changing it",
            "the CLI accepts --agent but does not validate it: a nonexistent agent name produced a normal reply instead of an error, and a workspace agent whose instructions were unmistakable did not change the response, so the harness never confirms an identity was applied. ahu does not pass it",
        ],
    },
    HarnessEntry {
        id: "opencode",
        display_name: "OpenCode",
        adapter_available: true,
        executable: "opencode",
        // Both were checked on 2026-09-13; the install updated itself in place
        // between the two probe runs, which is why the entry lists a pair.
        verified_versions: "1.18.29, 1.18.30, 1.18.31",
        // The batch surface was inspected on 1.18.30; 1.18.29 carries the same
        // `run` options and is the other version the catalog entry names.
        // 1.18.31 was live-probed on 2026-09-15: a fresh `run --format json`
        // turn and a `--session` resume that recalled context both behaved.
        headless_verified_versions: &["1.18.29", "1.18.30", "1.18.31"],
        enforces_model_for_session: false,
        features: &[
            Feature::InteractiveLaunch,
            Feature::HeadlessLaunch,
            Feature::HeadlessResume,
            Feature::AutoApprovals,
            Feature::HookInventory,
            Feature::StartupPluginInventory,
            Feature::ProviderQualifiedModels,
        ],
        enforcement_gaps: &[
            "the model is set at launch with --model, but ahu cannot stop an interactive session changing it",
            "--agent <name> is accepted but not validated: a missing name only warns \"agent ... not found. Falling back to default agent\" and the run continues, so the flag can never confirm an identity was applied. It would also override the agent's own model and permissions, contradicting the model ahu pins. ahu does not pass it",
            "OpenCode defaults most tool permissions to allow, so an agent declaring permissions = prompt does not mean OpenCode asks before acting; the effective boundary comes from the user's own OpenCode configuration, not from any flag ahu passes",
            "inference for an ollama/*:cloud model is performed by Ollama's cloud service reached through the local endpoint; it is not local inference, and ahu neither holds nor checks those credentials",
            "OpenCode has no sandbox of its own and ahu passes none, so its file tools act on whatever absolute path the model names. Observed under --auto on 1.18.30: a write landed in the parent checkout rather than the task worktree ahu launched in, where `ahu diff` does not look. ahu discloses such paths: headless attempts record write-tool targets outside the worktree in the result envelope, and an empty `ahu diff` says so on stderr. The disclosure is post-run review information, not a boundary; the worktree is still where the session starts, not a wall",
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
    // Routed by OpenCode through an `ollama` provider the user configures; the
    // `:cloud` tag runs on Ollama's cloud service reached through the local
    // endpoint, so entitlement is an account matter ahu does not verify.
    ModelEntry {
        harness: "opencode",
        model: "ollama/glm-5.3:cloud",
        display_name: "GLM 5.3 (Ollama cloud)",
        quality_rank: 0,
        evaluation_basis: "offered by the local Ollama endpoint as digest 8477dab3e25b on 2026-09-13; a cloud entitlement ahu does not verify, and not evaluated on this project's tasks",
        reviewed_on: "2026-09-13",
        is_moving_alias: true,
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

/// Reject a project config requiring a catalog this build does not ship.
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

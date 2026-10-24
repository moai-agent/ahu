//! Harness adapters.
//!
//! An adapter's job is to turn a frozen launch identity into an exact argument
//! vector, and to state honestly what it can and cannot enforce. It may not
//! translate an agent to another harness, drop native settings, or widen
//! permissions beyond the manifest request.

pub mod antigravity;
pub mod claude_code;
pub mod codex;
pub mod codex_metadata;
pub mod isolation;
pub mod kiro;
pub(crate) mod native_trust;
pub mod opencode;

use serde::{Deserialize, Serialize};

use crate::bail;
use crate::util::Result;

/// Legacy task-record wording. Retained for readers and compatibility tests;
/// normal harness capabilities do not produce reliability warnings.
pub const RELIABILITY_WARNING: &str =
    "This harness is not reliable for producing consistent personified agent behavior.";

/// Everything an adapter needs to build a launch command.
#[derive(Debug, Clone)]
pub struct LaunchRequest<'a> {
    /// Exact model identifier from the agent manifest or the frozen policy.
    pub model: &'a str,
    /// Everything the harness receives in its prompt slot: ahu's fenced
    /// delegation contract, the resolved agent's fenced instructions, and the
    /// task prompt, composed by `crate::orchestration`.
    ///
    /// There is no separate agent-selection or system-prompt channel. Adapters
    /// receive composed text, not a name to resolve through native agent search.
    pub prompt: &'a str,
    /// Working directory: the task worktree.
    pub cwd: &'a std::path::Path,
    /// Approval widening the agent explicitly asked for, if any.
    pub permissions: crate::agent::Permissions,
}

/// A launch reduced to an executable plus an argument vector.
///
/// There is no shell string here by design. The prompt travels as one argv
/// element, so shell syntax inside it cannot be interpreted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LaunchCommand {
    pub program: String,
    pub args: Vec<String>,
    /// Digest of the committed planner binding; prevents changing planner policy
    /// or rebuilding a plugin-planned task through a built-in adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planner_binding_digest: Option<String>,
    /// Index in `args` holding the task prompt, when one is passed there.
    ///
    /// Task records store the command with this element replaced, so a prompt is
    /// never written to a second file with weaker permissions than the one ahu
    /// deliberately protects.
    #[serde(default)]
    pub prompt_arg: Option<usize>,
}

/// Placeholder standing in for the prompt in a stored command.
pub const REDACTED_PROMPT: &str = "<prompt: see prompt.txt>";

/// Set the native task directory when launching a harness process.
pub(crate) fn configure_task_directory(
    command: &mut std::process::Command,
    harness: &str,
    cwd: &std::path::Path,
) {
    command.current_dir(cwd);
    if harness == "opencode" {
        // OpenCode 2's mini and run prefer PWD over process.cwd() and chdir
        // there. Command::current_dir alone leaves the parent's PWD intact.
        command.env("PWD", cwd);
    }
}

impl LaunchCommand {
    /// The command with the prompt replaced by a placeholder, for storage and
    /// for display.
    pub fn redacted(&self) -> LaunchCommand {
        let mut copy = self.clone();
        if let Some(index) = copy.prompt_arg
            && let Some(slot) = copy.args.get_mut(index)
        {
            *slot = REDACTED_PROMPT.to_string();
        }
        copy
    }
}

/// The exact-model option a harness CLI takes, with the checks that option needs.
///
/// One mapping serves the headless adapters and the coordinator shortcuts in
/// `crate::commands`, so a coordinating session cannot pass a flag an adapter
/// does not use, and a model an adapter would refuse is refused there too.
///
/// Verified against the installed CLIs' own help: Claude Code, the Antigravity
/// CLI and OpenCode take `--model`; Codex takes `-m` (its `--model` alias is
/// equivalent, and `-m` is what the catalog's enforcement gap names).
pub fn model_args(harness_id: &str, model: &str) -> Result<Vec<String>> {
    let entry = crate::catalog::harness(harness_id).ok_or_else(|| {
        crate::util::Error::new(format!(
            "the compatibility catalog has no entry for harness {harness_id:?}, so ahu cannot \
             name the option that CLI takes for a model. This is an ahu build problem, not a \
             repository one."
        ))
    })?;
    let name = entry.display_name;
    let flag = match harness_id {
        "claude-code" | "antigravity" | "opencode" | "kiro" => "--model",
        "codex" => "-m",
        other => {
            bail!("ahu has no validated model option for harness {other:?}. It will not guess one.")
        }
    };
    if model.is_empty() {
        bail!("the {name} adapter requires an exact model identifier.");
    }
    if model.starts_with('-') {
        bail!("model identifier {model:?} would be read as an option by the {name} CLI.");
    }
    // OpenCode documents `--model` as `provider/model`. An unqualified
    // identifier is a silent-misroute risk: which provider it would reach
    // depends on the user's configuration, and ahu will not guess one.
    if entry.supports(crate::catalog::Feature::ProviderQualifiedModels)
        && !model.split_once('/').is_some_and(|(provider, rest)| {
            !provider.is_empty() && !rest.is_empty() && !rest.contains('/')
        })
    {
        bail!(
            "model identifier {model:?} is not provider-qualified. {name}'s --model takes \
             <provider>/<model>, where the provider half names one of the providers \
             {name}'s own configuration defines. ahu will not guess which provider an \
             unqualified identifier means; name it in the manifest."
        );
    }
    Ok(vec![flag.to_string(), model.to_string()])
}

/// What an adapter can actually guarantee about the session it starts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnforcementReport {
    pub harness: String,
    pub harness_version: Option<String>,
    /// Whether the configured model is held for the whole session.
    pub model_fixed_for_session: bool,
    /// Specific controls the adapter does not have.
    pub gaps: Vec<String>,
    /// Native controls the adapter did set.
    pub applied_controls: Vec<String>,
}

pub trait Adapter {
    fn id(&self) -> &'static str;
    /// Build the exact command for a launch.
    fn launch_command(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand>;
    /// Report what this adapter can enforce on this machine.
    ///
    /// Takes the launch's `permissions` because the report describes the flags
    /// this adapter actually passes, and those depend on it. A report built
    /// without it can only assert a fixed list, which is how the Enforcement
    /// block came to claim ahu passes no `--permission-mode` on a launch that
    /// passes exactly that.
    ///
    /// Fallible because the report is built from the compatibility catalog. The
    /// entry is there in every shipped build, but a catalog edit that does not
    /// update an adapter must surface as an error the caller can print, not as
    /// a panic inside a harness adapter.
    fn enforcement(
        &self,
        model: &str,
        permissions: crate::agent::Permissions,
    ) -> Result<EnforcementReport>;
}

pub fn adapter_for(harness_id: &str) -> Result<Box<dyn Adapter>> {
    match harness_id {
        "claude-code" => Ok(Box::new(claude_code::ClaudeCode)),
        "codex" => Ok(Box::new(codex::Codex)),
        "antigravity" => Ok(Box::new(antigravity::Antigravity)),
        "opencode" => Ok(Box::new(opencode::OpenCode)),
        "kiro" => Ok(Box::new(kiro::Kiro)),
        other => bail!(
            "ahu has no validated adapter for harness {other:?}. \
             It will not substitute another harness."
        ),
    }
}

/// Pilot dispatch; the selected external path never falls back on failure.
pub(crate) fn task_command(
    repo: &crate::git::Repo,
    harness: &str,
    request: &LaunchRequest<'_>,
    batch: Option<&crate::headless::Spec>,
) -> Result<LaunchCommand> {
    if cfg!(feature = "codex-plugin-pilot") && harness == "codex" {
        return crate::harness_plugin::plan(repo, harness, request, batch);
    }
    if let Some(spec) = batch {
        crate::headless::batch_command(harness, request, spec)
    } else {
        adapter_for(harness)?.launch_command(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_lookup_fails_closed_for_unregistered_harnesses() {
        assert!(adapter_for("codex").is_ok());
        assert!(
            adapter_for("unknown")
                .err()
                .unwrap()
                .to_string()
                .contains("no validated adapter")
        );
    }

    #[test]
    fn model_arguments_require_an_exact_safe_catalog_model() {
        assert_eq!(model_args("codex", "gpt-5.5").unwrap(), ["-m", "gpt-5.5"]);
        assert_eq!(
            model_args("claude-code", "claude-sonnet").unwrap(),
            ["--model", "claude-sonnet"]
        );
        assert!(
            model_args("missing", "model")
                .unwrap_err()
                .to_string()
                .contains("no entry for harness")
        );
        assert!(
            model_args("codex", "")
                .unwrap_err()
                .to_string()
                .contains("exact model")
        );
        assert!(
            model_args("codex", "--help")
                .unwrap_err()
                .to_string()
                .contains("option")
        );
        assert!(
            model_args("opencode", "model-without-provider")
                .unwrap_err()
                .to_string()
                .contains("provider-qualified")
        );
        assert!(
            model_args("opencode", "/model")
                .unwrap_err()
                .to_string()
                .contains("provider-qualified")
        );
        assert!(
            model_args("opencode", "provider/")
                .unwrap_err()
                .to_string()
                .contains("provider-qualified")
        );
    }
}

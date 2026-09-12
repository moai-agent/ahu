//! Harness adapters.
//!
//! An adapter's job is to turn a frozen launch identity into an exact argument
//! vector, and to state honestly what it can and cannot enforce. It may not
//! translate an agent to another harness, drop native settings, or widen
//! permissions.

pub mod antigravity;
pub mod claude_code;
pub mod codex;

use serde::{Deserialize, Serialize};

use crate::bail;
use crate::util::Result;

/// The prominent warning ahu shows when a harness cannot hold the configured
/// identity for a whole session. The wording is fixed so it is recognisable.
pub const RELIABILITY_WARNING: &str =
    "This harness is not reliable for producing consistent personified agent behavior.";

/// Everything an adapter needs to build a launch command.
#[derive(Debug, Clone)]
pub struct LaunchRequest<'a> {
    /// Exact model identifier from the agent manifest or the frozen policy.
    pub model: &'a str,
    /// Native agent name to select in the harness, when the launch is named and
    /// the harness has a native concept for it.
    pub native_agent: Option<&'a str>,
    /// The task prompt, delivered as data. Never interpolated into a shell.
    pub prompt: &'a str,
    /// Working directory: the task worktree.
    pub cwd: &'a std::path::Path,
}

/// A launch reduced to an executable plus an argument vector.
///
/// There is no shell string here by design. The prompt travels as one argv
/// element, so shell syntax inside it cannot be interpreted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LaunchCommand {
    pub program: String,
    pub args: Vec<String>,
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

/// Detect whether the harness binary ahu resolved is really a wrapper.
///
/// cmux installs shim scripts on `PATH` that `exec` its own wrapper around the
/// real harness, and those wrappers add flags of their own. Observed on cmux
/// 0.64.22: the Codex wrapper enables `--dangerously-bypass-hook-trust`, a flag
/// ahu deliberately never passes. ahu cannot see or control what a wrapper adds,
/// so it says the wrapper is there instead of claiming the harness's own
/// defaults are untouched.
pub fn wrapper_interposed(executable: &std::path::Path) -> Option<String> {
    let path = executable.to_string_lossy();
    if path.contains("cmux-cli-shims") {
        return Some(format!(
            "the harness binary on PATH is a cmux shim ({path}), which execs cmux's own wrapper.              The wrapper may add flags ahu does not pass and cannot inspect"
        ));
    }
    for variable in [
        "CMUX_CLAUDE_WRAPPER_SHIM",
        "CMUX_CODEX_WRAPPER_SHIM",
        "CMUX_AGY_WRAPPER_SHIM",
    ] {
        if let Some(shim) = std::env::var_os(variable)
            && shim.to_string_lossy() == path
        {
            return Some(format!(
                "the harness binary on PATH is the cmux wrapper shim named by {variable}.                  The wrapper may add flags ahu does not pass and cannot inspect"
            ));
        }
    }
    None
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

impl EnforcementReport {
    pub fn needs_reliability_warning(&self) -> bool {
        !self.model_fixed_for_session
    }
}

pub trait Adapter {
    fn id(&self) -> &'static str;
    /// Build the exact command for a launch.
    fn launch_command(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand>;
    /// Report what this adapter can enforce on this machine.
    fn enforcement(&self, model: &str) -> EnforcementReport;
}

pub fn adapter_for(harness_id: &str) -> Result<Box<dyn Adapter>> {
    match harness_id {
        "claude-code" => Ok(Box::new(claude_code::ClaudeCode)),
        "codex" => Ok(Box::new(codex::Codex)),
        "antigravity" => Ok(Box::new(antigravity::Antigravity)),
        other => bail!(
            "ahu 0.1.1 has no validated adapter for harness {other:?}. \
             It will not substitute another harness."
        ),
    }
}

//! Harness adapters.
//!
//! An adapter's job is to turn a frozen launch identity into an exact argument
//! vector, and to state honestly what it can and cannot enforce. It may not
//! translate an agent to another harness, drop native settings, or widen
//! permissions beyond the manifest request.

pub mod codex_metadata;
pub(crate) mod native_trust;

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
    /// Plugin evidence that native export is compatible with the eval receiver.
    pub native_eval_otlp: bool,
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
    _harness: &str,
    cwd: &std::path::Path,
) {
    command.current_dir(cwd);
    // Keep the standard working-directory environment consistent with cwd for
    // every native process. Plugins need no provider-specific environment hook.
    command.env("PWD", cwd);
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

/// Resolve native launch behavior exclusively through the installed runtime.
pub(crate) fn task_command(
    repo: &crate::git::Repo,
    harness: &str,
    request: &LaunchRequest<'_>,
    batch: Option<&crate::headless::Spec>,
) -> Result<LaunchCommand> {
    let runtime = crate::harness_runtime::load(repo, harness)?;
    let version = match batch {
        Some(spec) => spec.harness_version.clone(),
        None => probe_version(repo, &runtime)?,
    };
    let value = runtime.call(
        "launch",
        serde_json::json!({
            "mode": if batch.is_some() { "headless" } else { "interactive" },
            "model": request.model, "permissions": request.permissions,
            "work_mode": batch.is_some_and(|s| s.options.work_mode),
            "session": batch.and_then(|s| s.session.as_deref()),
            "native_version": version,
            "timeout_seconds": batch.map_or(1800, |s| s.options.timeout_seconds),
            "cwd": request.cwd,
            "broker_dir": batch.and_then(|s| s.broker_dir.as_ref()),
            "native_profile": batch.and_then(|s| s.native_profile.as_ref()),
            "native_controls": batch.map(|s| &s.native_controls).cloned().unwrap_or_default(),
        }),
    )?;
    decode_plan(
        value,
        runtime.descriptor().program.as_str(),
        Some(request.prompt),
        runtime.binding_digest(),
    )
}

/// A coordinator's direct session has no injected task prompt.
pub(crate) fn direct_command(
    repo: &crate::git::Repo,
    harness: &str,
    model: &str,
) -> Result<LaunchCommand> {
    let runtime = crate::harness_runtime::load(repo, harness)?;
    let version = probe_version(repo, &runtime)?;
    let value = runtime.call(
        "launch",
        serde_json::json!({
            "mode": "direct", "model": model, "permissions": "prompt", "work_mode": false,
            "session": null, "native_version": version, "timeout_seconds": 1800,
            "cwd": repo.root, "broker_dir": null, "native_profile": null, "native_controls": [],
        }),
    )?;
    decode_plan(
        value,
        runtime.descriptor().program.as_str(),
        None,
        runtime.binding_digest(),
    )
}

pub(crate) fn probe_version(
    repo: &crate::git::Repo,
    runtime: &crate::harness_runtime::Runtime,
) -> Result<String> {
    let value = runtime.call(
        "probe",
        crate::harness_runtime::native_params(repo, runtime.descriptor())?,
    )?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_str)
        .filter(|v| crate::util::is_semver(v))
        .ok_or_else(|| crate::util::Error::new("runtime returned an invalid native version"))?;
    Ok(version.to_owned())
}

fn decode_plan(
    value: serde_json::Value,
    program: &str,
    prompt: Option<&str>,
    digest: String,
) -> Result<LaunchCommand> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Plan {
        native_eval_otlp: bool,
        program: String,
        args: Vec<String>,
        prompt_arg: Option<usize>,
    }
    let mut plan: Plan = serde_json::from_value(value)
        .map_err(|_| crate::util::Error::new("invalid native launch plan"))?;
    if plan.program != program
        || plan.args.len() > 256
        || plan
            .args
            .iter()
            .any(|s| s.contains('\0') || s.len() > 65536)
        || plan.args.iter().map(String::len).sum::<usize>() > 262144
    {
        bail!("native launch plan violates executable or argument bounds");
    }
    let placeholders = plan
        .args
        .iter()
        .filter(|s| s.as_str() == REDACTED_PROMPT)
        .count();
    match (prompt, plan.prompt_arg) {
        (Some(prompt), Some(index))
            if index.checked_add(1) == Some(plan.args.len())
                && placeholders == 1
                && plan.args[index] == REDACTED_PROMPT =>
        {
            plan.args[index] = prompt.into()
        }
        (None, None) if placeholders == 0 => (),
        _ => bail!("native launch plan violates prompt contract"),
    }
    Ok(LaunchCommand {
        program: plan.program,
        native_eval_otlp: plan.native_eval_otlp,
        args: plan.args,
        prompt_arg: plan.prompt_arg,
        planner_binding_digest: Some(digest),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn validates_native_identity_prompt_boundary_and_bounds() {
        let value = json!({"native_eval_otlp":true,"program":"sixth", "args":["--",REDACTED_PROMPT], "prompt_arg":1});
        let plan =
            decode_plan(value.clone(), "sixth", Some("--literal"), "binding".into()).unwrap();
        assert_eq!(plan.args, ["--", "--literal"]);
        assert_eq!(plan.redacted().args[1], REDACTED_PROMPT);
        for (field, replacement) in [
            ("program", json!("/bin/sh")),
            ("prompt_arg", json!(0)),
            ("args", json!([REDACTED_PROMPT, REDACTED_PROMPT])),
            ("extra", json!(true)),
        ] {
            let mut invalid = value.clone();
            invalid[field] = replacement;
            assert!(decode_plan(invalid, "sixth", Some("prompt"), "binding".into()).is_err());
        }
        assert!(decode_plan(value, "sixth", None, "binding".into()).is_err());
        let direct = json!({"native_eval_otlp":true,"program":"sixth","args":[],"prompt_arg":null});
        assert!(decode_plan(direct, "sixth", None, "binding".into()).is_ok());
    }
}

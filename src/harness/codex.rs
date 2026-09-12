//! The Codex adapter.
//!
//! Verified against codex-cli 0.154.0 on 2026-09-12 using the installed CLI's
//! own `--help` output and `codex login status`.
//!
//! Native conventions this adapter defers to:
//!   - `AGENTS.md` in the working directory is Codex's repository guidance, and
//!     it is discovered by Codex itself from the task worktree;
//!   - the model is selected with `-m <exact identifier>`;
//!   - the prompt is Codex's positional argument.
//!
//! What it cannot enforce: **Codex has no per-agent selection.** There is no
//! `--agent` flag and no instructions-file option; `codex agents` browses
//! running sessions, not definitions, and `-p/--profile` layers model and
//! sandbox config rather than instructions. So a named ahu agent backed by
//! Codex pins the harness and the exact model, but its system prompt is not
//! enforceable. ahu says so rather than quietly pasting the instructions into
//! the prompt, which would be exactly the silent translation it forbids.

use super::{Adapter, EnforcementReport, LaunchCommand, LaunchRequest};
use crate::bail;
use crate::util::Result;

pub struct Codex;

impl Adapter for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn launch_command(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand> {
        if request.model.is_empty() {
            bail!("the Codex adapter requires an exact model identifier.");
        }
        if request.model.starts_with('-') {
            bail!(
                "model identifier {:?} would be read as an option by the Codex CLI.",
                request.model
            );
        }
        // `--` closes the option list so a prompt starting with `-` is still a
        // prompt, and the prompt itself stays a single argv element.
        let args = vec![
            "-m".to_string(),
            request.model.to_string(),
            "--".to_string(),
            request.prompt.to_string(),
        ];
        let prompt_arg = Some(args.len() - 1);
        Ok(LaunchCommand {
            program: "codex".to_string(),
            args,
            prompt_arg,
        })
    }

    fn enforcement(&self, _model: &str) -> EnforcementReport {
        let entry = crate::catalog::harness("codex").expect("codex is in the catalog");
        EnforcementReport {
            harness: "codex".to_string(),
            harness_version: installed_version("codex"),
            model_fixed_for_session: entry.enforces_model_for_session,
            gaps: entry.enforcement_gaps.iter().map(|s| s.to_string()).collect(),
            applied_controls: vec![
                "-m pins the exact model for the session's first request".to_string(),
                "ahu passes no --sandbox, --ask-for-approval, --approve-for-me, --dangerously-bypass-approvals-and-sandbox, or --dangerously-bypass-hook-trust; whether the effective session keeps Codex's own defaults also depends on any wrapper on PATH".to_string(),
                "repository guidance is discovered by Codex from the task worktree, which carries the parent's AGENTS.md unchanged".to_string(),
            ],
        }
    }
}

/// Ask a harness binary for its version, for the enforcement report.
pub(crate) fn installed_version(program: &str) -> Option<String> {
    let output = std::process::Command::new(program)
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

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
//! sandbox config rather than instructions. A named ahu agent backed by Codex
//! pins the harness and the exact model; its instructions are not enforceable
//! here, and they are not enforceable on any other harness either.
//!
//! Instructions and the delegation contract travel in the prompt, attributed
//! to the source file and delivered-text digest. This is reported as a gap in
//! `crate::launch::DELIVERY_IS_NOT_ENFORCEMENT`, not as identity enforcement.

use super::{Adapter, EnforcementReport, LaunchCommand, LaunchRequest};
use crate::agent::Permissions;
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
        let mut args = vec!["-m".to_string(), request.model.to_string()];
        // Verified against `codex --help`: --approve-for-me routes approvals
        // through automatic review in the workspace-write sandbox, and
        // `--ask-for-approval never` stops Codex asking at all.
        match request.permissions {
            Permissions::Prompt => {}
            Permissions::AcceptEdits => {
                args.push("--approve-for-me".to_string());
            }
            Permissions::Auto => {
                args.push("--ask-for-approval".to_string());
                args.push("never".to_string());
                args.push("--sandbox".to_string());
                args.push("workspace-write".to_string());
            }
        }
        args.push("--".to_string());
        let prompt_arg = Some(args.len());
        args.push(request.prompt.to_string());
        Ok(LaunchCommand {
            program: "codex".to_string(),
            args,
            prompt_arg,
        })
    }

    fn enforcement(&self, _model: &str, permissions: Permissions) -> Result<EnforcementReport> {
        let entry = crate::catalog::harness("codex").ok_or_else(|| {
            crate::util::Error::new(
                "the compatibility catalog has no entry for harness \"codex\", so ahu cannot \
                 state what this adapter enforces. This is an ahu build problem, not a \
                 repository one.",
            )
        })?;
        Ok(EnforcementReport {
            harness: "codex".to_string(),
            harness_version: installed_version("codex"),
            model_fixed_for_session: entry.enforces_model_for_session,
            gaps: entry.enforcement_gaps.iter().map(|s| s.to_string()).collect(),
            applied_controls: vec![
                "-m pins the exact model for the session's first request".to_string(),
                permission_control(permissions),
                "repository guidance is discovered by Codex from the task worktree, which carries the parent's AGENTS.md unchanged".to_string(),
            ],
        })
    }
}

/// State the approval flags this adapter passes, from the same value that
/// decides them, so the disclosure matches the requested argv.
fn permission_control(permissions: Permissions) -> String {
    let tail = "ahu does not know the effective approval boundary, only which flags it \
                passed. Codex's own settings decide it, including any this repository \
                carries into the task worktree, and a wrapper on PATH can change what Codex's \
                own defaults end up being";
    match permissions {
        Permissions::Prompt => format!(
            "ahu passes no --sandbox, --ask-for-approval, --approve-for-me, \
             --dangerously-bypass-approvals-and-sandbox, or --dangerously-bypass-hook-trust; \
             {tail}"
        ),
        Permissions::AcceptEdits => format!(
            "ahu passes --approve-for-me because this agent's manifest declares \
             permissions = accept-edits; it passes no --sandbox, --ask-for-approval, \
             --dangerously-bypass-approvals-and-sandbox, or --dangerously-bypass-hook-trust; \
             {tail}"
        ),
        Permissions::Auto => format!(
            "ahu passes --ask-for-approval never and --sandbox workspace-write because this \
             agent's manifest declares permissions = auto; it passes no --approve-for-me, \
             --dangerously-bypass-approvals-and-sandbox, or --dangerously-bypass-hook-trust; \
             {tail}"
        ),
    }
}

/// Ask a harness binary for its version, for the enforcement report.
///
/// Resolution and the repository exclusion live in `selection`, so a diagnostic
/// probe cannot run a binary a launch would refuse.
pub(crate) fn installed_version(program: &str) -> Option<String> {
    crate::selection::installed_version(program)
}

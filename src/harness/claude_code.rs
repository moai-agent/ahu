//! The Claude Code adapter.
//!
//! Verified against Claude Code 2.1.269 on 2026-09-11 using the installed CLI's
//! own `--help` output.
//!
//! Native conventions this adapter defers to:
//!   - `CLAUDE.md`, skills under `.claude/skills/`, and settings under
//!     `.claude/` are discovered by Claude Code itself from the working
//!     directory, which is the task worktree;
//!   - the model is selected with `--model <exact identifier>`.
//!
//! What it cannot enforce: an interactive Claude Code session can change model
//! with `/model` after launch, and ahu has no supported control that disables
//! that. The launch therefore carries the reliability warning.
//!
//! This adapter deliberately passes **no** `--agent` and **no**
//! `--append-system-prompt`. `--agent <name>` selects whatever Claude Code's own
//! agent search resolves that name to, which is not necessarily the file ahu
//! read and digested, so ahu was asserting a binding it could not check. The
//! agent's instructions and ahu's delegation contract now travel in the prompt
//! on every harness instead — see `crate::orchestration`. That is not an
//! enforced system prompt, and the enforcement report says so as a gap.

use super::{Adapter, EnforcementReport, LaunchCommand, LaunchRequest};
use crate::agent::Permissions;
use crate::bail;
use crate::util::Result;

pub struct ClaudeCode;

impl Adapter for ClaudeCode {
    fn id(&self) -> &'static str {
        "claude-code"
    }

    fn launch_command(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand> {
        if request.model.is_empty() {
            bail!("the Claude Code adapter requires an exact model identifier.");
        }
        if request.model.starts_with('-') {
            bail!(
                "model identifier {:?} would be read as an option by the Claude Code CLI.",
                request.model
            );
        }
        let mut args = vec!["--model".to_string(), request.model.to_string()];
        // Verified against `claude --help`: --permission-mode takes
        // acceptEdits | auto | bypassPermissions | manual | dontAsk | plan.
        match request.permissions {
            Permissions::Prompt => {}
            Permissions::AcceptEdits => {
                args.push("--permission-mode".to_string());
                args.push("acceptEdits".to_string());
            }
            Permissions::Auto => {
                args.push("--permission-mode".to_string());
                args.push("auto".to_string());
            }
        }
        // `--` closes the option list so a prompt starting with `-` is still a
        // prompt, and the prompt itself stays a single argv element.
        args.push("--".to_string());
        let prompt_arg = Some(args.len());
        args.push(request.prompt.to_string());
        Ok(LaunchCommand {
            program: "claude".to_string(),
            args,
            prompt_arg,
        })
    }

    fn enforcement(&self, _model: &str, permissions: Permissions) -> Result<EnforcementReport> {
        let entry = crate::catalog::harness("claude-code").ok_or_else(|| {
            crate::util::Error::new(
                "the compatibility catalog has no entry for harness \"claude-code\", so ahu cannot \
                 state what this adapter enforces. This is an ahu build problem, not a \
                 repository one.",
            )
        })?;
        Ok(EnforcementReport {
            harness: "claude-code".to_string(),
            harness_version: installed_version(),
            model_fixed_for_session: entry.enforces_model_for_session,
            gaps: entry
                .enforcement_gaps
                .iter()
                .map(|s| s.to_string())
                .collect(),
            applied_controls: vec![
                "--model pins the exact model for the session's first request".to_string(),
                permission_control(permissions),
            ],
        })
    }
}

/// State the permission flags this adapter passes, from the same value that
/// decides them.
///
/// Derived rather than fixed so it cannot drift out of step with
/// `launch_command`. The fixed version of this string asserted "ahu passes no
/// --permission-mode" on launches that passed exactly that flag, which put the
/// Enforcement block in direct contradiction with the Approvals block and the
/// argv dump printed beside it.
fn permission_control(permissions: Permissions) -> String {
    let tail = "ahu does not know the effective approval boundary, only which flags it \
                passed. The harness's own settings decide it, including any this repository \
                carries into the task worktree, and a wrapper on PATH can change what the harness's \
                own approval boundaries end up being";
    match permissions {
        Permissions::Prompt => format!(
            "ahu passes no --dangerously-skip-permissions, --permission-mode, --allowedTools, \
             or --add-dir; {tail}"
        ),
        Permissions::AcceptEdits => format!(
            "ahu passes --permission-mode acceptEdits because this agent's manifest declares \
             permissions = accept-edits; it passes no --dangerously-skip-permissions, \
             --allowedTools, or --add-dir; {tail}"
        ),
        Permissions::Auto => format!(
            "ahu passes --permission-mode auto because this agent's manifest declares \
             permissions = auto; it passes no --dangerously-skip-permissions, --allowedTools, \
             or --add-dir; {tail}"
        ),
    }
}

/// Ask the installed Claude Code for its version.
///
/// Goes through `selection`, which resolves the program to an absolute path and
/// refuses one inside a repository ahu has opened. Running
/// `Command::new("claude")` here repeated the operating system's PATH lookup --
/// including relative entries -- and executed a planted binary before the user
/// was shown a preview to approve.
fn installed_version() -> Option<String> {
    crate::selection::installed_version("claude")
}

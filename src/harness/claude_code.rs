//! The Claude Code adapter.
//!
//! Verified against Claude Code 2.1.269 on 2026-09-11 using the installed CLI's
//! own `--help` output.
//!
//! Native conventions this adapter defers to:
//!   - agent definitions live at `.claude/agents/<name>.md` and are selected
//!     with `--agent <name>`; ahu does not inline their body into a system
//!     prompt, so the frontmatter's tool and permission settings keep applying;
//!   - `CLAUDE.md`, skills under `.claude/skills/`, and settings under
//!     `.claude/` are discovered by Claude Code itself from the working
//!     directory, which is the task worktree;
//!   - the model is selected with `--model <exact identifier>`.
//!
//! What it cannot enforce: an interactive Claude Code session can change model
//! with `/model` after launch, and ahu has no supported control that disables
//! that. The launch therefore carries the reliability warning.

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
        if let Some(agent) = request.native_agent {
            if agent.starts_with('-') {
                bail!("agent name {agent:?} would be read as an option by the Claude Code CLI.");
            }
            args.push("--agent".to_string());
            args.push(agent.to_string());
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

    fn enforcement(&self, _model: &str) -> Result<EnforcementReport> {
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
            gaps: entry.enforcement_gaps.iter().map(|s| s.to_string()).collect(),
            applied_controls: vec![
                "--model pins the exact model for the session's first request".to_string(),
                "--agent selects the native agent definition, preserving its own tool and permission settings".to_string(),
                "ahu passes no --dangerously-skip-permissions, --permission-mode, --allowedTools, or --add-dir; whether the effective session keeps the harness's own approval boundaries also depends on any wrapper on PATH".to_string(),
            ],
        })
    }
}

fn installed_version() -> Option<String> {
    let output = std::process::Command::new("claude")
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

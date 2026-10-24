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
//! that. The enforcement report records this session-level limitation.
//!
//! This adapter deliberately passes **no** `--agent` and **no**
//! `--append-system-prompt`. `--agent <name>` selects whatever Claude Code's own
//! agent search resolves that name to, which is not necessarily the file ahu
//! read and digested. The agent's instructions and ahu's delegation contract
//! travel in the prompt
//! on every harness instead — see `crate::orchestration`. That is not an
//! enforced system prompt, and the enforcement report says so as a gap.

use super::{Adapter, EnforcementReport, LaunchCommand, LaunchRequest};
use crate::agent::Permissions;
use crate::util::Result;

pub struct ClaudeCode;

impl Adapter for ClaudeCode {
    fn id(&self) -> &'static str {
        "claude-code"
    }

    fn launch_command(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand> {
        let mut args = super::model_args(self.id(), request.model)?;
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
                // Invocation-scoped approval of Ahu's project MCP server only.
                // Do not approve every current/future server or persist this
                // decision into the user's native settings.
                args.push("--settings".to_string());
                args.push(r#"{"enabledMcpjsonServers":["ahu"]}"#.to_string());
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
             or --add-dir; --settings approves only the ahu project MCP server for this invocation; interactive auto tasks also authorize native folder trust for only the exact verified Ahu worktree, with Ahu-added trust removed by ahu remove; {tail}"
        ),
    }
}

/// Ask the installed Claude Code for its version.
///
/// Goes through `selection` so probes reject relative PATH entries and binaries
/// inside a repository ahu has opened, just as launches do.
fn installed_version() -> Option<String> {
    crate::selection::installed_version("claude")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_approves_only_ahu_mcp_for_this_invocation() {
        for permissions in [
            Permissions::Prompt,
            Permissions::AcceptEdits,
            Permissions::Auto,
        ] {
            let command = ClaudeCode
                .launch_command(&LaunchRequest {
                    model: "claude-opus-5",
                    prompt: "--settings malicious",
                    cwd: std::path::Path::new("/tmp/task"),
                    permissions,
                })
                .unwrap();
            let settings = command.args.windows(2).find(|pair| pair[0] == "--settings");
            if permissions == Permissions::Auto {
                let value: serde_json::Value = serde_json::from_str(&settings.unwrap()[1]).unwrap();
                assert_eq!(value, serde_json::json!({"enabledMcpjsonServers":["ahu"]}));
            } else {
                assert!(settings.is_none());
            }
            let index = command.prompt_arg.unwrap();
            assert_eq!(command.args[index - 1], "--");
            assert_eq!(command.args[index], "--settings malicious");
        }
    }
}

//! The Antigravity CLI adapter.
//!
//! Verified against Antigravity CLI 1.2.2 (`agy`) on 2026-09-12 using the
//! installed CLI's own help, `agy models`, and live probes.
//!
//! Native conventions this adapter defers to:
//!   - the model is selected with `--model <exact identifier>`, and the
//!     identifiers come from `agy models` rather than from ahu's guesswork;
//!   - `-i/--prompt-interactive` runs the submitted prompt and then keeps the
//!     session interactive, which is the shape ahu needs.
//!
//! Authentication is the harness's own OAuth sign-in. ahu holds no key and sets
//! no provider: a `modelProvider` in the CLI's settings that demands an API key
//! is a local configuration problem ahu reports rather than works around.
//!
//! What it cannot enforce: **`--agent` is not validated by the CLI.** A probe
//! with a nonexistent agent name returned a normal assistant reply instead of
//! an error, so ahu could not tell from the harness whether the named
//! definition was actually loaded. This adapter therefore no longer passes
//! `--agent` at all: the agent's instructions travel in the prompt, identically
//! to every other harness — see `crate::orchestration`.

use super::{Adapter, EnforcementReport, LaunchCommand, LaunchRequest};
use crate::agent::Permissions;
use crate::bail;
use crate::util::Result;

pub struct Antigravity;

impl Adapter for Antigravity {
    fn id(&self) -> &'static str {
        "antigravity"
    }

    fn launch_command(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand> {
        if request.model.is_empty() {
            bail!("the Antigravity adapter requires an exact model identifier.");
        }
        if request.model.starts_with('-') {
            bail!(
                "model identifier {:?} would be read as an option by the Antigravity CLI.",
                request.model
            );
        }
        let mut args = vec!["--model".to_string(), request.model.to_string()];
        // Verified against `agy --help`: --mode takes accept-edits | plan, and
        // --dangerously-skip-permissions auto-approves every tool request.
        match request.permissions {
            Permissions::Prompt => {}
            Permissions::AcceptEdits => {
                args.push("--mode".to_string());
                args.push("accept-edits".to_string());
            }
            Permissions::Auto => {
                args.push("--dangerously-skip-permissions".to_string());
            }
        }
        // `--prompt-interactive` runs the prompt and keeps the session alive.
        // The prompt is its value, so it stays one argv element.
        args.push("--prompt-interactive".to_string());
        let prompt_arg = Some(args.len());
        args.push(request.prompt.to_string());
        Ok(LaunchCommand {
            program: "agy".to_string(),
            args,
            prompt_arg,
        })
    }

    fn enforcement(&self, _model: &str, permissions: Permissions) -> Result<EnforcementReport> {
        let entry = crate::catalog::harness("antigravity").ok_or_else(|| {
            crate::util::Error::new(
                "the compatibility catalog has no entry for harness \"antigravity\", so ahu cannot \
                 state what this adapter enforces. This is an ahu build problem, not a \
                 repository one.",
            )
        })?;
        Ok(EnforcementReport {
            harness: "antigravity".to_string(),
            harness_version: super::codex::installed_version("agy"),
            model_fixed_for_session: entry.enforces_model_for_session,
            gaps: entry
                .enforcement_gaps
                .iter()
                .map(|s| s.to_string())
                .collect(),
            applied_controls: vec![
                "--model pins the exact model for the session's first request".to_string(),
                permission_control(permissions),
                "authentication is the harness's own OAuth sign-in; ahu holds no API key"
                    .to_string(),
            ],
        })
    }
}

/// State the permission flags this adapter passes, from the same value that
/// decides them. See the note in the Claude Code adapter: a fixed string here
/// denied passing `--dangerously-skip-permissions` on the very launches that
/// pass it.
fn permission_control(permissions: Permissions) -> String {
    let tail = "ahu does not know the effective approval boundary, only which flags it \
                passed. The harness's own settings decide it, including any this repository \
                carries into the task worktree, and a wrapper on PATH can change what the harness's \
                own approval boundaries end up being";
    match permissions {
        Permissions::Prompt => {
            format!("ahu passes no --dangerously-skip-permissions, --mode, or --sandbox; {tail}")
        }
        Permissions::AcceptEdits => format!(
            "ahu passes --mode accept-edits because this agent's manifest declares \
             permissions = accept-edits; it passes no --dangerously-skip-permissions or \
             --sandbox; {tail}"
        ),
        Permissions::Auto => format!(
            "ahu passes --dangerously-skip-permissions because this agent's manifest declares \
             permissions = auto, which auto-approves every tool request; it passes no --mode or \
             --sandbox; {tail}"
        ),
    }
}

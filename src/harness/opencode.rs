//! The OpenCode adapter.
//!
//! Verified against OpenCode 1.18.29 and 1.18.30 on 2026-09-13 using the
//! installed CLI's own `--help`, `opencode agent list`, `opencode models`, and
//! live probes. The install replaced itself in place between those two
//! versions during verification, which is why the catalog entry names both.
//!
//! Native conventions this adapter defers to:
//!   - the model is selected with `--model <provider>/<model>`, and OpenCode
//!     documents that exact shape. The provider half is resolved by OpenCode's
//!     own configuration — a project `opencode.json`, the user's global config,
//!     or a managed one — which ahu carries into the task worktree but never
//!     writes;
//!   - `AGENTS.md` is OpenCode's rules file, with the Claude files as its
//!     fallback, and skills are discovered on demand, including under `.agents`
//!     and `.claude`. All of it is discovered by OpenCode itself from the task
//!     worktree;
//!   - the default `opencode` command with `--prompt <text>` submits the prompt
//!     and leaves the session interactive, which is the shape ahu needs.
//!
//! More than one OpenCode install can sit on `PATH` — a `~/.opencode/bin`
//! install and a Homebrew one were both present on the machine this adapter was
//! verified on, at different versions. ahu reports the version of the binary it
//! resolved, through the same `crate::selection` path a launch uses, so an
//! enforcement report cannot describe a different binary than the one that runs.
//!
//! What it cannot enforce:
//!
//! **An agent identity.** `--agent <name>` is accepted but not validated:
//! `opencode run --agent definitely-not-an-agent` printed
//! `! agent "definitely-not-an-agent" not found. Falling back to default agent`,
//! ran anyway under `build`, and exited 0. A flag that silently falls back can
//! never confirm an identity was applied, and it would additionally override the
//! named agent's own model and permissions — contradicting the model ahu pins.
//! This adapter does not pass it. The agent's instructions travel in the prompt
//! instead, identically to every other harness — see `crate::orchestration`.
//!
//! **An approval boundary.** OpenCode's permission actions are `allow`, `ask`
//! and `deny`, set statically in configuration or agent frontmatter, and the
//! built-in agents resolve to `*: allow` with only a few narrower entries. The
//! one permission-affecting CLI flag is `--auto`, which widens. There is no
//! accept-edits flag, so `permissions = "accept-edits"` is refused rather than
//! approximated: `--auto` would be a widening and silence would be a
//! misdescription.
//!
//! An `opencode.json` may name `plugin` modules that OpenCode installs and runs
//! at startup. That is executable configuration under the same snapshot and
//! trust boundary as any other inherited config; ahu neither adds a plugin nor
//! passes `--pure` to disable the user's.

use super::{Adapter, EnforcementReport, LaunchCommand, LaunchRequest};
use crate::agent::Permissions;
use crate::bail;
use crate::util::Result;

pub struct OpenCode;

impl Adapter for OpenCode {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn launch_command(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand> {
        if request.model.is_empty() {
            bail!("the OpenCode adapter requires an exact model identifier.");
        }
        if request.model.starts_with('-') {
            bail!(
                "model identifier {:?} would be read as an option by the OpenCode CLI.",
                request.model
            );
        }
        // OpenCode documents `--model` as `provider/model`. An unqualified
        // identifier is a silent-misroute risk: which provider it would reach
        // depends on the user's configuration, and ahu will not guess one.
        let qualified = request
            .model
            .split_once('/')
            .filter(|(provider, model)| {
                !provider.is_empty() && !model.is_empty() && !model.contains('/')
            })
            .is_some();
        if !qualified {
            bail!(
                "model identifier {:?} is not provider-qualified. OpenCode's --model takes \
                 <provider>/<model>, where the provider half names one of the providers \
                 OpenCode's own configuration defines. ahu will not guess which provider an \
                 unqualified identifier means; name it in the manifest.",
                request.model
            );
        }
        let mut args = vec!["--model".to_string(), request.model.to_string()];
        // Verified against `opencode --help`: --auto auto-approves every
        // permission that is not explicitly denied. There is no accept-edits
        // equivalent; permission actions are static configuration.
        match request.permissions {
            Permissions::Prompt => {}
            Permissions::AcceptEdits => bail!(
                "OpenCode has no accept-edits mode. Its permission actions are allow, ask and \
                 deny, set in OpenCode's own configuration or agent frontmatter, and its only \
                 permission flag is --auto, which auto-approves everything not explicitly \
                 denied. ahu will not widen the request to --auto, and will not pass nothing \
                 while reporting accept-edits. Declare permissions = \"prompt\" or \
                 permissions = \"auto\" in the manifest, or express the boundary you want as \
                 an OpenCode `permission` configuration you own."
            ),
            Permissions::Auto => args.push("--auto".to_string()),
        }
        // No `--`: the default OpenCode command has a positional `project`
        // path, so `--` would make the next token a directory argument —
        // `opencode /definitely/not/a/dir-xyz` fails with "Failed to change
        // directory". A `--prompt` value beginning with `-` is not taken as the
        // value at all (`--prompt --version` printed the version), so a prompt
        // like that is refused rather than silently parsed as flags. ahu's
        // composed prompt opens with the delegation-contract fence, so this is
        // a defensive guard rather than a path a normal launch reaches.
        if request.prompt.starts_with('-') {
            bail!(
                "the composed prompt begins with '-', which the OpenCode CLI would read as \
                 options rather than as the --prompt value, and OpenCode's own positional \
                 argument is a project directory, so `--` cannot be used to end the option \
                 list. ahu will not launch a session whose prompt it cannot deliver intact."
            );
        }
        args.push("--prompt".to_string());
        let prompt_arg = Some(args.len());
        args.push(request.prompt.to_string());
        Ok(LaunchCommand {
            program: "opencode".to_string(),
            args,
            prompt_arg,
        })
    }

    fn enforcement(&self, _model: &str, permissions: Permissions) -> Result<EnforcementReport> {
        let entry = crate::catalog::harness("opencode").ok_or_else(|| {
            crate::util::Error::new(
                "the compatibility catalog has no entry for harness \"opencode\", so ahu cannot \
                 state what this adapter enforces. This is an ahu build problem, not a \
                 repository one.",
            )
        })?;
        Ok(EnforcementReport {
            harness: "opencode".to_string(),
            harness_version: crate::selection::installed_version("opencode"),
            model_fixed_for_session: entry.enforces_model_for_session,
            gaps: entry
                .enforcement_gaps
                .iter()
                .map(|s| s.to_string())
                .collect(),
            applied_controls: vec![
                "--model pins the exact provider-qualified model for the session's first request"
                    .to_string(),
                permission_control(permissions),
                "OpenCode discovers its own configuration, AGENTS.md rules and skills from the \
                 task worktree, which carries the parent checkout's copies unchanged"
                    .to_string(),
            ],
        })
    }
}

/// State the permission flags this adapter passes, from the same value that
/// decides them. See the note in the Claude Code adapter: a fixed string there
/// denied passing a flag on the very launches that passed it.
fn permission_control(permissions: Permissions) -> String {
    let tail = "ahu does not know the effective approval boundary, only which flags it \
                passed. OpenCode's own settings decide it — and they default most tool \
                permissions to allow — including any this repository carries into the task \
                worktree, and a wrapper on PATH can change what the harness's own approval \
                boundaries end up being";
    match permissions {
        Permissions::Prompt => {
            format!("ahu passes no --auto and no --pure; {tail}")
        }
        // Unreachable through `launch_command`, which refuses accept-edits
        // rather than approximating it. Stated rather than panicked so a report
        // built from a stale record still describes the refusal.
        Permissions::AcceptEdits => format!(
            "OpenCode has no accept-edits mode, so ahu refuses to build this launch at all \
             rather than widen it to --auto or pass nothing; {tail}"
        ),
        Permissions::Auto => format!(
            "ahu passes --auto because this agent's manifest declares permissions = auto, \
             which auto-approves every permission OpenCode does not explicitly deny; it \
             passes no --pure; {tail}"
        ),
    }
}

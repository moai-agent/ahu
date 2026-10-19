//! The OpenCode adapter.
//!
//! OpenCode 1.x behavior was live-probed on 1.18.29 and 1.18.30. On 2026-10-19,
//! 2.0.32 help and interactive startup were checked against the installed CLI.
//! Headless behavior remains verified only for the 1.x versions listed in the
//! compatibility catalog.
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
//! OpenCode 2.0.32 removed --model from the default interactive command.
//! Its `run` command still accepts it, but exits after the message. The `mini`
//! subcommand provides the persistent interactive interface with --model and
//! --prompt, but rejects --auto even when it appears before the subcommand.
//! Ahu therefore supports prompt-permission interactive sessions on v2 and
//! refuses auto-permission sessions before creating a task. Interactive
//! admission selects `mini` only when the default command lacks a requested
//! flag, then verifies the resolved binary's help before submission and again
//! before execution.
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
//! and `deny`, set statically in configuration or agent frontmatter. The
//! interactive permission flag `--auto` is accepted by supported 1.x CLIs but
//! not by v2's persistent `mini` command. There is no
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

/// OpenCode 2.x has the isolated `run --standalone` mode; 1.x does not.
pub(crate) fn supports_standalone_run(version: &str) -> bool {
    crate::catalog::version_token(version)
        .and_then(|version| version.split('.').next())
        .and_then(|major| major.parse::<u64>().ok())
        .is_some_and(|major| major == 2)
}

pub struct OpenCode;

/// Check only option declarations, not mentions in prose or subcommand help.
/// OpenCode 1.x uses `-m, --model`; 2.x uses `--model, -m string`.
fn declares_flag(help: &str, flag: &str) -> bool {
    help.lines().any(|line| {
        line.split_whitespace()
            .take_while(|word| word.starts_with('-'))
            .any(|word| word.trim_end_matches(',') == flag)
    })
}

pub(crate) fn check_interactive_help(help: &str, args: &[String]) -> Result<()> {
    for flag in ["--model", "--prompt", "--auto"] {
        if !args.iter().any(|arg| arg == flag) {
            continue;
        }
        if !declares_flag(help, flag) {
            bail!(
                "OpenCode interactive CLI is incompatible: the resolved executable's \
                 `opencode --help` does not advertise top-level {flag}, which this launch \
                 requires. ahu will not drop the exact model or approval controls, or \
                 replace a persistent interactive session with one-shot `opencode run`. \
                 Check which opencode is on PATH and use an interactive CLI supporting \
                 these flags. For a batch task, explicitly request --headless instead; \
                 its separate compatibility and isolation checks still apply."
            );
        }
    }
    Ok(())
}

pub(crate) fn check_mini_interactive_help(
    _top_level_help: &str,
    mini_help: &str,
    args: &[String],
) -> Result<()> {
    for flag in ["--model", "--prompt"] {
        if args.iter().any(|arg| arg == flag) && !declares_flag(mini_help, flag) {
            bail!(
                "OpenCode mini interactive CLI is incompatible: `opencode mini --help` does not \
                 advertise {flag}, which this launch requires. ahu will not drop the exact \
                 model or prompt."
            );
        }
    }
    if args.iter().any(|arg| arg == "--auto") {
        bail!(
            "OpenCode 2's persistent `mini` interface does not accept `--auto` (OpenCode rejects \
             it even when placed before `mini`). ahu will not silently weaken the requested \
             automatic-approval setting. Use `permissions = \"prompt\"` for an interactive \
             OpenCode 2 session, or use a compatible OpenCode 1.x CLI; headless OpenCode uses its \
             separately checked `run` interface."
        );
    }
    Ok(())
}

impl Adapter for OpenCode {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn launch_command(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand> {
        let mut args = super::model_args(self.id(), request.model)?;
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
            "ahu passes --auto for supported OpenCode 1.x interactive sessions; OpenCode 2's \
             persistent mini interface rejects --auto, so ahu refuses this interactive launch. \
             Headless OpenCode is checked separately; {tail}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_task_directory_overrides_stale_pwd_for_mini_and_run() {
        // Static inspection of 2.0.32: both mini's iQd and run's gQd
        // prefer process.env.PWD to process.cwd(), then chdir to that path.
        // A Python fixture preserves the inherited PWD (a shell may repair it).
        let root = tempfile::tempdir().unwrap();
        let worktree = root.path().join("task directory");
        std::fs::create_dir(&worktree).unwrap();
        for interface in ["mini", "run"] {
            for inherited in [root.path().to_path_buf(), root.path().join("missing")] {
                let mut command = std::process::Command::new("python3");
                command
                    .args([
                        "-c",
                        "import os,sys; os.chdir(os.environ.get('PWD', os.getcwd())); assert os.path.samefile(os.getcwd(), sys.argv[2]), sys.argv[1]",
                        interface,
                    ])
                    .arg(&worktree)
                    .env("PWD", inherited);
                crate::harness::configure_task_directory(&mut command, "opencode", &worktree);
                let output = command.output().unwrap();
                assert!(
                    output.status.success(),
                    "{interface}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }

    #[test]
    fn interactive_help_requires_each_requested_flag_as_a_declaration() {
        let args: Vec<_> = ["--model", "provider/model", "--auto", "--prompt", "task"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let help = include_str!("../../tests/fixtures/opencode/1.x-help.txt");
        check_interactive_help(help, &args).unwrap();
        for flag in ["--model", "--auto", "--prompt"] {
            let missing = help
                .lines()
                .filter(|line| !line.contains(flag))
                .collect::<Vec<_>>()
                .join("\n");
            let misleading = format!(
                "{missing}\nUse {flag} with run instead\n  {flag}-other  unrelated option\n"
            );
            let error = check_interactive_help(&misleading, &args).unwrap_err();
            assert!(error.to_string().contains(&format!("top-level {flag}")));
        }
        // Prompt approvals do not require --auto; a coordinator has no prompt.
        check_interactive_help("  --model, -m string  Model", &args[..2]).unwrap();
        assert!(check_interactive_help("", &args).is_err());
    }

    #[test]
    fn mini_interactive_help_pins_model_and_prompt_but_rejects_auto() {
        let top = include_str!("../../tests/fixtures/opencode/2.0.32-help.txt");
        let mini = include_str!("../../tests/fixtures/opencode/2.0.32-mini-help.txt");
        assert!(declares_flag(top, "--auto"));
        assert!(!declares_flag(mini, "--auto"));
        let args: Vec<_> = [
            "--model",
            "ollama/glm-5.3:cloud",
            "--auto",
            "--prompt",
            "task",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let error = check_mini_interactive_help(top, mini, &args).unwrap_err();
        assert!(error.to_string().contains("does not accept `--auto`"));
        assert!(check_mini_interactive_help(top, "Usage: opencode mini", &args).is_err());
        assert!(check_mini_interactive_help("", mini, &args).is_err());
    }
}

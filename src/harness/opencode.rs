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
            planner_binding_digest: None,
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

/// The recorded-session projection and its sanitized export were verified
/// against OpenCode 2.0.32. Do not extend this to another version until its
/// export and sanitization semantics have been checked too.
pub(crate) fn supports_session_export_evidence(version: &str) -> bool {
    crate::catalog::version_token(version).is_some_and(|version| version == "2.0.32")
}

/// Bound retries while OpenCode makes the completed headless turn available
/// through its sanitized recorded-session projection.
pub(crate) const EXPORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Bound the inspected projection. A turn that recorded more messages than
/// this is not a shape this check claims to understand.
const EXPORT_MESSAGE_LIMIT: usize = 4096;

/// OpenCode mints session identifiers as `ses_` followed by its own
/// alphanumeric id. Anything else is not an identifier this adapter will place
/// on a command line or match an export against.
pub(crate) fn valid_session_id(session: &str) -> bool {
    session.len() >= 8
        && session.len() <= 128
        && session.starts_with("ses_")
        && session[4..].chars().all(|c| c.is_ascii_alphanumeric())
}

/// Decide whether a sanitized `opencode session export` establishes that
/// `session` completed the one turn this attempt started.
///
/// Statically verified against the 2.0.32 binary: `session export` prints
/// `{"info": Session.Info, "messages": [Session.Message.Info]}`, the export
/// drops assistant messages that have no `time.completed`, the projection
/// appends an `idle` message carrying the execution outcome, and sanitization
/// rewrites only text-bearing fields — `type`, `finish`, `outcome`, `status`
/// and `time` survive it intact. Nothing here reads a transcript field.
///
/// Every unexpected shape is a failure. In particular this refuses any
/// recorded tool call that is not `completed`: a refusal and a recovered tool
/// error are the same `error` status in the projection, and ahu will not tell
/// them apart from redacted text. A run that recovered from a tool error is
/// therefore not rescued here; it is reported as missing completion evidence,
/// which is what it was before this check existed.
pub(crate) fn export_establishes_completion(
    export: &str,
    session: &str,
    attempt_started_ms: u64,
) -> std::result::Result<(), &'static str> {
    use serde_json::Value;

    if !valid_session_id(session) {
        return Err("the attempt's session identifier is not an OpenCode session id");
    }
    let value: Value = serde_json::from_str(export).map_err(|_| "the export is not JSON")?;
    let info = value
        .get("info")
        .and_then(Value::as_object)
        .ok_or("the export carries no session info")?;
    if info.get("id").and_then(Value::as_str) != Some(session) {
        return Err("the export describes a different session");
    }
    if info.get("outcome").and_then(Value::as_str) != Some("succeeded") {
        return Err("the recorded session did not end with a succeeded outcome");
    }
    let time = info
        .get("time")
        .and_then(Value::as_object)
        .ok_or("the recorded session carries no timestamps")?;
    if !time.get("idle").is_some_and(Value::is_number) {
        return Err("the recorded session is not idle");
    }
    // The projection timestamps are epoch milliseconds. A session created
    // before this attempt spawned is a different run's session, whatever its
    // identifier says.
    let created = time
        .get("created")
        .and_then(Value::as_u64)
        .ok_or("the recorded session has no usable creation time")?;
    if created < attempt_started_ms {
        return Err("the recorded session predates this attempt");
    }
    let messages = value
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("the export carries no messages")?;
    if messages.is_empty() || messages.len() > EXPORT_MESSAGE_LIMIT {
        return Err("the exported message count is empty or past the inspection bound");
    }

    let mut prompts = 0usize;
    let mut last_assistant = None;
    for (index, message) in messages.iter().enumerate() {
        let message = message
            .as_object()
            .ok_or("an exported message is not an object")?;
        match message
            .get("type")
            .and_then(Value::as_str)
            .ok_or("an exported message has no type")?
        {
            "user" => prompts += 1,
            "assistant" => {
                if message.get("error").is_some_and(|error| !error.is_null()) {
                    return Err("an assistant message recorded an error");
                }
                if !message
                    .get("time")
                    .and_then(|time| time.get("completed"))
                    .is_some_and(Value::is_number)
                {
                    return Err("an assistant message never completed");
                }
                for part in message
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or("an assistant message has no content")?
                {
                    if part.get("type").and_then(Value::as_str) == Some("tool")
                        && part.pointer("/state/status").and_then(Value::as_str)
                            != Some("completed")
                    {
                        return Err("a recorded tool call did not complete");
                    }
                }
                last_assistant = Some(index);
            }
            "idle" => {
                if index + 1 != messages.len() {
                    return Err("the recorded session went idle before its last message");
                }
                if message.get("outcome").and_then(Value::as_str) != Some("succeeded") {
                    return Err("the final idle message did not record success");
                }
                if message.get("error").is_some_and(|error| !error.is_null()) {
                    return Err("the final idle message recorded an error");
                }
            }
            // Turn bookkeeping that carries no completion claim. Everything
            // else — a shell message, a compaction, a type added by a later
            // OpenCode — leaves this check unable to describe the run.
            "system" | "synthetic" | "skill" | "agent-switched" | "model-switched"
            | "location-switched" | "compaction" => (),
            _ => return Err("the export holds a message type this check does not recognize"),
        }
    }
    // ahu passes no --session on the launch this fallback covers, so the
    // session holds exactly one prompt: its own. More than one would mean the
    // stop evidence could belong to an earlier turn.
    if prompts != 1 {
        return Err("the exported session does not hold exactly this attempt's one turn");
    }
    let last_assistant = last_assistant.ok_or("the exported session has no assistant message")?;
    // The execution outcome lives in the idle message the projection appends,
    // so a session that never recorded one never recorded an outcome.
    if messages[messages.len() - 1]
        .get("type")
        .and_then(Value::as_str)
        != Some("idle")
    {
        return Err("the exported session does not end with an idle message");
    }
    // That idle message is last, so the final assistant message must be the
    // one immediately before it: anything in between is work the stop reason
    // does not describe.
    if last_assistant + 2 != messages.len() {
        return Err("the exported session records work after its final assistant message");
    }
    if messages[last_assistant]
        .get("finish")
        .and_then(Value::as_str)
        != Some("stop")
    {
        return Err("the final assistant message did not finish with stop");
    }
    Ok(())
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

    const SESSION: &str = "ses_f5cc153b6ffeNq1dwHoZGMECKw";
    const STARTED: u64 = 1_760_000_000_000;

    /// A synthetic sanitized export in the 2.0.32 shape: one prompt, one
    /// assistant message that completed a tool call and stopped, and the
    /// idle message the projection appends on execution success. Transcript
    /// fields carry the redaction markers sanitization writes.
    fn export(messages: serde_json::Value) -> String {
        serde_json::json!({
            "info": {
                "id": SESSION,
                "projectID": "prj_synthetic",
                "outcome": "succeeded",
                "cost": 0,
                "tokens": {"input": 12, "output": 4, "reasoning": 0,
                           "cache": {"read": 0, "write": 0}},
                "time": {"created": STARTED + 10, "updated": STARTED + 900,
                         "idle": STARTED + 900},
                "title": format!("[redacted:session-title:{SESSION}]"),
                "location": {"directory": format!("/[redacted:session-directory:{SESSION}]")},
            },
            "messages": messages,
        })
        .to_string()
    }

    fn prompt() -> serde_json::Value {
        serde_json::json!({"id":"msg_prompt","type":"user","time":{"created":STARTED + 11},
                           "text":"[redacted:text:msg_prompt]"})
    }

    fn assistant(id: &str, finish: &str, tool_status: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id, "type": "assistant", "agent": "build",
            "model": {"providerID": "ollama", "modelID": "glm-5.3:cloud"},
            "content": [
                {"type":"tool","id":"prt_tool","tool":"write","callID":"call_synthetic",
                 "state":{"status":tool_status,"input":{"redacted":format!("tool-input:{id}")}}},
                {"type":"text","id":"prt_text","text":format!("[redacted:text:{id}]")},
            ],
            "finish": finish,
            "time": {"created": STARTED + 12, "completed": STARTED + 800},
        })
    }

    fn idle(outcome: &str) -> serde_json::Value {
        serde_json::json!({"id":"msg_idle","type":"idle","outcome":outcome,
                           "time":{"created":STARTED + 900}})
    }

    fn accepted() -> serde_json::Value {
        serde_json::json!([
            prompt(),
            assistant("msg_turn", "stop", "completed"),
            idle("succeeded")
        ])
    }

    #[test]
    fn a_sanitized_export_establishes_completion_only_for_this_attempts_finished_turn() {
        export_establishes_completion(&export(accepted()), SESSION, STARTED).unwrap();

        // Several steps: only the last one has to carry the stop.
        let stepped = serde_json::json!([
            prompt(),
            assistant("msg_step", "tool-calls", "completed"),
            {"id":"msg_compaction","type":"compaction","status":"completed",
             "time":{"created":STARTED + 850}},
            assistant("msg_turn", "stop", "completed"),
            idle("succeeded"),
        ]);
        export_establishes_completion(&export(stepped), SESSION, STARTED).unwrap();

        let rejected: [(&str, String, &str); 14] = [
            (
                "a different session",
                export(accepted()).replace(SESSION, "ses_someoneElsesSession0001"),
                "different session",
            ),
            (
                "an export of a session older than this attempt",
                export(accepted()),
                "predates this attempt",
            ),
            (
                "a stop that is not the session's last word",
                serde_json::to_string(&serde_json::json!({
                    "info": serde_json::from_str::<serde_json::Value>(&export(accepted()))
                        .unwrap()["info"],
                    "messages": [prompt(), assistant("msg_turn", "stop", "completed"),
                                 idle("succeeded"), assistant("msg_after", "stop", "completed")],
                }))
                .unwrap(),
                "idle before its last message",
            ),
            (
                "a later tool call after the final stop",
                export(serde_json::json!([
                    prompt(),
                    assistant("msg_turn", "stop", "completed"),
                    assistant("msg_after", "tool-calls", "completed"),
                    idle("succeeded"),
                ])),
                "did not finish with stop",
            ),
            (
                "a tool call that did not complete",
                export(serde_json::json!([
                    prompt(),
                    assistant("msg_turn", "stop", "running"),
                    idle("succeeded"),
                ])),
                "tool call did not complete",
            ),
            (
                "a refused tool call, which the projection records as an error",
                export(serde_json::json!([
                    prompt(),
                    assistant("msg_turn", "stop", "error"),
                    idle("succeeded"),
                ])),
                "tool call did not complete",
            ),
            (
                "an assistant error",
                export(serde_json::json!([
                    prompt(),
                    {
                        "id":"msg_turn","type":"assistant","agent":"build",
                        "model":{"providerID":"ollama","modelID":"glm-5.3:cloud"},
                        "content":[], "finish":"stop",
                        "error":{"name":"ProviderError"},
                        "time":{"created":STARTED + 12,"completed":STARTED + 800},
                    },
                    idle("succeeded"),
                ])),
                "recorded an error",
            ),
            (
                "an interrupted run",
                export(serde_json::json!([
                    prompt(),
                    assistant("msg_turn", "stop", "completed"),
                    idle("interrupted"),
                ])),
                "did not record success",
            ),
            (
                "a session carrying an earlier turn",
                export(serde_json::json!([
                    prompt(),
                    assistant("msg_earlier", "stop", "completed"),
                    prompt(),
                    assistant("msg_turn", "stop", "completed"),
                    idle("succeeded"),
                ])),
                "exactly this attempt's one turn",
            ),
            (
                "a session that never recorded an execution outcome",
                export(serde_json::json!([
                    prompt(),
                    assistant("msg_turn", "stop", "completed"),
                    {"id":"msg_system","type":"system","text":"[redacted:system:msg_system]",
                     "time":{"created":STARTED + 901}},
                ])),
                "does not end with an idle message",
            ),
            (
                "a session with no assistant message at all",
                export(serde_json::json!([prompt(), idle("succeeded")])),
                "no assistant message",
            ),
            (
                "a message type this check cannot describe",
                export(serde_json::json!([
                    prompt(),
                    {"id":"msg_shell","type":"shell","shellID":"shl_1","status":"completed",
                     "command":"[redacted:shell-command:msg_shell]",
                     "time":{"created":STARTED + 13}},
                    assistant("msg_turn", "stop", "completed"),
                    idle("succeeded"),
                ])),
                "does not recognize",
            ),
            ("malformed output", "{\"info\": ".to_string(), "not JSON"),
            (
                "output that is JSON but not an export",
                serde_json::json!({"data": {"info": {"id": SESSION}}}).to_string(),
                "no session info",
            ),
        ];
        for (case, body, expected) in rejected {
            // The stale-session case is the only one that reuses the accepted
            // body; it is rejected by the attempt window rather than by shape.
            let started = if expected == "predates this attempt" {
                STARTED + 1_000_000
            } else {
                STARTED
            };
            let error = export_establishes_completion(&body, SESSION, started).expect_err(case);
            assert!(error.contains(expected), "{case}: {error}");
        }
    }

    #[test]
    fn an_export_is_matched_only_against_a_well_formed_opencode_session_id() {
        assert!(valid_session_id(SESSION));
        for rejected in [
            "",
            "ses_",
            "ses_short",
            "msg_f5cc153b6ffeNq1dwHoZGMECKw",
            "ses_f5cc153b6ffe;rm -rf /",
            "ses_f5cc153b6ffe\nses_other",
        ] {
            if rejected == "ses_short" {
                // Long enough and well shaped: the bound is on length, not on
                // this adapter guessing OpenCode's id body.
                assert!(valid_session_id(rejected));
                continue;
            }
            assert!(!valid_session_id(rejected), "{rejected:?}");
        }
        assert!(!valid_session_id(&format!("ses_{}", "a".repeat(200))));
        let error = export_establishes_completion(&export(accepted()), "not-a-session", STARTED)
            .expect_err("a malformed identifier cannot be matched");
        assert!(error.contains("not an OpenCode session id"), "{error}");
    }

    #[test]
    fn recorded_session_evidence_is_gated_to_the_verified_version() {
        for supported in ["2.0.32", "opencode 2.0.32"] {
            assert!(supports_session_export_evidence(supported), "{supported}");
        }
        for unsupported in [
            "1.18.30", "1.18.34", "2.0.33", "2.1.0", "3.0.0", "", "unknown",
        ] {
            assert!(
                !supports_session_export_evidence(unsupported),
                "{unsupported}"
            );
        }
    }

    #[test]
    fn an_oversized_export_is_bounded_before_its_shape_is_trusted() {
        let mut messages = vec![prompt()];
        for index in 0..5000 {
            messages.push(assistant(
                &format!("msg_{index}"),
                "tool-calls",
                "completed",
            ));
        }
        messages.push(assistant("msg_turn", "stop", "completed"));
        messages.push(idle("succeeded"));
        let error = export_establishes_completion(
            &export(serde_json::Value::Array(messages)),
            SESSION,
            STARTED,
        )
        .expect_err("a projection past the inspection bound is not evidence");
        assert!(error.contains("inspection bound"), "{error}");
    }
}

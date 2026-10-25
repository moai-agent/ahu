//! Native argument mapping for the explicitly accepted CLI version.
use crate::protocol::{Mode, PROMPT, Permissions, Plan, Request};
pub(crate) const HARNESS: &str = "codex";
pub(crate) const PROGRAM: &str = "codex";
pub(crate) const NATIVE_VERSION: &str = "0.163.0";

pub(crate) fn plan(request: Request) -> Result<Plan, &'static str> {
    let session = request.validate()?;
    let mut args = Vec::new();
    let mut add = |values: &[&str]| args.extend(values.iter().map(|value| (*value).to_owned()));

    match request.mode {
        Mode::Interactive => {
            add(&["-m", &request.model, "--no-daemon"]);
            match request.permissions {
                Permissions::Prompt => (),
                Permissions::AcceptEdits => add(&["--approve-for-me"]),
                Permissions::Auto => add(&[
                    "--ask-for-approval",
                    "never",
                    "--sandbox",
                    "workspace-write",
                ]),
            }
            add(&["--"]);
        }
        Mode::Headless => {
            add(&["exec"]);
            if session.is_some() {
                add(&["resume"]);
            }
            add(&[
                "--model",
                &request.model,
                "--json",
                "-c",
                "approval_policy=\"never\"",
                "-c",
                "sandbox_mode=\"danger-full-access\"",
            ]);
            if session.is_none() {
                add(&["--color", "never"]);
            }
            add(&["--"]);
            if let Some(session) = session {
                add(&[session]);
            }
        }
    }

    let prompt_arg = args.len();
    args.push(PROMPT.to_owned());
    Ok(Plan {
        protocol_version: 2,
        harness: HARNESS,
        program: PROGRAM,
        args,
        prompt_arg,
    })
}

#[cfg(test)]
mod tests;

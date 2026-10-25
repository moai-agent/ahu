//! Native argument mapping for the explicitly accepted CLI version.
use crate::protocol::{Mode, PROMPT, Permissions, Plan, Request};
pub(crate) const HARNESS: &str = "claude-code";
pub(crate) const PROGRAM: &str = "claude";
pub(crate) const NATIVE_VERSION: &str = "2.1.310";

pub(crate) fn plan(request: Request) -> Result<Plan, &'static str> {
    let session = request.validate()?;
    let mut args = Vec::new();
    let mut add = |values: &[&str]| args.extend(values.iter().map(|value| (*value).to_owned()));

    match request.mode {
        Mode::Interactive => {
            add(&["--model", &request.model]);
            match request.permissions {
                Permissions::Prompt => (),
                Permissions::AcceptEdits => add(&["--permission-mode", "acceptEdits"]),
                Permissions::Auto => add(&[
                    "--permission-mode",
                    "auto",
                    "--settings",
                    r#"{"enabledMcpjsonServers":["ahu"]}"#,
                ]),
            }
        }
        Mode::Headless => {
            add(&[
                "--print",
                "--model",
                &request.model,
                "--output-format",
                "stream-json",
                "--verbose",
                "--permission-prompts",
                "none",
                "--permission-mode",
                "auto",
            ]);
            if let Some(session) = session {
                add(&["--resume", session]);
            }
        }
    }
    add(&["--"]);

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

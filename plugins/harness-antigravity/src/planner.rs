//! Native argument mapping for the explicitly accepted CLI version.
use crate::protocol::{Mode, PROMPT, Permissions, Plan, Request};
pub(crate) const HARNESS: &str = "antigravity";
pub(crate) const PROGRAM: &str = "agy";
pub(crate) const NATIVE_VERSION: &str = "1.4.7";

pub(crate) fn plan(request: Request) -> Result<Plan, &'static str> {
    let session = request.validate()?;
    let mut args = Vec::new();
    let mut add = |values: &[&str]| args.extend(values.iter().map(|value| (*value).to_owned()));

    add(&["--model", &request.model]);
    match request.mode {
        Mode::Interactive => {
            match request.permissions {
                Permissions::Prompt => (),
                Permissions::AcceptEdits => add(&["--mode", "accept-edits"]),
                Permissions::Auto => add(&["--dangerously-skip-permissions"]),
            }
            add(&["--prompt-interactive"]);
        }
        Mode::Headless => {
            add(&[
                "--output-format",
                "stream-json",
                "--print-timeout",
                &format!("{}s", request.timeout_seconds),
                "--dangerously-skip-permissions",
            ]);
            if let Some(session) = session {
                add(&["--conversation", session]);
            }
            add(&["--print"]);
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

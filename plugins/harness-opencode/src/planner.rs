//! Native argument mapping for the explicitly accepted CLI version.
use crate::protocol::{Mode, PROMPT, Permissions, Plan, Request};
pub(crate) const HARNESS: &str = "opencode";
pub(crate) const PROGRAM: &str = "opencode";
pub(crate) const NATIVE_VERSION: &str = "2.0.32";

pub(crate) fn plan(request: Request) -> Result<Plan, &'static str> {
    let session = request.validate()?;
    let mut args = Vec::new();
    let mut add = |values: &[&str]| args.extend(values.iter().map(|value| (*value).to_owned()));

    let Some((provider, model)) = request.model.split_once('/') else {
        return Err("invalid provider/model");
    };
    if !crate::protocol::valid_model(provider)
        || provider.contains(':')
        || !crate::protocol::valid_model(model)
        || model.contains('/')
        || provider == "."
        || provider == ".."
    {
        return Err("invalid provider/model");
    }
    match request.mode {
        Mode::Interactive => {
            if request.permissions != Permissions::Prompt {
                return Err("interactive permission mode unsupported");
            }
            add(&["mini", "--model", &request.model, "--prompt"]);
        }
        Mode::Headless => {
            add(&[
                "run",
                "--standalone",
                "--format",
                "json",
                "--model",
                &request.model,
                "--auto",
            ]);
            if let Some(session) = session {
                add(&["--session", session]);
            }
            add(&["--"]);
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

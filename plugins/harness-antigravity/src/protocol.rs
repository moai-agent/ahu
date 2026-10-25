//! Strict, bounded launch-planning protocol. No native process or configuration access.
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub(crate) const LIMIT: usize = 65_536;
pub(crate) const PROMPT: &str = "<prompt: see prompt.txt>";

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Mode {
    Interactive,
    Headless,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Permissions {
    Prompt,
    AcceptEdits,
    Auto,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub(crate) protocol_version: u64,
    pub(crate) harness: String,
    pub(crate) mode: Mode,
    pub(crate) model: String,
    pub(crate) permissions: Permissions,
    pub(crate) work_mode: bool,
    // Value deliberately requires presence, unlike Option's implicit missing default.
    pub(crate) session: serde_json::Value,
    pub(crate) native_version: String,
    pub(crate) timeout_seconds: u64,
}

#[derive(Serialize)]
pub(crate) struct Plan {
    pub(crate) protocol_version: u64,
    pub(crate) harness: &'static str,
    pub(crate) program: &'static str,
    pub(crate) args: Vec<String>,
    pub(crate) prompt_arg: usize,
}

impl Request {
    pub(crate) fn validate(&self) -> Result<Option<&str>, &'static str> {
        if self.protocol_version != 2 || self.harness != crate::planner::HARNESS {
            return Err("unsupported protocol or harness");
        }
        if self.native_version != crate::planner::NATIVE_VERSION {
            return Err("unsupported native version");
        }
        if !(1..=604_800).contains(&self.timeout_seconds) {
            return Err("invalid timeout");
        }
        // Bounded ASCII identifiers keep model flags literal and exclude whitespace,
        // controls, NUL, shell syntax, and the reserved prompt placeholder.
        if !valid_model(&self.model) {
            return Err("invalid model");
        }
        let session = match &self.session {
            serde_json::Value::Null => None,
            serde_json::Value::String(value)
                if (1..=128).contains(&value.len())
                    && value
                        .as_bytes()
                        .first()
                        .is_some_and(u8::is_ascii_alphanumeric)
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') =>
            {
                Some(value.as_str())
            }
            _ => return Err("invalid session"),
        };
        match self.mode {
            Mode::Interactive if self.work_mode || session.is_some() => {
                return Err("interactive work mode or resume unsupported");
            }
            Mode::Headless if !self.work_mode || self.permissions != Permissions::Auto => {
                return Err("guarded headless mode unsupported");
            }
            _ => (),
        }
        Ok(session)
    }
}

pub(crate) fn valid_model(model: &str) -> bool {
    (1..=256).contains(&model.len())
        && model
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:/".contains(&b))
}

pub(crate) fn response(raw: &[u8]) -> Result<Vec<u8>, &'static str> {
    if raw.len() > LIMIT {
        return Err("input limit exceeded");
    }
    let request: Request = serde_json::from_slice(raw).map_err(|_| "invalid request")?;
    encode(&crate::planner::plan(request)?)
}

fn encode(plan: &Plan) -> Result<Vec<u8>, &'static str> {
    let mut output = serde_json::to_vec(plan).map_err(|_| "invalid response")?;
    if output.len() >= LIMIT {
        return Err("output limit exceeded");
    }
    output.push(b'\n');
    Ok(output)
}

pub(crate) fn run() -> Result<(), &'static str> {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--ahu-harness-launch-protocol=2"))
        || arguments.next().is_some()
    {
        return Err("invalid invocation");
    }
    let mut raw = Vec::new();
    std::io::stdin()
        .lock()
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut raw)
        .map_err(|_| "input failed")?;
    let output = response(&raw)?;
    std::io::stdout()
        .lock()
        .write_all(&output)
        .map_err(|_| "output failed")
}

#[cfg(test)]
mod tests;

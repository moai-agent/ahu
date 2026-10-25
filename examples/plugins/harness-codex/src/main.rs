//! Independent, side-effect-free Codex launch translation over stdin/stdout.
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::process::ExitCode;

const LIMIT: usize = 65_536;
const PROMPT: &str = "<prompt: see prompt.txt>";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    protocol_version: u64,
    harness: String,
    mode: String,
    model: String,
    permissions: String,
    work_mode: bool,
    // Value makes null explicit but still requires the field to be present.
    session: serde_json::Value,
}

#[derive(Serialize)]
struct Plan {
    protocol_version: u64,
    harness: &'static str,
    program: &'static str,
    args: Vec<String>,
    prompt_arg: usize,
}

fn plan(request: Request) -> Result<Plan, ()> {
    if request.protocol_version != 1 || request.harness != "codex" {
        return Err(());
    }
    if request.model.is_empty() || request.model.starts_with('-') || request.model.contains('\0') {
        return Err(());
    }
    let session = match &request.session {
        serde_json::Value::Null => None,
        serde_json::Value::String(value) if !value.is_empty() && !value.contains('\0') => {
            Some(value.as_str())
        }
        _ => return Err(()),
    };
    let mut args = Vec::new();
    match request.mode.as_str() {
        "headless" => {
            if !request.work_mode || request.permissions != "auto" {
                return Err(());
            }
            args.push("exec".to_owned());
            if session.is_some() {
                args.push("resume".to_owned());
            }
            args.extend([
                "--model".to_owned(),
                request.model,
                "--json".to_owned(),
                "-c".to_owned(),
                "approval_policy=\"never\"".to_owned(),
                "-c".to_owned(),
                "sandbox_mode=\"danger-full-access\"".to_owned(),
            ]);
            if session.is_none() {
                args.extend(["--color".to_owned(), "never".to_owned()]);
            }
            args.push("--".to_owned());
            if let Some(session) = session {
                args.push(session.to_owned());
            }
        }
        "interactive" => {
            if request.work_mode || session.is_some() {
                return Err(());
            }
            args.extend(["-m".to_owned(), request.model]);
            match request.permissions.as_str() {
                "auto" => args.extend(
                    [
                        "--ask-for-approval",
                        "never",
                        "--sandbox",
                        "workspace-write",
                    ]
                    .map(str::to_owned),
                ),
                "accept-edits" => args.push("--approve-for-me".to_owned()),
                "prompt" => {}
                _ => return Err(()),
            }
            args.push("--".to_owned());
        }
        _ => return Err(()),
    }
    args.push(PROMPT.to_owned());
    Ok(Plan {
        protocol_version: 1,
        harness: "codex",
        program: "codex",
        prompt_arg: args.len() - 1,
        args,
    })
}

fn response(raw: &[u8]) -> Result<Vec<u8>, ()> {
    let request = serde_json::from_slice(raw).map_err(|_| ())?;
    let json = serde_json::to_string(&plan(request)?).map_err(|_| ())?;
    // Preserve ensure_ascii framing and its byte-limit semantics, including
    // UTF-16 surrogate pairs for non-BMP characters and escaping DEL.
    let mut output = String::new();
    for character in json.chars() {
        if character < '\u{7f}' {
            output.push(character);
        } else {
            use std::fmt::Write;
            for unit in character.encode_utf16(&mut [0; 2]) {
                write!(output, "\\u{unit:04x}").map_err(|_| ())?;
            }
        }
        if output.len() >= LIMIT {
            return Err(());
        }
    }
    output.push('\n');
    Ok(output.into_bytes())
}

fn run() -> Result<(), ()> {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--ahu-harness-launch-protocol=1"))
        || arguments.next().is_some()
    {
        return Err(());
    }
    let mut raw = Vec::new();
    std::io::stdin()
        .lock()
        .take((LIMIT + 1) as u64)
        .read_to_end(&mut raw)
        .map_err(|_| ())?;
    if raw.len() > LIMIT {
        return Err(());
    }
    let output = response(&raw)?;
    std::io::stdout().lock().write_all(&output).map_err(|_| ())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::from(2),
    }
}

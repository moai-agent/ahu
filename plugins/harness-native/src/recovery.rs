//! Bounded sanitized native completion retrieval. Never replays a model request.
use crate::{Error, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Params {
    harness: String,
    native_version: String,
    native_executable: PathBuf,
    expected_executable_digest: Option<String>,
    worktree: PathBuf,
    session: String,
    attempt_started_ms: u64,
    home: Option<PathBuf>,
    #[serde(default)]
    environment: crate::Environment,
}
fn executable_digest(path: &std::path::Path) -> Result<String> {
    use std::os::unix::fs::OpenOptionsExt;
    crate::files::check_components(path)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    const MAX: u64 = 512 * 1024 * 1024;
    if !metadata.is_file() || metadata.len() > MAX {
        return Err(Error::invalid());
    }
    let mut hash = Sha256::new();
    let count = std::io::copy(&mut file.take(MAX + 1), &mut hash)?;
    if count > MAX {
        return Err(Error::invalid());
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub(crate) fn call(harness: &str, value: &Value) -> Result<Value> {
    let p: Params = crate::decode(value)?;
    if p.harness != harness {
        return Err(Error::invalid());
    }
    if harness != "opencode"
        || crate::compatibility::version_token(&p.native_version) != Some("2.0.32")
    {
        return Ok(json!({"recovered":false}));
    }
    let Some(expected) = &p.expected_executable_digest else {
        return Ok(json!({"recovered":false}));
    };
    if expected.len() != 64
        || !expected.bytes().all(|c| c.is_ascii_hexdigit())
        || p.attempt_started_ms == 0
        || !(8..=128).contains(&p.session.len())
        || !p.session.starts_with("ses_")
        || !p.session[4..].bytes().all(|c| c.is_ascii_alphanumeric())
    {
        return Err(Error::invalid());
    }
    crate::directory(&p.worktree)?;
    if let Some(home) = &p.home {
        crate::directory(home)?;
    }
    if p.environment.len() > 128
        || p.environment.iter().any(|(k, v)| {
            k.is_empty() || k.contains(['=', '\0']) || v.contains('\0') || k == "HOME"
        })
    {
        return Err(Error::invalid());
    }
    if executable_digest(&p.native_executable)? != *expected {
        return Err(Error::invalid());
    }
    let before = crate::files::snapshot(&p.worktree)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let recovered = loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Error::uncertain());
        }
        let mut command = Command::new(&p.native_executable);
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .envs(&p.environment)
            .env("PWD", &p.worktree)
            .env("OTEL_SDK_DISABLED", "true")
            .env("NO_COLOR", "1")
            .current_dir(&p.worktree)
            .args([
                "session",
                "export",
                "--sanitize",
                "--standalone",
                &p.session,
            ]);
        if let Some(home) = &p.home {
            command.env("HOME", home);
        }
        let bytes = crate::process::run_command(command, remaining)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| Error::uncertain())?;
        match ahu_harness_events::recovery::export_establishes_completion(
            text,
            &p.session,
            p.attempt_started_ms,
        ) {
            Ok(()) => break true,
            Err("the export is not JSON") => std::thread::sleep(
                Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
            ),
            Err(_) => break false,
        }
    };
    if executable_digest(&p.native_executable).map_err(|_| Error::uncertain())? != *expected
        || crate::files::snapshot(&p.worktree).map_err(|_| Error::uncertain())? != before
    {
        return Err(Error::uncertain());
    }
    Ok(json!({"recovered":recovered}))
}

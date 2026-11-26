//! Core-owned runtime for a harness that has no plugin and is integrated only
//! over MCP. It answers exactly `probe` and `launch`; everything else a plugin
//! provides (models, admission, setup, health, event decoding) is unsupported.
//! The declared native program is run once, bounded, for `--version` only.
use crate::util::{Error, Result};
use ahu_harness_api::Descriptor;
use serde_json::{Value, json};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const VERSION_OUTPUT_LIMIT: u64 = 16 * 1024;

pub(super) fn call(
    descriptor: &Descriptor,
    timeout: Duration,
    operation: &str,
    params: &Value,
) -> Result<Value> {
    match operation {
        "probe" => probe(params, timeout),
        "launch" => launch(descriptor, params),
        _ => Err(Error::new("unsupported harness runtime operation")),
    }
}

fn launch(descriptor: &Descriptor, params: &Value) -> Result<Value> {
    // A generic harness has no known way to take a prompt, model or permission
    // flags, so only a direct coordinator session is planned.
    if params.get("mode").and_then(Value::as_str) != Some("direct") {
        return Err(Error::new(
            "a generic MCP-only harness supports direct sessions only",
        ));
    }
    Ok(json!({
        "native_eval_otlp": false,
        "program": descriptor.program,
        "args": [],
        "prompt_arg": null,
    }))
}

fn probe(params: &Value, timeout: Duration) -> Result<Value> {
    let executable = params
        .get("native_executable")
        .and_then(Value::as_str)
        .map(Path::new)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| Error::new("probe requires an absolute native executable"))?;
    let home = params
        .get("home")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new("probe requires a native home"))?;
    let mut command = Command::new(executable);
    command
        .arg("--version")
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|_| Error::new("cannot run the native executable"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::new("native probe has no output"))?;
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        let _ = (&mut stdout)
            .take(VERSION_OUTPUT_LIMIT)
            .read_to_end(&mut output);
        output
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break Some(status),
            None if Instant::now() >= deadline => break None,
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    if status.is_none() {
        if let Ok(id) = i32::try_from(child.id()) {
            // SAFETY: the negative id names the child's own fresh process group.
            unsafe {
                libc::kill(-id, libc::SIGKILL);
            }
        }
        let _ = child.wait();
    }
    let output = reader.join().unwrap_or_default();
    if !status.is_some_and(|status| status.success()) {
        return Err(Error::new("native version probe failed"));
    }
    let text = String::from_utf8_lossy(&output);
    let version = text
        .split_whitespace()
        .map(|word| word.trim_start_matches('v').trim_end_matches(','))
        .find(|word| crate::util::is_semver(word))
        .ok_or_else(|| Error::new("native executable reported no semantic version"))?;
    // No reviewed compatibility evidence exists for a harness ahu has no plugin
    // for, and nothing about its model, identity or confinement is enforced.
    Ok(json!({
        "version": version,
        "compatibility": {},
        "reviewed_version": false,
        "enforcement": {
            "model": "not_enforced",
            "identity": "not_established",
            "filesystem_confinement": "not_established",
            "live_conformance": "untested",
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ahu_harness_api::{Mode, Operation};

    fn descriptor() -> Descriptor {
        Descriptor {
            id: "sixth".into(),
            display_name: "Sixth".into(),
            program: "sixth".into(),
            aliases: vec![],
            models: vec![],
            modes: vec![Mode::Direct],
            operations: vec![Operation::Probe, Operation::Launch],
            capabilities: vec!["mcp-only".into()],
            tested_versions: vec![],
            context_paths: vec![],
            private_context_paths: vec![],
            skill_paths: vec![],
            native_source_paths: vec![],
            environment_keys: vec![],
        }
    }

    #[test]
    fn plans_only_a_direct_session_with_no_arguments() {
        let plan = launch(&descriptor(), &json!({"mode": "direct"})).unwrap();
        assert_eq!(plan["program"], "sixth");
        assert_eq!(plan["args"], json!([]));
        for mode in ["interactive", "headless"] {
            assert!(launch(&descriptor(), &json!({ "mode": mode })).is_err());
        }
    }

    #[test]
    fn other_operations_are_unsupported() {
        let timeout = Duration::from_secs(1);
        for operation in ["models", "admission", "setup", "auth"] {
            assert!(call(&descriptor(), timeout, operation, &json!({})).is_err());
        }
    }

    #[test]
    fn probe_reads_a_semantic_version_from_the_native_executable() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("sixth");
        std::fs::write(&script, "#!/bin/sh\necho 'sixth v1.2.3 (build x)'\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let params = json!({"native_executable": script, "home": directory.path()});
        // A generous bound on the success path. This case asserts the parsed
        // version, never how quickly the probe returns, and a short budget
        // here only invents failures when the machine is loaded enough that
        // spawning a shell takes seconds. The refusal below keeps the short
        // budget, because there the timeout is the behaviour under test.
        let value = probe(&params, Duration::from_secs(120)).unwrap();
        assert_eq!(value["version"], "1.2.3");
        assert_eq!(value["reviewed_version"], false);
        std::fs::write(&script, "#!/bin/sh\nsleep 30\n").unwrap();
        assert!(probe(&params, Duration::from_millis(200)).is_err());
    }
}

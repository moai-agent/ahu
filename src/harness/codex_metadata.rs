//! Bounded Codex 0.157.1 metadata inspection, before any thread is started.
//! The native process retains authentication and managed policy. Raw responses
//! and stderr are never persisted or included in diagnostics.
use crate::util::{Error, Result};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const LIMIT: usize = 1024 * 1024;

/// Uses exactly the executable and isolation switches used by batch execution.
/// Requirements must be absent: a future policy change refuses admission rather
/// than suppressing mandatory plugin hooks or changing the required features.
pub fn inspect(executable: &Path, cwd: &Path) -> Result<Value> {
    let profile =
        super::isolation::profile("codex", "0.157.1").expect("reviewed Codex metadata profile");
    let mut command = Command::new(executable);
    command
        .args(["app-server", "--listen", "stdio://"])
        .args(profile.args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::cmux::integration::sanitize(&mut command, None);
    command.env("CMUX_CODEX_HOOKS_DISABLED", "1");
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|_| Error::new("cannot start native Codex metadata inspection"))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let mut stdin = child.stdin.take().expect("piped stdin");
    let (tx, rx) = std::sync::mpsc::sync_channel(4);
    let reader = std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut total = 0;
        loop {
            let mut line = Vec::new();
            // Bound a single unterminated line as well as the entire stream.
            use std::io::Read;
            let read = reader
                .by_ref()
                .take((LIMIT - total + 1) as u64)
                .read_until(b'\n', &mut line);
            match read {
                Ok(0) => break,
                Ok(n) if total + n <= LIMIT => total += n,
                _ => {
                    let _ = tx.send(None);
                    break;
                }
            }
            if tx.send(Some(line)).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let outcome = (|| {
        let send = |stdin: &mut std::process::ChildStdin, value: Value| -> Result<()> {
            let mut bytes = serde_json::to_vec(&value)?;
            bytes.push(b'\n');
            stdin
                .write_all(&bytes)
                .map_err(|_| Error::new("native Codex metadata input closed"))
        };
        send(
            &mut stdin,
            json!({"id":1,"method":"initialize","params":{
            "clientInfo":{"name":"ahu_headless_inspection","version":"1.0.0"},
            "capabilities":{"experimentalApi":true}}}),
        )?;
        let mut initialized = false;
        let mut hooks = None;
        let mut requirements = false;
        for _ in 0..256 {
            let line = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| {
                    Error::new(
                        "native Codex metadata inspection timed out or exited before completion",
                    )
                })?
                .ok_or_else(|| Error::new("native Codex metadata exceeded the inspection bound"))?;
            let event: Value = serde_json::from_slice(&line)
                .map_err(|_| Error::new("invalid native Codex metadata response"))?;
            if !event.is_object() {
                return Err(Error::new(
                    "native Codex metadata response is not an object",
                ));
            }
            if let Some(error) = event.get("error").filter(|error| !error.is_null()) {
                return Err(Error::new(format!(
                    "native Codex metadata RPC error (request {:?}, code {:?}); raw native error omitted",
                    event.get("id").and_then(Value::as_u64),
                    error.get("code").and_then(Value::as_i64)
                )));
            }
            if event
                .get("method")
                .and_then(Value::as_str)
                .is_some_and(|m| m.to_ascii_lowercase().contains("warning"))
            {
                return Err(Error::new(format!(
                    "native Codex metadata warning (SHA-256 {}); raw native warning omitted",
                    crate::util::digest_bytes(&serde_json::to_vec(
                        event.get("params").unwrap_or(&Value::Null)
                    )?)
                )));
            }
            match event.get("id").and_then(Value::as_u64) {
                Some(1)
                    if !initialized
                        && event.get("result").is_some_and(Value::is_object)
                        && event.get("method").is_none() =>
                {
                    initialized = true;
                    send(&mut stdin, json!({"method":"initialized"}))?;
                    send(
                        &mut stdin,
                        json!({"id":2,"method":"hooks/list","params":{"cwds":[cwd]}}),
                    )?;
                    send(
                        &mut stdin,
                        json!({"id":3,"method":"configRequirements/read","params":null}),
                    )?;
                }
                Some(2) if initialized && hooks.is_none() => {
                    hooks = Some(
                        event
                            .get("result")
                            .cloned()
                            .ok_or_else(|| Error::new("missing native hook result"))?,
                    );
                }
                Some(3) if initialized && !requirements => {
                    require_no_policy(event.get("result").unwrap_or(&Value::Null))?;
                    requirements = true;
                }
                Some(_) => return Err(Error::new("unexpected native Codex metadata response")),
                None if event.get("id").is_none() => (),
                None => return Err(Error::new("unexpected native Codex metadata request")),
            }
            if requirements && let Some(hooks) = hooks.take() {
                return Ok(hooks);
            }
        }
        Err(Error::new("native Codex metadata event limit exceeded"))
    })();
    drop(stdin);
    // This process is owned exclusively by this inspection, never a daemon or
    // an existing user session. Kill its group even after successful metadata.
    crate::headless::signal_group(child.id(), libc::SIGKILL);
    let _ = child.wait();
    drop(rx);
    let _ = reader.join();
    outcome
}

fn require_no_policy(result: &Value) -> Result<()> {
    if result.get("requirements") != Some(&Value::Null) {
        return Err(Error::new(
            "Codex managed requirements are present or unresolved; the optional-plugin isolation profile is not admitted",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_requirements_are_explicit_and_never_inferred_from_empty_objects() {
        require_no_policy(&json!({"requirements":null})).unwrap();
        for value in [
            json!({}),
            json!(null),
            json!({"requirements":{}}),
            json!({"requirements":{"featureRequirements":{"plugins":true}}}),
            json!({"requirements":{"hooks":{"Stop":[]}}}),
        ] {
            assert!(require_no_policy(&value).is_err());
        }
    }
    #[test]
    fn metadata_protocol_sends_no_thread_calls_and_rejects_required_policy() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().canonicalize().unwrap();
        let executable = cwd.join("native");
        let requests = cwd.join("requests");
        for requirements in [Value::Null, json!({"featureRequirements":{"plugins":true}})] {
            let quote = crate::util::shell_single_quote;
            let hooks = json!({"id":2,"error":null,"result":{"data":[{"cwd":cwd,"hooks":[],"warnings":[],"errors":[]}]}});
            let policy = json!({"id":3,"result":{"requirements":requirements}});
            let script = format!(
                "#!/bin/sh\ncase \"$*\" in *features.hooks=true*features.plugins=false*features.remote_plugin=false*notify=*) ;; *) exit 19;; esac\nread -r first\nprintf '%s\\n' \"$first\" > {}\nprintf '%s\\n' '{{\"id\":1,\"result\":{{}}}}'\nfor n in 1 2 3; do read -r request; printf '%s\\n' \"$request\" >> {}; done\nprintf '%s\\n' {} {}\n",
                quote(&requests.to_string_lossy()),
                quote(&requests.to_string_lossy()),
                quote(&hooks.to_string()),
                quote(&policy.to_string())
            );
            std::fs::write(&executable, script).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            let result = inspect(&executable, &cwd);
            assert_eq!(result.is_ok(), requirements.is_null());
            let methods: Vec<String> = std::fs::read_to_string(&requests)
                .unwrap()
                .lines()
                .map(|line| {
                    serde_json::from_str::<Value>(line).unwrap()["method"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                })
                .collect();
            assert_eq!(
                methods,
                [
                    "initialize",
                    "initialized",
                    "hooks/list",
                    "configRequirements/read"
                ]
            );
        }
    }

    #[test]
    fn metadata_protocol_refuses_exit_malformed_and_oversized_output() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().canonicalize().unwrap();
        let executable = cwd.join("native");
        for body in [
            "exit 1",
            "printf '%s\\n' '{\"method\":\"configWarning\",\"params\":{\"summary\":\"synthetic project trust warning\"}}'",
            "printf '%s\\n' 'malformed'",
            "/usr/bin/head -c 1048577 /dev/zero",
        ] {
            std::fs::write(&executable, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(inspect(&executable, &cwd).is_err());
        }
    }
}

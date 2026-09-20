//! Read-only, bounded projections of existing attempt metadata.
use super::{ConfinedDir, Spec};
use crate::util::{Error, Result, display_safe};
use serde_json::{Value, json};
use std::io::Read;
use std::path::Path;

const LIMIT: u64 = 1024 * 1024;

/// Include external tasks whose headless spec has gone missing or is a bad link.
pub(crate) fn is_headless(dir: &Path) -> bool {
    dir.join("headless.json").symlink_metadata().is_ok() || super::confined(dir, false).is_ok()
}

/// Review keeps incomplete or unreadable external task directories visible.
/// Lifecycle discovery still requires a record and retains its fail-closed policy.
pub(crate) fn directories(repo: &crate::git::Repo) -> Result<Vec<std::path::PathBuf>> {
    let mut paths = Vec::new();
    for store in super::stores(repo)? {
        super::confined_in(repo, &store, false)?;
        let entries = match std::fs::read_dir(&store) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if crate::task::is_canonical_task_uuid(&name)
                || (!name.is_empty() && name.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                paths.push(entry.path());
            }
        }
    }
    Ok(paths)
}

/// Preserve the runtime backend's directory checks before generic record loading.
pub(crate) fn record(repo: &crate::git::Repo, dir: &Path) -> Result<crate::task::TaskRecord> {
    super::confined_in(repo, dir, false)?;
    crate::task::load(dir)
}

/// Pin the parent and refuse links, devices and oversized files before parsing.
/// Parsing errors deliberately omit values from private records.
pub(super) fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_slice(&read_bytes(path, Some(LIMIT))?)
        .map_err(|_| Error::new("metadata is malformed or has an unsupported shape"))
}

/// Lifecycle reads preserve the legacy unbounded result API; inspection passes
/// a bound. Both paths pin and validate the actual descriptor identically.
pub(super) fn read_bytes(path: &Path, limit: Option<u64>) -> Result<Vec<u8>> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let directory = path.parent().ok_or_else(|| Error::new("missing parent"))?;
    let parent = ConfinedDir::open(directory)?;
    let private = crate::storage::HeadlessStore::containing(directory)?.is_some();
    let name = ConfinedDir::entry_name(
        path.file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::new("invalid metadata name"))?,
    )?;
    if parent.kind(&name)? != Some(libc::S_IFREG) {
        return Err(Error::new("metadata is missing or is not a regular file"));
    }
    // SAFETY: parent owns the directory descriptor and name is NUL terminated.
    let fd = unsafe {
        libc::openat(
            parent.handle.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: openat returned a new, owned descriptor.
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    crate::storage::validate_owned_metadata(&file.metadata()?, private)?;
    let mut bytes = Vec::new();
    if let Some(limit) = limit {
        if file.metadata()?.len() > limit {
            return Err(Error::new("metadata exceeds the inspection bound"));
        }
        file.take(limit + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(Error::new("metadata exceeds the inspection bound"));
        }
    } else {
        (&file).read_to_end(&mut bytes)?;
    }
    Ok(bytes)
}

pub(super) fn validate_result(value: &Value, id: &str, attempt: u32) -> Result<()> {
    let outcome = value["outcome"].as_str();
    if !matches!(value["schema_version"].as_u64(), Some(1 | 2))
        || value["task_id"] != id
        || value["attempt"] != attempt
        || !matches!(
            outcome,
            Some(
                "succeeded"
                    | "failed"
                    | "cancelled"
                    | "timed_out"
                    | "capture_failed"
                    | "supervisor_error"
            )
        )
        || (outcome == Some("succeeded")
            && (value["process"]["exit_code"] != 0
                || value["harness"]["terminal"] != true
                || value["harness"]["failed"] != false))
    {
        return Err(Error::new(
            "result metadata is malformed, unsupported, or belongs to another attempt",
        ));
    }
    for blockers in [&value["blockers"], &value["harness"]["blockers"]] {
        if !blockers.is_null()
            && !blockers
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string))
        {
            return Err(Error::new("result blockers are malformed"));
        }
    }
    Ok(())
}

pub(super) fn projection(
    dir: &Path,
    spec: Option<&Spec>,
    result: Option<&Value>,
    error: Option<&str>,
) -> Value {
    let id = dir.file_name().unwrap_or_default().to_string_lossy();
    let ownership = super::supervisor_owns_attempt(dir).ok();
    let liveness = crate::task::observed_liveness(ownership).as_str();
    let mut blockers: Vec<Value> = Vec::new();
    if let Some(result) = result {
        for field in [&result["blockers"], &result["harness"]["blockers"]] {
            if let Some(items) = field.as_array() {
                blockers.extend(items.iter().cloned());
            }
        }
    }
    if let Some(error) = error {
        blockers.push(error.into());
    }
    if ownership.is_none() {
        blockers.push("supervisor ownership unavailable; liveness unknown".into());
    }
    let checkpoint = spec.and_then(|spec| {
        let path = super::attempt_dir(dir, spec).join("native-session.json");
        if matches!(path.symlink_metadata(), Err(e) if e.kind() == std::io::ErrorKind::NotFound) {
            return None;
        }
        let validated = (|| -> Result<super::SessionCheckpoint> {
            let checkpoint: super::SessionCheckpoint = read(&path)?;
            let record: crate::task::TaskRecord = read(&dir.join("task.json"))?;
            checkpoint.validate(&record, spec, &id)?;
            Ok(checkpoint)
        })();
        match validated {
            Ok(checkpoint) => Some(checkpoint),
            Err(_) => {
                blockers.push("native session checkpoint unavailable: unreadable, unsupported, invalid session or mismatched task/attempt/harness ownership".into());
                None
            }
        }
    });
    let session = result
        .and_then(|v| v["harness"]["session"].as_str())
        .filter(|s| !s.is_empty());
    let (session, source) = if let Some(session) = session {
        (Some(session), "result.json harness.session")
    } else if let Some(session) = checkpoint.as_ref().map(|c| c.session.as_str()) {
        (Some(session), "checkpoint; harness event stream")
    } else if let Some(session) = spec
        .and_then(|s| s.session.as_deref())
        .filter(|s| !s.is_empty())
    {
        (Some(session), "headless.json resume target")
    } else {
        (None, "unknown")
    };
    // Commands are offered only for identifiers accepted by headless lookup.
    let commands = if !id.is_empty() && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
        vec![
            format!("ahu task ahu:task:{id}"),
            format!("ahu result ahu:task:{id} --output json"),
            format!("ahu diff ahu:task:{id}"),
            format!("ahu wait ahu:task:{id} --output json"),
        ]
    } else {
        Vec::new()
    };
    let attempt = spec.map(|s| super::attempt_dir(dir, s));
    json!({
        "backend":"headless", "task_id":id, "task_ref":crate::task_ref::display(&id), "number":spec.map(|s| s.attempt),
        "outcome":result.map(|v| &v["outcome"]).cloned().unwrap_or(json!("unavailable")),
        "outcome_source":if error.is_some() {"unavailable"} else if result.is_some_and(|v| v["outcome"] == "running" || v["outcome"] == "interrupted") {"ownership observation"} else {"result.json"},
        "liveness":liveness, "ownership_source":"owner.lock observation", "supervisor_owned":ownership,
        "session_state":read::<Value>(&dir.join("task.json")).ok().and_then(|v| v["state"].as_str().map(str::to_owned)),
        "native_session":session, "native_session_source":source,
        "native_data":"harness-owned; no verified native data location recorded",
        "blockers":blockers, "runtime":dir, "record_path":dir.join("task.json"),
        "result_path":attempt.as_ref().map(|p| p.join("result.json")),
        "result_metadata":if error.is_some() {"unavailable"} else if result.is_some_and(|v| v["outcome"] == "running" || v["outcome"] == "interrupted") {"missing"} else {"available"},
        "agent_report_path":spec.filter(|s| s.schema_version == 1).map(|_|dir.join("result.md")), "final_path":attempt.as_ref().filter(|_| spec.is_some_and(|s| s.schema_version == 1)).map(|p| p.join("final.txt")), "attempt_path":attempt,
        "parent_task":spec.and_then(|s| s.parent_task.as_deref()),
        "native_helpers":spec.map(|s| &s.options.native_helpers),
        "timeout_seconds":spec.map(|s| s.options.timeout_seconds),
        "captured_artifacts_removed":spec.map(|s| super::attempt_dir(dir,s).join("artifacts-removed.json").exists()),
        "completion_verified":false, "acceptance":"not assessed", "commands":commands
    })
}

pub(crate) fn safe(text: &str) -> String {
    let mut chars = text.chars();
    let mut text: String = chars.by_ref().take(512).collect();
    if chars.next().is_some() {
        text.push_str("… [truncated]");
    }
    display_safe(&text)
}

fn field(value: &Value, name: &str) -> String {
    match &value[name] {
        Value::String(s) => safe(s),
        Value::Null => "unknown".into(),
        other => safe(&other.to_string()),
    }
}

pub(crate) fn render(value: &Value, concise: bool) -> String {
    let mut out = format!(
        "  attempt   {} / {} ({})\n",
        field(value, "number"),
        field(value, "outcome"),
        field(value, "outcome_source")
    );
    if concise {
        if value["result_metadata"] == "unavailable" {
            out.push_str("  review    metadata unavailable; inspect task details\n");
        }
        if let Some(command) = value["commands"]
            .as_array()
            .and_then(|v| v.first())
            .and_then(Value::as_str)
        {
            out.push_str(&format!("  inspect   {}\n", safe(command)));
        }
        return out;
    }
    out.push_str(&format!("  ownership {} (owner.lock observation)\n  native    {} ({})\n  data      {}\n  runtime   {}\n  attempt directory {}\n  result metadata {} ({})\n  agent report {} (if provided)\n  captured final {} (if retained)\n  captures removed {}\n",
        field(value,"liveness"),field(value,"native_session"),field(value,"native_session_source"),field(value,"native_data"),field(value,"runtime"),field(value,"attempt_path"),field(value,"result_path"),field(value,"result_metadata"),field(value,"agent_report_path"),field(value,"final_path"),field(value,"captured_artifacts_removed")));
    if let Some(blockers) = value["blockers"].as_array() {
        if blockers.is_empty() {
            out.push_str("  blockers  none recorded\n");
        }
        for blocker in blockers.iter().take(8).filter_map(Value::as_str) {
            out.push_str(&format!("  blocker   {}\n", safe(blocker)));
        }
        if blockers.len() > 8 {
            out.push_str("  blockers  additional entries omitted; inspect result JSON\n");
        }
    }
    if let Some(commands) = value["commands"].as_array() {
        for command in commands.iter().filter_map(Value::as_str) {
            out.push_str(&format!("  review    {}\n", safe(command)));
        }
    }
    out.push_str("Process outcome is evidence only. Task completion is not verified; acceptance is not assessed.\n");
    out
}

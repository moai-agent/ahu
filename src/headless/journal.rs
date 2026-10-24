//! Durable JSON records and resume-journal recovery for headless attempts.

use super::Spec;
use super::admission::attempt_dir;
use super::confined;
use super::review;
use crate::util::{Error, Result};
use crate::{state, task};
use serde::Deserialize;
use std::path::Path;

pub(crate) fn durable_json(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::new("runtime file needs a parent"))?;
    confined(parent, true)?;
    state::write_durable_json_compact(path, value)
}

pub(crate) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_slice(&review::read_bytes(
        path,
        Some(crate::util::MAX_TASK_RECORD_BYTES),
    )?)
    .map_err(|e| Error::new(format!("{} is not valid ahu state: {e}", path.display())))
}

/// Restore only metadata for an attempt that has never acknowledged process start.
/// This is invoked by explicit resume, never by read-only inspection or a timer.
pub(super) fn recover_resume(dir: &Path) -> Result<()> {
    let journal = dir.join("resume-journal.json");
    if !journal.exists() {
        return Ok(());
    }
    #[derive(Deserialize)]
    struct Journal {
        record: task::TaskRecord,
        spec: Spec,
        prompt: String,
        next_attempt: u32,
    }
    let old: Journal = read_json(&journal)?;
    let next = Spec {
        attempt: old.next_attempt,
        ..old.spec.clone()
    };
    if !attempt_dir(dir, &next).join("spawn-intent.json").exists() {
        task::save(dir, &old.record, &old.prompt)?;
        durable_json(&dir.join("headless.json"), &old.spec)?;
    }
    std::fs::remove_file(journal)?;
    Ok(())
}

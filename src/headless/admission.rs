//! Admission gates for headless launches: placement intent, ownership locks, and parent-attempt validation.

use super::Spec;
use super::journal::read_json;
use super::lookup;
use crate::util::{Error, Result};
use crate::{bail, state};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) const PLACEMENT_INTENT: &str = "placement-intent.json";

#[derive(Serialize, Deserialize)]
pub(super) struct PlacementIntent {
    pub(super) schema_version: u32,
    pub(super) task_id: String,
    pub(super) worktree: PathBuf,
    pub(super) branch: String,
    pub(super) pid: i32,
    pub(super) timestamp: u64,
}

pub(super) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|e| e.as_secs())
        .unwrap_or(0)
}

/// Consume any leftover launch intent before admission, healing worktrees left
/// by a crashed or killed launch. Unverifiable records do not authorize cleanup.
/// Age alone never proves the recorder is dead, and Git retains dirty worktrees.
pub(super) fn sweep_placement_intent(
    console: &mut crate::launcher::Console<'_>,
    repo: &crate::git::Repo,
    store_dir: &Path,
) -> Result<()> {
    let intent_path = store_dir.join(PLACEMENT_INTENT);
    if !intent_path.try_exists()? {
        return Ok(());
    }
    let raw =
        match super::review::read_bytes(&intent_path, Some(crate::util::MAX_TASK_RECORD_BYTES)) {
            Ok(raw) => raw,
            Err(_) => {
                console.say("cannot safely read launch intent; worktree cleanup skipped\n")?;
                return Ok(());
            }
        };
    let Ok(intent) = serde_json::from_slice::<PlacementIntent>(&raw) else {
        console.say(&format!(
            "cannot parse {}; ignoring the stale launch intent\n",
            intent_path.display()
        ))?;
        let _ = std::fs::remove_file(&intent_path);
        return Ok(());
    };
    if intent.schema_version != 1
        || intent.pid <= 0
        || intent.task_id.is_empty()
        || !intent
            .task_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        console.say("invalid launch intent identity; worktree cleanup skipped\n")?;
        return Ok(());
    }
    let recorder_dead = {
        let rc = unsafe { libc::kill(intent.pid, 0) };
        rc != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    };
    let task_record_exists = store_dir.join(&intent.task_id).join("task.json").exists();
    let expected_worktree = state::worktree_dir(&repo.primary_root()?, &intent.task_id)?;
    let removable = recorder_dead && !task_record_exists && intent.worktree == expected_worktree;
    if removable {
        let worktree = intent.worktree.to_string_lossy().to_string();
        match crate::git::run(&repo.root, &["worktree", "remove", "--", &worktree]) {
            Ok(output) if output.status.success() => {
                console.say(&format!(
                    "removed worktree from an interrupted launch: {worktree}\n"
                ))?;
            }
            Ok(output) => {
                console.say(&format!(
                    "could not remove worktree from an interrupted launch: {worktree}: {}\n",
                    String::from_utf8_lossy(&output.stderr)
                ))?;
            }
            Err(error) => {
                console.say(&format!(
                    "could not remove worktree from an interrupted launch: {worktree}: {error}\n"
                ))?;
            }
        }
    } else {
        let reason = if task_record_exists {
            "its task record exists"
        } else if intent.worktree != expected_worktree {
            "the recorded worktree does not match this repository's layout"
        } else {
            "its recording process cannot be confirmed dead"
        };
        console.say(&format!(
            "kept worktree from an interrupted launch: {}: {reason}\n",
            intent.worktree.display()
        ))?;
    }
    let _ = std::fs::remove_file(&intent_path);
    Ok(())
}

pub(super) fn attempt_dir(dir: &Path, spec: &Spec) -> PathBuf {
    dir.join(format!("attempt-{}", spec.attempt))
}

/// Count prior assignments for the admission gates in one pass over the
/// domain. A task whose headless.json cannot be read is excluded from both
/// counts and reported: one unreadable record must not block all submissions
/// by failing the launch, nor silently count as active.
pub(super) fn admission_counts(
    console: &mut crate::launcher::Console<'_>,
    previous: &[PathBuf],
    root_task: &Option<String>,
) -> Result<(usize, usize)> {
    let mut tree_count = 0;
    let mut active = 0;
    for dir in previous {
        let Ok(recorded) = read_json::<Spec>(&dir.join("headless.json")) else {
            console.say(&format!(
                "cannot read {}/headless.json: treating as unknown; admission counts may under-count\n",
                dir.display()
            ))?;
            continue;
        };
        if recorded.root_task == *root_task {
            tree_count += 1;
        }
        if !attempt_dir(dir, &recorded).join("result.json").exists() {
            active += 1;
        }
    }
    Ok((tree_count, active))
}

/// OS-owned locks are released on process loss, never stolen based on age.
pub(super) struct Lock(std::fs::File);
impl Lock {
    pub(super) fn acquire(path: &Path) -> Result<Self> {
        Self::try_acquire(path)?.ok_or_else(|| {
            Error::new(
                "attempt is already owned by a live supervisor; concurrent execution refused",
            )
        })
    }
    pub(super) fn is_owned(path: &Path) -> Result<bool> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        // Inspection neither creates nor writes the ownership file.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        crate::storage::validate_owned_metadata(&file.metadata()?, true)?;
        // SAFETY: file owns a valid descriptor; dropping it releases a successful probe.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(false);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            Ok(true)
        } else {
            Err(error.into())
        }
    }
    pub(super) fn try_acquire(path: &Path) -> Result<Option<Self>> {
        Self::try_open(path, true)
    }
    pub(super) fn try_acquire_existing(path: &Path) -> Result<Option<Self>> {
        Self::try_open(path, false)
    }
    fn try_open(path: &Path, create: bool) -> Result<Option<Self>> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;
        state::confine_file(path)?;
        let file = std::fs::OpenOptions::new()
            .create(create)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)?;
        crate::storage::validate_owned_metadata(&file.metadata()?, true)?;
        // SAFETY: file owns a valid descriptor for the duration of the call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(None);
            }
            return Err(error.into());
        }
        Ok(Some(Self(file)))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: this object still owns the descriptor.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

pub(super) fn validate_parent_attempt(repo: &crate::git::Repo, spec: &Spec) -> Result<()> {
    let Some(parent) = &spec.parent_task else {
        return Ok(());
    };
    let dir = lookup(repo, parent)?;
    let parent_spec: Spec = read_json(&dir.join("headless.json"))?;
    if spec.parent_attempt != Some(parent_spec.attempt) {
        bail!("stale parent attempt cannot admit or start a child");
    }
    let attempt = attempt_dir(&dir, &parent_spec);
    if dir.join("cancel.json").exists()
        || attempt.join("admission-closed.json").exists()
        || attempt.join("result.json").exists()
    {
        bail!("parent attempt ended or closed child admission");
    }
    if Lock::try_acquire(&dir.join("owner.lock"))?.is_some() {
        bail!("parent supervisor is no longer live; child dispatch refused");
    }
    let request = spec
        .broker_request
        .as_deref()
        .filter(|id| id.len() == 16 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| Error::new("child lacks a broker request identity"))?;
    let claim: Value = read_json(&attempt.join("broker").join(format!("{request}.claim.json")))?;
    if claim["state"] != "dispatching"
        || claim["parent_attempt"] != parent_spec.attempt
        || claim["request_id"] != request
    {
        bail!("child dispatch does not match a consumed request in the live parent attempt");
    }
    Ok(())
}

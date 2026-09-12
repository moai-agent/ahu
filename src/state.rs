//! ahu's local state directory.
//!
//! Everything here is operational state: task records, worktrees, the
//! repository-to-cmux-group mapping, and hygiene review timestamps. None of it
//! is policy. Policy lives in the repository, the same for every user.

use std::path::{Path, PathBuf};

use crate::bail;
use crate::util::{Error, Result};

/// Root of ahu's local state, overridable with `AHU_STATE_DIR` so tests and
/// sandboxes never touch a developer's real state.
pub fn root() -> Result<PathBuf> {
    if let Some(explicit) = std::env::var_os("AHU_STATE_DIR") {
        return Ok(PathBuf::from(explicit));
    }
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(xdg).join("ahu"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| {
        Error::new(
            "cannot locate ahu's state directory: neither AHU_STATE_DIR, XDG_STATE_HOME, nor HOME is set.",
        )
    })?;
    Ok(PathBuf::from(home).join(".local/state/ahu"))
}

pub fn repo_dir(repo_identity: &str) -> Result<PathBuf> {
    Ok(root()?.join("repos").join(repo_identity))
}

pub fn tasks_dir(repo_identity: &str) -> Result<PathBuf> {
    Ok(repo_dir(repo_identity)?.join("tasks"))
}

pub fn task_dir(repo_identity: &str, task_id: &str) -> Result<PathBuf> {
    Ok(tasks_dir(repo_identity)?.join(task_id))
}

pub fn worktree_dir(repo_identity: &str, task_id: &str) -> Result<PathBuf> {
    Ok(repo_dir(repo_identity)?.join("worktrees").join(task_id))
}

/// Path of the lock that serialises find-or-create of a repository's cmux group
/// and the allocation of task identifiers.
pub fn lock_path(repo_identity: &str) -> Result<PathBuf> {
    Ok(repo_dir(repo_identity)?.join("launch.lock"))
}

/// A crude cross-process lock built on exclusive file creation.
///
/// Concurrent launches must not create two cmux groups for one repository or
/// collide on a branch. A stale lock older than `STALE_AFTER` is reclaimed so a
/// killed launch cannot wedge the repository permanently.
pub struct LaunchLock {
    path: PathBuf,
}

const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(120);

impl LaunchLock {
    pub fn acquire(repo_identity: &str) -> Result<Self> {
        let path = lock_path(repo_identity)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    use std::io::Write;
                    let _ = write!(file, "{}", std::process::id());
                    return Ok(Self { path });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if let Ok(meta) = std::fs::metadata(&path)
                        && let Ok(modified) = meta.modified()
                        && modified.elapsed().unwrap_or_default() > STALE_AFTER
                    {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if std::time::Instant::now() >= deadline {
                        bail!(
                            "another ahu launch is in progress for this repository ({}).\n\
                             Wait for it to finish, or remove the lock if you are sure it died.",
                            path.display()
                        );
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => bail!("cannot create {}: {e}", path.display()),
            }
        }
    }
}

impl Drop for LaunchLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Read a JSON state file, treating "missing" as "default".
pub fn read_json<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)
            .map_err(|e| Error::new(format!("{} is not valid ahu state: {e}", path.display())))?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => bail!("cannot read {}: {e}", path.display()),
    }
}

/// Write a JSON state file atomically, so a crash cannot leave a half file.
pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("tmp{}", std::process::id()));
    let body = serde_json::to_vec_pretty(value)
        .map_err(|e| Error::new(format!("cannot serialize state: {e}")))?;
    std::fs::write(&temp, &body)?;
    std::fs::rename(&temp, path)
        .map_err(|e| Error::new(format!("cannot write {}: {e}", path.display())))?;
    Ok(())
}

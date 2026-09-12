//! ahu's local state directory.
//!
//! This module locates operational state: task records, task worktrees, the
//! repository-to-cmux-group mapping, and hygiene review timestamps. None of it
//! is policy. Policy lives in the repository, the same for every user.

use std::path::{Path, PathBuf};

use crate::bail;
use crate::util::{Error, Result};

/// Root of ahu's local state inside the current checkout. Linked worktrees
/// have their own `.ahu/state` rather than writing to the parent.
/// `AHU_STATE_DIR` remains an explicit override for isolated tests and tools.
pub fn root() -> Result<PathBuf> {
    if let Some(explicit) = std::env::var_os("AHU_STATE_DIR") {
        return Ok(PathBuf::from(explicit));
    }
    default_root(&std::env::current_dir()?)
}

fn default_root(start: &Path) -> Result<PathBuf> {
    let repo = crate::git::discover(start)?;
    checkout_root(&repo.root)
}

pub fn checkout_root(checkout: &Path) -> Result<PathBuf> {
    let local = checkout.join(".ahu");
    let root = local.join("state");
    // Read-only commands must not create state or follow repository-supplied
    // links that redirect the default store outside the checkout.
    for path in [&local, &root] {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                bail!(
                    "refusing ahu state path {}: expected a real directory, not a symlink or file",
                    path.display()
                );
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(state_io_error("inspect directory", path, e)),
        }
    }
    Ok(root)
}

/// Create session state inside its checkout, ignored without editing tracked
/// repository policy. Never follow a repository-provided state/ignore symlink.
pub fn ensure_checkout_state(checkout: &Path) -> Result<PathBuf> {
    let root = checkout_root(checkout)?;
    let local = root.parent().expect("state has local directory");
    std::fs::create_dir_all(&root).map_err(|e| state_io_error("create directory", &root, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(local, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| state_io_error("set private permissions", &root, e))?;
    }
    let ignore = local.join(".gitignore");
    match std::fs::symlink_metadata(&ignore) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            bail!(
                "refusing ahu state ignore path {}: expected a regular file",
                ignore.display()
            );
        }
        Ok(_) => {
            let content = std::fs::read_to_string(&ignore)
                .map_err(|e| state_io_error("read ignore file", &ignore, e))?;
            if !ignores_everything(&content) {
                bail!(
                    "{} must ignore all state files with a bare * and no negations",
                    ignore.display()
                );
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&ignore)
                .map_err(|e| state_io_error("create ignore file", &ignore, e))?;
            file.write_all(b"# Local ahu session state. Never commit.\n*\n")
                .map_err(|e| state_io_error("write ignore file", &ignore, e))?;
        }
        Err(e) => return Err(state_io_error("inspect ignore file", &ignore, e)),
    }
    Ok(root)
}

pub fn repo_dir(repo_identity: &str) -> Result<PathBuf> {
    Ok(root()?.join("repos").join(repo_identity))
}

/// The group mapping and launch lock coordinate sibling worktrees, so they
/// belong to the primary checkout. Per-session data stays in its own checkout.
pub fn coordination_dir(repo: &crate::git::Repo) -> Result<PathBuf> {
    let current = root()?;
    let shared = if current == checkout_root(&repo.root)? {
        checkout_root(&repo.primary_root()?)?
    } else {
        // Preserve explicit isolated stores used by embedding tools and tests.
        current
    };
    Ok(shared.join("repos").join(repo.identity()))
}

pub fn tasks_dir(repo_identity: &str) -> Result<PathBuf> {
    Ok(repo_dir(repo_identity)?.join("tasks"))
}

pub fn task_dir(repo_identity: &str, task_id: &str) -> Result<PathBuf> {
    Ok(tasks_dir(repo_identity)?.join(task_id))
}

/// Directory holding a repository's task worktrees.
///
/// Task worktrees live under `.worktrees/` in the primary checkout, including
/// when launched from another task. That
/// keeps a task's checkout discoverable from the repository it belongs to.
///
/// The directory ignores itself (see [`WORKTREES_GITIGNORE`]), so it never
/// appears in `git status` and cannot be committed by accident.
pub fn worktrees_root(repo_root: &Path) -> Result<PathBuf> {
    Ok(crate::git::discover(repo_root)?
        .primary_root()?
        .join(WORKTREES_DIR))
}

/// Name of the in-repository directory holding task worktrees.
pub const WORKTREES_DIR: &str = ".worktrees";

/// Contents of the `.gitignore` ahu places inside `.worktrees/`.
///
/// `*` ignores every entry including this file, so the whole directory is
/// invisible to Git without ahu ever editing the repository's own `.gitignore`.
pub const WORKTREES_GITIGNORE: &str =
    "# Created by ahu. Task worktrees are local state, never committed.\n*\n";

/// Path of one task's worktree, inside its repository.
pub fn worktree_dir(repo_root: &Path, task_id: &str) -> Result<PathBuf> {
    Ok(worktrees_root(repo_root)?.join(task_id))
}

/// Create `.worktrees/` and make it ignore itself.
///
/// `.worktrees` sits inside the repository, so a repository can commit a symlink
/// at that path. Following it would put the task checkout, the harness's working
/// directory, and every configuration file ahu copies wherever the repository
/// chose — `~/.claude/skills`, say, which would install a machine-wide skill.
/// So the path is required to be a real directory, and a symlink is refused.
pub fn ensure_worktrees_root(repo_root: &Path) -> Result<PathBuf> {
    let root = worktrees_root(repo_root)?;
    match std::fs::symlink_metadata(&root) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let points_to = std::fs::read_link(&root)
                .map(|t| t.to_string_lossy().to_string())
                .unwrap_or_else(|_| "an unreadable target".to_string());
            bail!(
                "refusing to use {} because it is a symlink pointing at {points_to}.\n\
                 Task worktrees must live inside the repository. A symlink here would send this \
                 task's checkout, its working directory, and every configuration file ahu copies \
                 to a path the repository chose, so nothing was created.\n\
                 Inspect that path in the repository before launching again.",
                root.display()
            );
        }
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => bail!(
            "{} exists and is not a directory, so ahu cannot put task worktrees there.",
            root.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&root)
                .map_err(|e| Error::new(format!("cannot create {}: {e}", root.display())))?;
        }
        Err(e) => bail!("cannot inspect {}: {e}", root.display()),
    }
    let ignore = root.join(".gitignore");
    // `.worktrees/.gitignore` is an ordinary path inside the repository, so a
    // repository can commit one. Checking only that *a* file is there would let
    // a committed `# nothing ignored here` stand in for the real one, and then
    // task checkouts show up in the parent's `git status`, can be swept into a
    // `git add -A`, and stop being protected from `git clean -xdf`. So the
    // contents are verified, not just the existence.
    match std::fs::symlink_metadata(&ignore) {
        Ok(meta) if meta.file_type().is_symlink() => bail!(
            "refusing to write {} because it is a symlink.",
            ignore.display()
        ),
        Ok(meta) if meta.is_dir() => bail!(
            "{} is a directory, so ahu cannot make {} ignore itself.",
            ignore.display(),
            root.display()
        ),
        Ok(_) => {
            let found = std::fs::read_to_string(&ignore)
                .map_err(|e| Error::new(format!("cannot read {}: {e}", ignore.display())))?;
            if !ignores_everything(&found) {
                bail!(
                    "{} exists but does not ignore everything under {}.\n\
                     ahu relies on that directory ignoring itself so task checkouts never appear \
                     in git status and cannot be committed by accident. This file is in the \
                     repository, so it may have been committed deliberately.\n\
                     Inspect it, then either delete it so ahu can write its own or give it a bare \
                     `*` line with no negations.",
                    ignore.display(),
                    root.display()
                );
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::write(&ignore, WORKTREES_GITIGNORE)
                .map_err(|e| Error::new(format!("cannot create {}: {e}", ignore.display())))?;
        }
        Err(e) => bail!("cannot inspect {}: {e}", ignore.display()),
    }
    Ok(root)
}

/// Whether a `.gitignore` body really ignores every entry beside it.
///
/// A bare `*` line is what does the ignoring; a later negation (`!keep-me`)
/// takes entries back out, which is exactly the hole this is looking for.
fn ignores_everything(body: &str) -> bool {
    let mut ignores_all = false;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "*" {
            ignores_all = true;
        } else if line.starts_with('!') {
            return false;
        }
    }
    ignores_all
}

/// Confirm a prepared task worktree really sits inside the repository.
///
/// Checked again at exec time, because `.worktrees` could have been replaced
/// between submission and the workspace shell starting.
pub fn verify_worktree_inside_repo(repo_root: &Path, worktree: &Path) -> Result<()> {
    let root = worktrees_root(repo_root)?;
    if let Ok(meta) = std::fs::symlink_metadata(&root)
        && meta.file_type().is_symlink()
    {
        bail!(
            "{} is a symlink, so ahu cannot confirm this task's worktree is inside the repository.",
            root.display()
        );
    }
    // Both paths have to resolve. `Path::starts_with` compares components
    // literally, so an unresolved `<repo>/.worktrees/../../elsewhere` would pass
    // a prefix test while naming a directory outside the repository. Falling
    // back to the unresolved path on a canonicalize failure would turn this
    // check into exactly that prefix test, so a path that cannot be resolved is
    // refused instead. Nothing legitimate reaches here unresolvable: the
    // worktree has been created and materialized into by the time it is checked.
    let canonical_repo = root
        .parent()
        .expect("worktrees has parent")
        .canonicalize()
        .map_err(|e| {
            Error::new(format!(
                "cannot resolve the repository root {}: {e}. \
             ahu will not start a session it cannot place inside a repository.",
                repo_root.display()
            ))
        })?;
    let canonical_worktree = worktree.canonicalize().map_err(|e| {
        Error::new(format!(
            "cannot resolve the task worktree {}: {e}. \
             ahu will not start a session in a directory it cannot confirm is inside {}.",
            worktree.display(),
            repo_root.display()
        ))
    })?;
    if !canonical_worktree.starts_with(&canonical_repo) {
        bail!(
            "task worktree {} resolves outside its repository ({}). ahu will not start a session there.",
            canonical_worktree.display(),
            canonical_repo.display()
        );
    }
    Ok(())
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

fn state_io_error(operation: &str, path: &Path, error: std::io::Error) -> Error {
    let mut message = format!(
        "cannot {operation} ahu state at {}: {error}",
        path.display()
    );
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        message.push_str(
            "\nCheck directory permissions and the calling process's sandbox. \
             Run ahu from a terminal with access to this state directory, or set \
             AHU_STATE_DIR to an absolute, writable path inside the checkout's ignored .ahu directory. \
             Use the same AHU_STATE_DIR for subsequent ahu commands; changing it \
             does not migrate existing task records. State access does not grant \
             permission to create Git worktrees or access cmux.",
        );
    }
    Error::new(message)
}

impl LaunchLock {
    pub fn acquire(repo_identity: &str) -> Result<Self> {
        Self::acquire_at(lock_path(repo_identity)?)
    }

    pub fn acquire_at(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            create_private_dir_all(parent)?;
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
                Err(e) => return Err(state_io_error("create launch lock", &path, e)),
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
        Err(e) => Err(state_io_error("read", path, e)),
    }
}

/// Create a directory tree that only its owner can enter.
///
/// Task state contains prompts and records and should be accessible only to
/// the owning account. Worktree locations are managed separately.
pub fn create_private_dir_all(dir: &Path) -> Result<()> {
    let state_root = root()?;
    if let Some(local_state) = dir.ancestors().find(|ancestor| {
        ancestor.file_name().is_some_and(|name| name == "state")
            && ancestor
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == ".ahu")
    }) {
        ensure_checkout_state(
            local_state
                .parent()
                .and_then(Path::parent)
                .ok_or_else(|| Error::new("invalid checkout state root"))?,
        )?;
    }
    std::fs::create_dir_all(dir).map_err(|e| state_io_error("create directory", dir, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut current = PathBuf::new();
        for component in dir.components() {
            current.push(component);
            if (current.starts_with(&state_root) || current == dir) && current.is_dir() {
                let _ = std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o700));
            }
        }
    }
    Ok(())
}

/// Write a JSON state file atomically, so a crash cannot leave a half file.
pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("tmp{}", std::process::id()));
    let body = serde_json::to_vec_pretty(value)
        .map_err(|e| Error::new(format!("cannot serialize state: {e}")))?;
    std::fs::write(&temp, &body).map_err(|e| state_io_error("write temporary file", &temp, e))?;
    // Owner-only here, so no caller has to remember. State records carry task
    // titles, hook labels, and repository paths.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| state_io_error("set private permissions", &temp, e))?;
    }
    std::fs::rename(&temp, path).map_err(|e| state_io_error("replace file", path, e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn permission_errors_explain_state_location_and_sandbox_recovery() {
        // EPERM (sandbox denial) and EACCES (filesystem permissions) must both
        // explain recovery. Inject errors so this also works when run as root.
        for code in [1, 13] {
            let error = state_io_error(
                "write temporary file",
                Path::new("/example/state/hygiene.tmp123"),
                std::io::Error::from_raw_os_error(code),
            )
            .to_string();
            assert!(error.contains("write temporary file"), "{error}");
            assert!(error.contains("/example/state/hygiene.tmp123"), "{error}");
            assert!(error.contains("sandbox"), "{error}");
            assert!(error.contains("AHU_STATE_DIR"), "{error}");
            assert!(error.contains("does not migrate"), "{error}");
        }
    }

    #[test]
    fn failed_state_write_names_the_operation_and_preserves_previous_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hygiene.json");
        std::fs::write(&path, "{}").unwrap();
        let temp = path.with_extension(format!("tmp{}", std::process::id()));
        std::fs::create_dir(&temp).unwrap();
        let error = write_json(&path, &serde_json::json!({"updated": true}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("write temporary file"), "{error}");
        assert!(error.contains(&temp.display().to_string()), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
    }
}

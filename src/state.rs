//! ahu's local state directory.
//!
//! This module locates operational state: task records, task worktrees, the
//! repository-to-cmux-group mapping, and hygiene review timestamps. None of it
//! is policy. Policy lives in the repository, the same for every user.

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use crate::bail;
use crate::util::{Error, Result};

/// Local state of the invoking checkout, discovered from the working directory.
pub fn root() -> Result<PathBuf> {
    let repo = crate::git::discover(&std::env::current_dir()?)?;
    crate::storage::CheckoutStorage::new(&repo.root).state_root()
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
    let local = root
        .parent()
        .expect("state has local directory")
        .to_path_buf();
    // One component at a time, so neither name is created through a link that
    // was already there. `checkout_root` inspected both just above; inspecting
    // a path and then acting on it cannot prevent something replacing it in
    // between, and nothing here claims otherwise.
    create_one_dir(&local)?;
    set_private_mode(&local)?;
    create_one_dir(&root)?;
    set_private_mode(&root)?;
    let local = local.as_path();
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

/// Repository-wide coordination belongs to the primary checkout.
pub fn coordination_dir(repo: &crate::git::Repo) -> Result<PathBuf> {
    crate::storage::RepositoryStorage::new(repo)?.coordination_dir()
}

pub fn tasks_dir(repo_identity: &str) -> Result<PathBuf> {
    Ok(repo_dir(repo_identity)?.join("tasks"))
}

pub fn task_dir(repo_identity: &str, task_id: &str) -> Result<PathBuf> {
    Ok(tasks_dir(repo_identity)?.join(task_id))
}

/// Where one task's record and prompt live: inside that task's own worktree.
///
/// A task's operational state belongs to the checkout the task works in, so it
/// is created with the worktree and goes away with it. Nothing has to be set in
/// the environment for that to happen: the location is derived from the
/// worktree ahu just created.
///
/// This is a path, not a promise that it is safe to write: the worktree does
/// not exist yet at plan time. [`ensure_checkout_state`] and
/// [`create_private_dir_all`] do the symlink checks when it is created.
pub fn worktree_task_dir(worktree: &Path, repo_identity: &str, task_id: &str) -> PathBuf {
    crate::storage::CheckoutStorage::new(worktree).task_dir(repo_identity, task_id)
}

/// The checkout whose store holds `path`, when `path` is inside one.
///
/// `<checkout>/.ahu/state/...` answers `<checkout>`; external stores answer nothing.
pub fn enclosing_checkout(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| is_checkout_state_root(ancestor))
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

/// A repository's task worktrees, without re-discovering the repository.
///
/// [`worktrees_root`] shells out to Git to find the primary checkout. A caller
/// that already holds one passes it here instead.
pub fn worktrees_root_at(primary_root: &Path) -> PathBuf {
    primary_root.join(WORKTREES_DIR)
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

pub(crate) fn state_io_error(operation: &str, path: &Path, error: std::io::Error) -> Error {
    let mut message = format!(
        "cannot {operation} ahu state at {}: {error}",
        path.display()
    );
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        message.push_str(
            "\nCheck directory permissions and the calling process's sandbox. \
             Run ahu from a terminal with access to the state directory shown above. \
             State access does not grant \
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
        confine_file(&path)?;
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
                    // `symlink_metadata`, so a link planted at the lock name is
                    // aged and unlinked as itself rather than as its target.
                    if let Ok(meta) = std::fs::symlink_metadata(&path)
                        && (meta.file_type().is_symlink()
                            || meta
                                .modified()
                                .is_ok_and(|m| m.elapsed().unwrap_or_default() > STALE_AFTER))
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
///
/// The path is validated first, so a link a repository committed under
/// `.ahu/state` cannot make this read a file outside the store.
pub fn read_json<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Result<T> {
    if confine_file(path)?.is_none() {
        return Ok(T::default());
    }
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
///
/// Every component below the state root is created one at a time and refused if
/// it is already a symlink; see [`confine_dir`] for why.
pub fn create_private_dir_all(dir: &Path) -> Result<()> {
    let Some(base) = confinement_base(dir) else {
        // A directory ahu did not choose: an embedding tool's own store, given
        // to a library entry point. It is the caller's path, not a repository's.
        std::fs::create_dir_all(dir).map_err(|e| state_io_error("create directory", dir, e))?;
        set_private_mode(dir)?;
        return Ok(());
    };
    ensure_base(&base)?;
    confine_dir(&base, dir, true)
}

/// The state root a path is confined to, when ahu chose that root itself.
///
/// Components at and above the root are the invoking user's own: the checkout
/// ahu was run from. Components below it are
/// reachable by a repository, because `.ahu/` sits inside the checkout and Git
/// will happily check out a tracked symlink at `.ahu/state/repos`.
///
/// The boundary is the *outermost* one that applies, never the innermost. A
/// `.ahu/state` pair appearing further down a path is an ordinary pair of names
/// below the root already in force: treating it as a new root would let a
/// descendant move the boundary past the link that reached it, which is the one
/// thing the boundary exists to prevent.
fn confinement_base(path: &Path) -> Option<PathBuf> {
    // `ancestors` runs deepest first, so the last match is the shallowest.
    path.ancestors()
        .filter(|ancestor| is_checkout_state_root(ancestor))
        .last()
        .map(Path::to_path_buf)
}

/// `<checkout>/.ahu/state`, the default store of one working tree.
fn is_checkout_state_root(path: &Path) -> bool {
    path.file_name() == Some(OsStr::new("state"))
        && path.parent().and_then(Path::file_name) == Some(OsStr::new(".ahu"))
}

/// Check the state root itself without creating anything.
///
/// A caller that reads a record directly -- `task::load` on a path it was
/// handed, say -- has not necessarily been through `root()`, so the two
/// components that make up a checkout's default store are validated here rather
/// than assumed to have been validated earlier.
fn verify_base(base: &Path) -> Result<()> {
    checkout_root(checkout_for_state(base)?)?;
    Ok(())
}

/// Make sure the checkout state root itself exists, with its own symlink checks.
fn ensure_base(base: &Path) -> Result<()> {
    ensure_checkout_state(checkout_for_state(base)?)?;
    Ok(())
}

fn checkout_for_state(base: &Path) -> Result<&Path> {
    if !is_checkout_state_root(base) {
        bail!("invalid checkout state root");
    }
    base.parent()
        .and_then(Path::parent)
        .ok_or_else(|| Error::new("invalid checkout state root"))
}

/// Walk `dir` from `base` down, refusing to traverse a symlink at any component
/// below the root, and optionally creating the missing ones.
///
/// `create_dir_all` would create, chmod, and later write *through* a link a
/// repository committed at `.ahu/state/repos`, landing all of it wherever the
/// link points. So each component is inspected before it is entered.
///
/// This refuses links that are already there, which is what a checked-out
/// repository can plant. It is not a guarantee against a process racing the
/// walk; no sequence of path-name operations can offer one.
fn confine_dir(base: &Path, dir: &Path, create: bool) -> Result<()> {
    let relative = dir.strip_prefix(base).map_err(|_| {
        Error::new(format!(
            "{} is not inside the ahu state directory {}",
            dir.display(),
            base.display()
        ))
    })?;
    let mut current = base.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            bail!(
                "refusing ahu state path {}: it contains a traversal component",
                dir.display()
            );
        };
        current.push(name);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => return Err(symlink_refusal(&current)),
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => bail!(
                "refusing ahu state path {}: expected a directory, found a file",
                current.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if !create {
                    // Nothing below this exists, so there is nothing to follow.
                    return Ok(());
                }
                create_one_dir(&current)?;
            }
            Err(e) => return Err(state_io_error("inspect directory", &current, e)),
        }
        if create {
            set_private_mode(&current)?;
        }
    }
    Ok(())
}

/// Create exactly one directory, re-inspecting if it appeared meanwhile.
fn create_one_dir(path: &Path) -> Result<()> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            match std::fs::symlink_metadata(path) {
                Ok(meta) if meta.file_type().is_symlink() => Err(symlink_refusal(path)),
                Ok(meta) if meta.is_dir() => Ok(()),
                Ok(_) => Err(Error::new(format!(
                    "refusing ahu state path {}: expected a directory, found a file",
                    path.display()
                ))),
                Err(e) => Err(state_io_error("inspect directory", path, e)),
            }
        }
        Err(e) => Err(state_io_error("create directory", path, e)),
    }
}

fn symlink_refusal(path: &Path) -> Error {
    let points_to = std::fs::read_link(path)
        .map(|target| target.to_string_lossy().to_string())
        .unwrap_or_else(|_| "an unreadable target".to_string());
    Error::new(format!(
        "refusing ahu state path {}: it is a symlink pointing at {points_to}.\n\
         ahu state stays inside the store it was given. A link here, which a repository can \
         commit and Git will check out, would put ahu's files and their permissions wherever \
         the link points, so nothing was created, written, or changed.\n\
         Inspect that path in the checkout before running ahu again.",
        path.display()
    ))
}

/// Set owner-only permissions on a directory or file ahu already inspected.
///
/// `set_permissions` takes a path and follows links. Opening the entry first and
/// changing the mode of the descriptor means the mode lands on the inode that
/// was inspected, or on nothing: the identity of the open file is checked
/// against the inspected one and a mismatch is refused rather than chmod-ed.
fn set_private_mode(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let inspected = std::fs::symlink_metadata(path)
            .map_err(|e| state_io_error("inspect directory", path, e))?;
        if inspected.file_type().is_symlink() {
            return Err(symlink_refusal(path));
        }
        let mode = if inspected.is_dir() { 0o700 } else { 0o600 };
        let opened = std::fs::File::open(path)
            .map_err(|e| state_io_error("open for permissions", path, e))?;
        let confirmed = opened
            .metadata()
            .map_err(|e| state_io_error("inspect for permissions", path, e))?;
        if confirmed.dev() != inspected.dev() || confirmed.ino() != inspected.ino() {
            bail!(
                "refusing to change permissions on {}: it was replaced while ahu was \
                 inspecting it.",
                path.display()
            );
        }
        opened
            .set_permissions(std::fs::Permissions::from_mode(mode))
            .map_err(|e| state_io_error("set private permissions", path, e))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Validate every directory component of a state directory that is about to be
/// listed or entered, without creating anything.
///
/// Enumerating state through a link a repository planted would read records
/// from wherever it points, so listing is confined exactly like reading a file.
pub fn confine_existing_dir(dir: &Path) -> Result<()> {
    match confinement_base(dir) {
        Some(base) => {
            verify_base(&base)?;
            confine_dir(&base, dir, false)
        }
        None => Ok(()),
    }
}

/// Validate the directories leading to a state file, and the file itself.
///
/// Returns the metadata of the leaf when it exists. A symlink anywhere below
/// the state root is refused rather than followed, so neither a read nor a
/// write can be redirected outside the store.
pub fn confine_file(path: &Path) -> Result<Option<std::fs::Metadata>> {
    if let Some(base) = confinement_base(path) {
        verify_base(&base)?;
        let parent = path
            .parent()
            .ok_or_else(|| Error::new(format!("{} has no parent", path.display())))?;
        confine_dir(&base, parent, false)?;
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(symlink_refusal(path)),
        // Every state file ahu writes is a regular file. A directory, a FIFO,
        // a socket or a device node at one of these names did not come from
        // ahu, and opening some of them blocks until something else answers --
        // so the kind is checked before anything is opened, not after.
        Ok(meta) if !meta.is_file() => Err(Error::new(format!(
            "refusing ahu state path {}: expected a regular file, found {}.",
            path.display(),
            describe_file_type(&meta)
        ))),
        Ok(meta) => Ok(Some(meta)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(state_io_error("inspect", path, e)),
    }
}

/// Name what is actually at a path, for a refusal the reader can act on.
fn describe_file_type(meta: &std::fs::Metadata) -> &'static str {
    if meta.is_dir() {
        return "a directory";
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let kind = meta.file_type();
        if kind.is_fifo() {
            return "a named pipe";
        }
        if kind.is_socket() {
            return "a socket";
        }
        if kind.is_block_device() || kind.is_char_device() {
            return "a device node";
        }
    }
    "something that is not a regular file"
}

/// Create a new owner-only file, refusing to write through anything already
/// there. `create_new` is `O_EXCL`, so an existing symlink fails rather than
/// being followed, and the owner-only mode is part of the creating call rather
/// than a change made after the file already exists.
pub fn create_new_private_file(path: &Path) -> Result<std::fs::File> {
    for attempt in 0..2 {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        // The mode goes in the creating `open` itself, so the file is never
        // briefly readable by anyone else. `umask` can only clear further bits,
        // and the `set_permissions` below pins the result either way.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(path) {
            Ok(file) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    file.set_permissions(std::fs::Permissions::from_mode(0o600))
                        .map_err(|e| state_io_error("set private permissions", path, e))?;
                }
                return Ok(file);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt == 0 => {
                // A leftover from a process that died mid-write, or a link
                // planted at the name ahu is about to use. Either is removed
                // by name -- `remove_file` unlinks a symlink rather than its
                // target -- and a directory is reported instead of deleted.
                match std::fs::symlink_metadata(path) {
                    // A link at this name is never something ahu left behind,
                    // so it is reported rather than cleared and reused.
                    Ok(meta) if meta.file_type().is_symlink() => {
                        return Err(symlink_refusal(path));
                    }
                    Ok(meta) if meta.is_dir() => {
                        return Err(state_io_error("write temporary file", path, e));
                    }
                    // A plain file here is a temporary left by a process that
                    // died mid-write and whose id has been reused. Unlink it by
                    // name and retry; the parent is already confined.
                    Ok(_) => std::fs::remove_file(path)
                        .map_err(|e| state_io_error("remove stale temporary file", path, e))?,
                    Err(_) => {}
                }
            }
            Err(e) => return Err(state_io_error("write temporary file", path, e)),
        }
    }
    Err(Error::new(format!(
        "cannot create {}: it keeps reappearing while ahu writes state.",
        path.display()
    )))
}

/// Write a JSON state file atomically, so a crash cannot leave a half file.
///
/// The destination and the temporary file are both confined: a symlink at
/// either name is refused rather than written through. `rename` replaces the
/// destination name itself and never follows it, so the swap cannot escape.
pub fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let body = serde_json::to_vec_pretty(value)
        .map_err(|e| Error::new(format!("cannot serialize state: {e}")))?;
    write_private_file(path, &body)
}

/// Write a state file that is not JSON, with the same confinement.
pub fn write_private_file(path: &Path, body: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir_all(parent)?;
    }
    confine_file(path)?;
    crate::private_io::atomic_write(path, body, crate::private_io::Durability::Atomic)
}

/// Read a state file ahu wrote, refusing a redirected path.
pub fn read_private_file(path: &Path) -> Result<Vec<u8>> {
    confine_file(path)?.ok_or_else(|| {
        state_io_error(
            "read",
            path,
            std::io::Error::from(std::io::ErrorKind::NotFound),
        )
    })?;
    std::fs::read(path).map_err(|e| state_io_error("read", path, e))
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
            assert!(error.contains("state directory shown above"), "{error}");
        }
    }

    #[test]
    fn failed_state_write_names_the_operation_and_preserves_previous_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hygiene.json");
        std::fs::write(&path, "{}").unwrap();
        let error = crate::private_io::atomic_write_with(
            &path,
            crate::private_io::Durability::Atomic,
            |file| {
                use std::io::Write;
                file.write_all(b"partial")?;
                Err(std::io::Error::other("injected write failure"))
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("injected write failure"), "{error}");
        assert!(error.contains("replace file"), "{error}");
        assert!(error.contains(&path.display().to_string()), "{error}");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
    }
}

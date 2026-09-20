//! Immutable repository-scoped task handles. UUIDs remain the task identity.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::git::Repo;
use crate::util::{Error, Result};
use crate::{bail, state};

const MAX_ENTRY: u64 = 1024;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    schema_version: u32,
    repo_identity: String,
    task_id: String,
    name: String,
}

/// Explicit names are case-insensitive ASCII slugs; the sigil is optional here.
pub fn name(input: &str) -> Result<String> {
    let value = input
        .strip_prefix('@')
        .unwrap_or(input)
        .to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 48
        || !value.as_bytes()[0].is_ascii_lowercase()
        || !value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        || value.ends_with('-')
        || value.contains("--")
    {
        bail!(kind: crate::util::ErrorKind::Usage,
            "task names must start with a letter and contain at most 48 ASCII letters, digits or single hyphens (for example @storage-cleanup)");
    }
    Ok(value)
}

/// A short deterministic base. Allocation adds a suffix only on collision.
pub fn generated_name(title: &str) -> String {
    let words: Vec<_> = title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .take(4)
        .collect();
    let mut value = words.join("-").to_ascii_lowercase();
    value.truncate(32);
    value = value.trim_end_matches('-').to_string();
    if !value.as_bytes().first().is_some_and(u8::is_ascii_lowercase) {
        value = format!("work-{value}").trim_end_matches('-').to_string();
    }
    value
}

fn root(repo: &Repo) -> Result<PathBuf> {
    Ok(state::coordination_dir(repo)?.join("task-handles"))
}

fn directory(path: &Path, create: bool) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    state::confine_existing_dir(path)?;
    if create {
        state::create_private_dir_all(path)?;
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            // SAFETY: geteuid has no preconditions.
            if !meta.is_dir()
                || meta.uid() != unsafe { libc::geteuid() }
                || meta.mode() & 0o077 != 0
            {
                bail!("task handle directories must be owned by the current user and owner-only");
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !create => (),
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

fn paths(repo: &Repo, create: bool) -> Result<(PathBuf, PathBuf)> {
    let root = root(repo)?;
    directory(&root, create)?;
    let names = root.join("names");
    let ids = root.join("ids");
    directory(&names, create)?;
    directory(&ids, create)?;
    if create {
        // Persist newly created directory entries as well as the binding files.
        // Stop at the verified primary checkout rather than syncing its parents.
        use std::os::unix::fs::OpenOptionsExt;
        let primary = repo.primary_root()?;
        for ancestor in root.ancestors() {
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(ancestor)?
                .sync_all()?;
            if ancestor == primary {
                break;
            }
        }
    }
    Ok((names, ids))
}

fn read(path: &Path, repo: &Repo) -> Result<Option<Binding>> {
    use std::os::unix::fs::OpenOptionsExt;
    if state::confine_file(path)?.is_none() {
        return Ok(None);
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let meta = file.metadata()?;
    crate::storage::validate_owned_metadata(&meta, true)?;
    if meta.len() > MAX_ENTRY {
        bail!("task handle binding exceeds 1 KiB");
    }
    let mut bytes = Vec::new();
    file.take(MAX_ENTRY + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_ENTRY {
        bail!("task handle binding exceeds 1 KiB");
    }
    let value: Binding = serde_json::from_slice(&bytes)?;
    if value.schema_version != 1
        || value.repo_identity != repo.identity()
        || !crate::task::is_canonical_task_uuid(&value.task_id)
        || name(&value.name)? != value.name
    {
        bail!("task handle binding has invalid identity or schema");
    }
    Ok(Some(value))
}

/// Publish once. A crash/failed write leaves a reserved, possibly unreadable
/// name; never reclaim it or risk making an old reference mean another task.
fn create(path: &Path, value: &Binding) -> Result<bool> {
    use std::os::unix::{
        ffi::OsStrExt,
        fs::{OpenOptionsExt, PermissionsExt},
        io::{AsRawFd, FromRawFd},
    };
    let parent = path.parent().expect("binding has a parent");
    let dir = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(parent)?;
    let filename = std::ffi::CString::new(path.file_name().expect("binding has a name").as_bytes())
        .map_err(|_| Error::new("invalid task handle filename"))?;
    // SAFETY: dir is open; filename is a NUL-terminated relative basename.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            filename.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            return Ok(false);
        }
        return Err(error.into());
    }
    // SAFETY: openat returned a fresh descriptor; File takes ownership.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    let bytes = serde_json::to_vec(value)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    dir.sync_all()?;
    Ok(true)
}

/// Reserve before a task can start, shared by interactive and headless launches.
pub fn reserve(repo: &Repo, id: &str, requested: Option<&str>, title: &str) -> Result<String> {
    if !crate::task::is_canonical_task_uuid(id) {
        bail!("task handles require a canonical task UUID");
    }
    let base = match requested {
        Some(value) => name(value)?,
        None => generated_name(title),
    };
    let (names, ids) = paths(repo, true)?;
    if read(&ids.join(format!("{id}.json")), repo)?.is_some() {
        bail!("task already has a reserved name; task names are immutable");
    }
    for suffix in 1..=1000 {
        let candidate = if suffix == 1 {
            base.clone()
        } else {
            format!("{base}-{suffix}")
        };
        let value = Binding {
            schema_version: 1,
            repo_identity: repo.identity(),
            task_id: id.into(),
            name: candidate.clone(),
        };
        if !create(&names.join(format!("{candidate}.json")), &value)? {
            if requested.is_some() {
                bail!(kind: crate::util::ErrorKind::Usage, "task name @{candidate} is already reserved in this repository; choose a different --name");
            }
            continue;
        }
        if !create(&ids.join(format!("{id}.json")), &value)? {
            bail!("task already has a name; new reservation retained, but not activated");
        }
        return Ok(format!("@{candidate}"));
    }
    bail!("generated task name has too many collisions; choose an explicit --name")
}

/// Exact handle lookup only. Both immutable directions must agree.
pub fn resolve(repo: &Repo, input: &str) -> Result<String> {
    let name = name(input)?;
    let (names, ids) = paths(repo, false)?;
    let value = read(&names.join(format!("{name}.json")), repo)?
        .ok_or_else(|| Error::new(format!("no task named @{name} in this repository; use ahu tasks or select a checkout with --repo"))
            .with_kind(crate::util::ErrorKind::Usage))?;
    if value.name != name
        || read(&ids.join(format!("{}.json", value.task_id)), repo)?.as_ref() != Some(&value)
    {
        bail!("task handle binding is incomplete or mismatched; use the canonical task UUID");
    }
    Ok(value.task_id)
}

/// Presentation is optional; malformed bindings never produce a usable handle.
pub fn handle(repo: &Repo, id: &str) -> Result<Option<String>> {
    if !crate::task::is_canonical_task_uuid(id) {
        return Ok(None);
    }
    let (names, ids) = paths(repo, false)?;
    let Some(value) = read(&ids.join(format!("{id}.json")), repo)? else {
        return Ok(None);
    };
    if value.task_id != id
        || read(&names.join(format!("{}.json", value.name)), repo)?.as_ref() != Some(&value)
    {
        bail!("task handle binding is incomplete or mismatched; use the canonical task UUID");
    }
    Ok(Some(format!("@{}", value.name)))
}

pub fn at(dir: &Path, id: &str) -> Option<String> {
    if !crate::task::is_canonical_task_uuid(id) {
        return None;
    }
    let repo = crate::git::discover(dir).ok()?;
    handle(&repo, id).ok().flatten()
}

pub fn label(repo: &Repo, id: &str) -> String {
    let canonical = crate::task_ref::display(id);
    match handle(repo, id).ok().flatten() {
        Some(handle) => format!("{handle} ({canonical})"),
        None => canonical,
    }
}

/// One copy-pasteable argument for suggested commands.
pub fn reference(repo: &Repo, id: &str) -> String {
    handle(repo, id)
        .ok()
        .flatten()
        .unwrap_or_else(|| crate::task_ref::display(id))
}

//! Primary-owned task pointers. Legacy external entries remain read-only.

use std::path::{Path, PathBuf};

use crate::util::{Error, Result};
use crate::{bail, state, task};

pub const INDEX_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoreKind {
    Worktree,
    Headless,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub schema_version: u32,
    pub task_id: String,
    pub repo_identity: String,
    pub checkout: PathBuf,
    pub store: StoreKind,
}

/// The selected repository supplies scope; ambient index overrides are retired.
pub fn index_root() -> Result<PathBuf> {
    let repo = crate::git::discover(&std::env::current_dir()?)?;
    root_for(&repo)
}

pub fn root_for(repo: &crate::git::Repo) -> Result<PathBuf> {
    Ok(crate::storage::RepositoryStorage::new(repo)?
        .primary
        .state_root()?
        .join("task-index"))
}

fn confined(path: &Path, create: bool) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let primary = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| Error::new("invalid task index root"))?;
    let repo = crate::git::discover(primary)?;
    if root_for(&repo)? != path || repo.primary_root()? != primary {
        bail!("task index must belong to the selected primary checkout");
    }
    state::confine_existing_dir(path)?;
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        // SAFETY: geteuid has no preconditions.
        if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            bail!("task index must be an owner-only directory owned by the current user");
        }
    }
    if create {
        state::create_private_dir_all(path)?;
    }
    Ok(())
}

fn legacy_confined(root: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    if crate::storage::external_root(root)? != root {
        bail!("legacy index root is redirected");
    }
    match std::fs::symlink_metadata(root) {
        Ok(meta) => {
            // SAFETY: geteuid has no preconditions.
            if !meta.is_dir()
                || meta.file_type().is_symlink()
                || meta.uid() != unsafe { libc::geteuid() }
                || meta.mode() & 0o077 != 0
            {
                bail!("legacy index must be an owner-only directory owned by the current user");
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

fn entry_path(root: &Path, task_id: &str) -> PathBuf {
    root.join(format!("{task_id}.json"))
}

fn durable_write(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::new("task index file needs a parent"))?;
    confined(parent, true)?;
    state::confine_file(path)?;
    let mut body = serde_json::to_vec(value)?;
    body.push(b'\n');
    crate::private_io::atomic_write(path, &body, crate::private_io::Durability::Durable)
}

fn read_entry(root: &Path, task_id: &str) -> Result<Option<Entry>> {
    let path = entry_path(root, task_id);
    if state::confine_file(&path)?.is_none() {
        return Ok(None);
    }
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)?;
    let meta = file.metadata()?;
    // SAFETY: geteuid has no preconditions.
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.nlink() != 1
        || meta.len() > 65536
    {
        bail!("task index entry must be an owned regular file within 64 KiB");
    }
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        bail!("task index entry exceeds 64 KiB");
    }
    let entry: Entry = serde_json::from_slice(&bytes)
        .map_err(|e| Error::new(format!("invalid {}: {e}", path.display())))?;
    if !matches!(entry.schema_version, 1 | INDEX_SCHEMA_VERSION) {
        bail!(
            "task index entry {} has schema version {}; this ahu expects {}",
            path.display(),
            entry.schema_version,
            INDEX_SCHEMA_VERSION
        );
    }
    crate::storage::validate_owned_metadata(&meta, entry.schema_version == INDEX_SCHEMA_VERSION)?;
    if entry.task_id != task_id {
        bail!(
            "task index entry {} names a different task: {}",
            path.display(),
            entry.task_id
        );
    }
    Ok(Some(entry))
}

/// Record where a task's durable state lives, so other checkouts can find it.
pub fn register(
    repo_identity: &str,
    task_id: &str,
    checkout: &Path,
    store: StoreKind,
) -> Result<()> {
    if !task::is_canonical_task_uuid(task_id) {
        bail!("task index accepts canonical task ids only: {task_id}");
    }
    if !checkout.is_absolute() {
        bail!(
            "task index accepts absolute checkout paths only: {}",
            checkout.display()
        );
    }
    let repo = crate::git::discover(checkout)?;
    if repo.identity() != repo_identity {
        bail!("index registration repository identity mismatch");
    }
    durable_write(
        &entry_path(&root_for(&repo)?, task_id),
        &Entry {
            schema_version: INDEX_SCHEMA_VERSION,
            task_id: task_id.to_string(),
            repo_identity: repo_identity.to_string(),
            checkout: if store == StoreKind::Headless {
                repo.primary_root()?
            } else {
                checkout.to_path_buf()
            },
            store,
        },
    )
}

/// Forget a task. Removing an unknown or legacy id is not an error, because
/// nothing outside the launch path ever registers one.
pub fn remove(task_id: &str) -> Result<()> {
    if !task::is_canonical_task_uuid(task_id) {
        return Ok(());
    }
    let repo = crate::git::discover(&std::env::current_dir()?)?;
    remove_in(&repo, task_id)
}

pub fn remove_in(repo: &crate::git::Repo, task_id: &str) -> Result<()> {
    if !task::is_canonical_task_uuid(task_id) {
        return Ok(());
    }
    let root = root_for(repo)?;
    confined(&root, false)?;
    let path = entry_path(&root, task_id);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    }
    if let Ok(parent) = std::fs::File::open(&root) {
        let _ = parent.sync_all();
    }
    Ok(())
}

/// Look up one task by its canonical id. Grammar forms, uppercase ids, and
/// legacy ids are not looked up here; normalization is the caller's job.
pub fn lookup(task_id: &str) -> Result<Option<Entry>> {
    if !task::is_canonical_task_uuid(task_id) {
        return Ok(None);
    }
    let repo = crate::git::discover(&std::env::current_dir()?)?;
    lookup_in(&repo, task_id)
}

pub fn lookup_in(repo: &crate::git::Repo, task_id: &str) -> Result<Option<Entry>> {
    if !task::is_canonical_task_uuid(task_id) {
        return Ok(None);
    }
    let root = root_for(repo)?;
    confined(&root, false)?;
    if let Some(entry) = read_entry(&root, task_id)? {
        if entry.repo_identity != repo.identity() {
            bail!("primary task index entry belongs to another repository");
        }
        return Ok(Some(entry));
    }
    for root in crate::storage::legacy_index_roots(repo)? {
        legacy_confined(&root)?;
        if let Some(entry) = read_entry(&root, task_id)?
            && entry.repo_identity == repo.identity()
        {
            return Ok(Some(entry));
        }
    }
    Ok(None)
}

fn is_uuid_text_prefix(prefix: &str) -> bool {
    !prefix.is_empty()
        && prefix.len() <= 36
        && prefix
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'-'))
}

/// Find entries whose ids start with the given lowercase prefix. Ambiguous
/// prefixes are the caller's problem: this returns every match, sorted.
pub fn lookup_prefix(prefix: &str) -> Result<Vec<Entry>> {
    if !is_uuid_text_prefix(prefix) {
        return Ok(Vec::new());
    }
    let repo = crate::git::discover(&std::env::current_dir()?)?;
    lookup_prefix_in(&repo, prefix)
}

pub fn lookup_prefix_in(repo: &crate::git::Repo, prefix: &str) -> Result<Vec<Entry>> {
    if !is_uuid_text_prefix(prefix) {
        return Ok(Vec::new());
    }
    let primary = root_for(repo)?;
    confined(&primary, false)?;
    let mut roots = vec![primary];
    roots.extend(crate::storage::legacy_index_roots(repo)?);
    let mut out: Vec<Entry> = Vec::new();
    for (index, root) in roots.iter().enumerate() {
        if index != 0 {
            legacy_confined(root)?;
        }
        let entries = match std::fs::read_dir(root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        for dirent in entries {
            let dirent = dirent?;
            let name = dirent.file_name();
            let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".json")) else {
                continue;
            };
            if !stem.starts_with(prefix) {
                continue;
            }
            let entry = read_entry(root, stem)?;
            if index == 0
                && entry
                    .as_ref()
                    .is_some_and(|e| e.repo_identity != repo.identity())
            {
                bail!("primary task index entry belongs to another repository");
            }
            if let Some(entry) = entry
                && entry.repo_identity == repo.identity()
                && !out.iter().any(|old| old.task_id == entry.task_id)
            {
                out.push(entry);
            }
        }
    }
    out.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn repo() -> (tempfile::TempDir, crate::git::Repo) {
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        let repo = crate::git::discover(dir.path()).unwrap();
        (dir, repo)
    }
    #[test]
    fn primary_index_is_scoped_private_and_refuses_redirects() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let (_dir, repo) = repo();
        let root = root_for(&repo).unwrap();
        let id = "018f1a2b-3c4d-7e5f-8a9b-0c1d2e3f4a5b";
        register(&repo.identity(), id, &repo.root, StoreKind::Headless).unwrap();
        let entry = read_entry(&root, id).unwrap().unwrap();
        assert_eq!(entry.schema_version, 2);
        assert_eq!(entry.repo_identity, repo.identity());
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(entry_path(&root, id))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let (_other_dir, other) = super::tests::repo();
        assert!(
            read_entry(&root_for(&other).unwrap(), id)
                .unwrap()
                .is_none()
        );
        let path = entry_path(&root, id);
        std::fs::write(&path, b"{}").unwrap();
        assert!(read_entry(&root, id).is_err());
        std::fs::remove_file(&path).unwrap();
        symlink("/dev/null", &path).unwrap();
        assert!(read_entry(&root, id).is_err());
        assert!(register(&repo.identity(), id, &repo.root, StoreKind::Headless).is_err());
    }

    #[test]
    fn old_entries_remain_readable_and_unknown_versions_are_refused() {
        let (_dir, repo) = repo();
        let root = root_for(&repo).unwrap();
        confined(&root, true).unwrap();
        let id = "018f1a2b-3c4d-7e5f-8a9b-0c1d2e3f4a5b";
        let mut entry = Entry {
            schema_version: 1,
            task_id: id.into(),
            repo_identity: repo.identity(),
            checkout: repo.root,
            store: StoreKind::Headless,
        };
        let path = entry_path(&root, id);
        state::write_json(&path, &entry).unwrap();
        let original = std::fs::read(&path).unwrap();
        assert_eq!(read_entry(&root, id).unwrap(), Some(entry.clone()));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        entry.schema_version = 99;
        state::write_json(&path, &entry).unwrap();
        assert!(
            read_entry(&root, id)
                .unwrap_err()
                .to_string()
                .contains("schema version")
        );
        entry.schema_version = 2;
        entry.task_id = "another".into();
        state::write_json(&path, &entry).unwrap();
        assert!(
            read_entry(&root, id)
                .unwrap_err()
                .to_string()
                .contains("different task")
        );
    }
}

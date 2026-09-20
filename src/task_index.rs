//! The cross-checkout task index: pointers, not records.
//!
//! The index holds one small entry per task — its id, the repository identity
//! it belongs to, the checkout it lives in, and which store kind holds it —
//! and nothing else. It is user state that lives outside every repository
//! checkout, so it is never migrated in place and never rewritten to match
//! records it cannot read. An entry whose checkout has disappeared degrades
//! into a lead about where a task used to live, not an authority about where
//! one is now.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::util::{Error, Result};
use crate::{bail, state, task};

pub const INDEX_SCHEMA_VERSION: u32 = 1;

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

/// Resolve existing ancestors first, then refuse redirected index components.
/// The caller-selected root may be outside the default home, but never in Git.
pub fn index_root() -> Result<PathBuf> {
    let raw = std::env::var_os("AHU_TASK_INDEX_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state/ahu/task-index"))
        })
        .ok_or_else(|| {
            Error::new("set AHU_TASK_INDEX_DIR to a private directory outside repositories")
        })?;
    if !raw.is_absolute()
        || raw
            .components()
            .any(|p| matches!(p, std::path::Component::ParentDir))
    {
        bail!("task index root must be an absolute path without '..'");
    }
    let mut existing = raw.as_path();
    let mut tail = Vec::new();
    while !existing.exists() {
        if std::fs::symlink_metadata(existing).is_ok() {
            bail!("task index root contains a dangling symlink");
        }
        tail.push(
            existing
                .file_name()
                .ok_or_else(|| Error::new("invalid task index root"))?
                .to_os_string(),
        );
        existing = existing
            .parent()
            .ok_or_else(|| Error::new("invalid task index root"))?;
    }
    let mut root = existing.canonicalize()?;
    for component in tail.into_iter().rev() {
        root.push(component);
    }
    if root.ancestors().any(|p| p.join(".git").exists()) {
        bail!("task index must be outside every repository checkout");
    }
    Ok(root)
}

fn confined(path: &Path, create: bool) -> Result<()> {
    let root = index_root()?;
    if !path.starts_with(&root) {
        bail!("task index path is outside the task index root");
    }
    let mut cursor = root.clone();
    if let Ok(meta) = std::fs::symlink_metadata(&root) {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions.
        if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            bail!(
                "existing task index root must be an owner-only directory owned by the current user"
            );
        }
    }
    if create && !cursor.exists() {
        state::create_private_dir_all(&cursor)?;
    }
    for part in path
        .strip_prefix(&root)
        .map_err(|e| Error::new(e.to_string()))?
        .components()
    {
        if !matches!(part, std::path::Component::Normal(_)) {
            bail!("invalid task index component");
        }
        cursor.push(part);
        match std::fs::symlink_metadata(&cursor) {
            Ok(m) if m.file_type().is_symlink() || !m.is_dir() => bail!(
                "task index directory is redirected or not a directory: {}",
                cursor.display()
            ),
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                state::create_private_dir_all(&cursor)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
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
    let temp = parent.join(format!(".write-{}", crate::orchestration::new_nonce()?));
    let mut file = state::create_new_private_file(&temp)?;
    let result = (|| -> Result<()> {
        serde_json::to_writer(&mut file, value).map_err(|e| Error::new(e.to_string()))?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        std::fs::rename(&temp, path)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

fn read_entry(root: &Path, task_id: &str) -> Result<Option<Entry>> {
    let path = entry_path(root, task_id);
    if state::confine_file(&path)?.is_none() {
        return Ok(None);
    }
    let bytes = state::read_private_file(&path)?;
    let entry: Entry = serde_json::from_slice(&bytes)
        .map_err(|e| Error::new(format!("invalid {}: {e}", path.display())))?;
    if entry.schema_version != INDEX_SCHEMA_VERSION {
        bail!(
            "task index entry {} has schema version {}; this ahu expects {}",
            path.display(),
            entry.schema_version,
            INDEX_SCHEMA_VERSION
        );
    }
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
    durable_write(
        &entry_path(&index_root()?, task_id),
        &Entry {
            schema_version: INDEX_SCHEMA_VERSION,
            task_id: task_id.to_string(),
            repo_identity: repo_identity.to_string(),
            checkout: checkout.to_path_buf(),
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
    let root = index_root()?;
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
    let root = index_root()?;
    confined(&root, false)?;
    read_entry(&root, task_id)
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
    let root = index_root()?;
    confined(&root, false)?;
    let mut out = Vec::new();
    match std::fs::read_dir(&root) {
        Ok(entries) => {
            for dirent in entries {
                let dirent = dirent?;
                let name = dirent.file_name();
                let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".json")) else {
                    continue;
                };
                if !stem.starts_with(prefix) {
                    continue;
                }
                match read_entry(&root, stem) {
                    Ok(Some(entry)) => out.push(entry),
                    Ok(None) => continue,
                    Err(e) => return Err(e),
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    }
    out.sort_by(|a, b| a.task_id.cmp(&b.task_id));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tempfile` creates world-readable directories, and the index root must
    /// be owner-only, so scenario roots are tightened before use.
    fn owner_only_dir(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn index_scenarios() {
        let id = "018f1a2b-3c4d-7e5f-8a9b-0c1d2e3f4a5b";
        let id2 = "018f1a2b-3c4d-7e5f-8a9b-0c1d2e3f4a5c";
        let id3 = "018f1a2b-3c4d-7e5f-8a9b-0c1d2e3f4a5d";
        let stranger = "018f1a2b-3c4d-7e5f-8a9b-0c1d2e3f4a5e";
        let repo_identity = "0123456789abcdef";
        let checkout = std::env::temp_dir().join("ahu-task-index-checkout");
        let common_prefix: String = id.chars().take(35).collect();

        // Registration and lookup.
        {
            let root = tempfile::tempdir().unwrap();
            owner_only_dir(root.path());
            unsafe { std::env::set_var("AHU_TASK_INDEX_DIR", root.path()) };
            register(repo_identity, id, &checkout, StoreKind::Worktree).unwrap();
            let entry = lookup(id).unwrap().unwrap();
            assert_eq!(entry.task_id, id);
            assert_eq!(entry.repo_identity, repo_identity);
            assert_eq!(entry.checkout, checkout);
            assert_eq!(entry.store, StoreKind::Worktree);
            assert_eq!(entry.schema_version, INDEX_SCHEMA_VERSION);

            // Grammar, uppercase, and legacy forms are not looked up.
            assert!(lookup(&format!("ahu:task:{id}")).unwrap().is_none());
            assert!(
                lookup("018F1A2B-3C4D-7E5F-8A9B-0C1D2E3F4A5B")
                    .unwrap()
                    .is_none()
            );
            assert!(lookup("006aaec1fb360df8a1").unwrap().is_none());

            // Removing a legacy id is a no-op; registering one is refused.
            assert!(remove("006aaec1fb360df8a1").is_ok());
            let refused = register(
                repo_identity,
                "006aaec1fb360df8a1",
                &checkout,
                StoreKind::Worktree,
            )
            .unwrap_err();
            assert!(refused.to_string().contains("canonical task ids only"));
            let relative = std::path::PathBuf::from("relative/checkout");
            let refused = register(repo_identity, id, &relative, StoreKind::Worktree).unwrap_err();
            assert!(refused.to_string().contains("absolute checkout paths only"));
        }

        // Prefix lookup over two entries with different stores.
        {
            let root = tempfile::tempdir().unwrap();
            owner_only_dir(root.path());
            unsafe { std::env::set_var("AHU_TASK_INDEX_DIR", root.path()) };
            register(repo_identity, id, &checkout, StoreKind::Worktree).unwrap();
            register(repo_identity, id2, &checkout, StoreKind::Headless).unwrap();
            let entries = lookup_prefix(&common_prefix).unwrap();
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].task_id, id);
            assert_eq!(entries[1].task_id, id2);
            assert_eq!(entries[0].store, StoreKind::Worktree);
            assert_eq!(entries[1].store, StoreKind::Headless);
            assert_eq!(lookup_prefix("018f1a2b").unwrap().len(), 2);
            assert!(lookup_prefix("ffffffff").unwrap().is_empty());
            assert!(lookup_prefix("").unwrap().is_empty());
            assert!(lookup_prefix("not-a-prefix!").unwrap().is_empty());
        }

        // Removal forgets an entry and tolerates a missing one.
        {
            let root = tempfile::tempdir().unwrap();
            owner_only_dir(root.path());
            unsafe { std::env::set_var("AHU_TASK_INDEX_DIR", root.path()) };
            register(repo_identity, id, &checkout, StoreKind::Worktree).unwrap();
            register(repo_identity, id2, &checkout, StoreKind::Headless).unwrap();
            remove(id2).unwrap();
            assert!(lookup(id2).unwrap().is_none());
            assert_eq!(lookup_prefix("018f1a2b").unwrap().len(), 1);
            remove(id2).unwrap();
        }

        // Corrupt entries fail closed.
        {
            let root = tempfile::tempdir().unwrap();
            owner_only_dir(root.path());
            unsafe { std::env::set_var("AHU_TASK_INDEX_DIR", root.path()) };
            let path = root.path().join(format!("{id3}.json"));
            std::fs::write(
                &path,
                format!(
                    "{{\"schema_version\":99,\"task_id\":\"{id3}\",\
                     \"repo_identity\":\"{repo_identity}\",\
                     \"checkout\":\"/tmp\",\"store\":\"worktree\"}}\n"
                ),
            )
            .unwrap();
            assert!(
                lookup(id3)
                    .unwrap_err()
                    .to_string()
                    .contains("schema version")
            );
            std::fs::write(
                &path,
                format!(
                    "{{\"schema_version\":1,\"task_id\":\"{stranger}\",\
                     \"repo_identity\":\"{repo_identity}\",\
                     \"checkout\":\"/tmp\",\"store\":\"worktree\"}}\n"
                ),
            )
            .unwrap();
            assert!(
                lookup(id3)
                    .unwrap_err()
                    .to_string()
                    .contains("different task")
            );
            std::fs::write(&path, "not json\n").unwrap();
            assert!(lookup(id3).unwrap_err().to_string().contains("invalid"));
        }

        // The root must stay outside repositories.
        {
            let base = tempfile::tempdir().unwrap();
            std::fs::create_dir(base.path().join(".git")).unwrap();
            unsafe { std::env::set_var("AHU_TASK_INDEX_DIR", base.path().join("idx")) };
            let refused = register(repo_identity, id, &checkout, StoreKind::Worktree).unwrap_err();
            assert!(
                refused
                    .to_string()
                    .contains("outside every repository checkout")
            );
            unsafe { std::env::remove_var("AHU_TASK_INDEX_DIR") };
        }

        // The root must stay owner-only and owned by the current user.
        {
            let base = tempfile::tempdir().unwrap();
            std::fs::create_dir(base.path().join("idx")).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(base.path().join("idx"))
                    .unwrap()
                    .permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(base.path().join("idx"), perms).unwrap();
            }
            unsafe { std::env::set_var("AHU_TASK_INDEX_DIR", base.path().join("idx")) };
            let refused = register(repo_identity, id, &checkout, StoreKind::Worktree).unwrap_err();
            assert!(refused.to_string().contains("owner-only"));
            unsafe { std::env::remove_var("AHU_TASK_INDEX_DIR") };
        }
    }
}

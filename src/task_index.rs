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
    pub(super) fn repo() -> (tempfile::TempDir, crate::git::Repo) {
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

#[cfg(test)]
mod registration_tests {
    use super::tests::repo;
    use super::*;

    const ID: &str = "018f1a2b-3c4d-7e5f-8a9b-0c1d2e3f4a5b";
    const SIBLING: &str = "018f1a2b-3c4d-7e5f-8a9b-0c1d2e3f4a5c";
    const OTHER: &str = "018f1a2b-9999-7e5f-8a9b-0c1d2e3f4a5b";

    #[test]
    fn registration_refuses_uncanonical_ids_relative_paths_and_foreign_identities() {
        let (_dir, repo) = repo();
        let identity = repo.identity();
        for bad in ["", "018F1A2B-3C4D-7E5F-8A9B-0C1D2E3F4A5B", "018f1a2b"] {
            let error = register(&identity, bad, &repo.root, StoreKind::Worktree)
                .expect_err("an uncanonical id is refused")
                .to_string();
            assert!(error.contains("canonical task ids only"), "{error}");
        }
        let error = register(
            &identity,
            ID,
            Path::new("relative/checkout"),
            StoreKind::Worktree,
        )
        .expect_err("a relative checkout is refused")
        .to_string();
        assert!(error.contains("absolute checkout paths only"), "{error}");
        let error = register(&"0".repeat(16), ID, &repo.root, StoreKind::Worktree)
            .expect_err("a foreign identity is refused")
            .to_string();
        assert!(error.contains("identity mismatch"), "{error}");
        // None of the refusals may leave a pointer behind.
        assert!(!entry_path(&root_for(&repo).unwrap(), ID).exists());
    }

    #[test]
    fn worktree_registration_records_the_submitted_checkout() {
        let (_dir, repo) = repo();
        register(&repo.identity(), ID, &repo.root, StoreKind::Worktree).unwrap();
        let entry = lookup_in(&repo, ID).unwrap().unwrap();
        assert_eq!(entry.store, StoreKind::Worktree);
        assert_eq!(entry.checkout, repo.root);
        assert_eq!(entry.schema_version, INDEX_SCHEMA_VERSION);
        assert_eq!(
            serde_json::to_value(StoreKind::Worktree).unwrap(),
            serde_json::json!("worktree")
        );
        assert_eq!(
            serde_json::to_value(StoreKind::Headless).unwrap(),
            serde_json::json!("headless")
        );
    }

    #[test]
    fn removal_is_idempotent_and_ignores_uncanonical_ids() {
        let (_dir, repo) = repo();
        register(&repo.identity(), ID, &repo.root, StoreKind::Headless).unwrap();
        let path = entry_path(&root_for(&repo).unwrap(), ID);
        assert!(path.exists());
        remove_in(&repo, ID).unwrap();
        assert!(!path.exists());
        // Removing a forgotten task, or one that was never canonical, succeeds
        // without recreating the index or reporting a missing file.
        remove_in(&repo, ID).unwrap();
        remove_in(&repo, "not-a-task").unwrap();
        assert_eq!(lookup_in(&repo, ID).unwrap(), None);
    }

    #[test]
    fn lookup_refuses_an_entry_that_names_another_repository() {
        let (_dir, repo) = repo();
        let (_other_dir, other) = super::tests::repo();
        assert_eq!(lookup_in(&repo, "not-a-task").unwrap(), None);
        assert_eq!(lookup_in(&repo, ID).unwrap(), None);
        let root = root_for(&repo).unwrap();
        confined(&root, true).unwrap();
        state::write_json(
            &entry_path(&root, ID),
            &Entry {
                schema_version: INDEX_SCHEMA_VERSION,
                task_id: ID.into(),
                repo_identity: other.identity(),
                checkout: repo.root.clone(),
                store: StoreKind::Headless,
            },
        )
        .unwrap();
        let error = lookup_in(&repo, ID)
            .expect_err("a foreign primary entry is refused")
            .to_string();
        assert!(error.contains("belongs to another repository"), "{error}");
        let error = lookup_prefix_in(&repo, "018f1a2b")
            .expect_err("a foreign primary entry is refused by prefix search too")
            .to_string();
        assert!(error.contains("belongs to another repository"), "{error}");
    }

    #[test]
    fn prefix_search_rejects_unusable_prefixes() {
        let (_dir, repo) = repo();
        for bad in ["", "018F1A2B", "018f1a2b!", "z", &"0".repeat(37)] {
            assert_eq!(
                lookup_prefix_in(&repo, bad).unwrap(),
                Vec::new(),
                "prefix {bad:?} must not be searched"
            );
        }
    }

    #[test]
    fn prefix_search_returns_sorted_matches_and_skips_unrelated_files() {
        let (_dir, repo) = repo();
        let identity = repo.identity();
        register(&identity, SIBLING, &repo.root, StoreKind::Worktree).unwrap();
        register(&identity, ID, &repo.root, StoreKind::Headless).unwrap();
        register(&identity, OTHER, &repo.root, StoreKind::Headless).unwrap();
        let root = root_for(&repo).unwrap();
        state::write_private_file(&root.join("README"), b"not an entry\n").unwrap();
        let ids: Vec<String> = lookup_prefix_in(&repo, "018f1a2b-3c4d")
            .unwrap()
            .into_iter()
            .map(|entry| entry.task_id)
            .collect();
        assert_eq!(ids, [ID, SIBLING]);
        let all: Vec<String> = lookup_prefix_in(&repo, "018f1a2b")
            .unwrap()
            .into_iter()
            .map(|entry| entry.task_id)
            .collect();
        assert_eq!(all, [ID, SIBLING, OTHER]);
        assert_eq!(lookup_prefix_in(&repo, "018f1a2c").unwrap(), Vec::new());
    }

    #[test]
    fn ambient_helpers_refuse_uncanonical_input_without_reading_the_index() {
        // These take their scope from the working directory, so the only
        // behaviour that can be checked without moving it is the early refusal.
        assert_eq!(lookup("not-a-task").unwrap(), None);
        assert_eq!(lookup_prefix("NOT-A-PREFIX").unwrap(), Vec::new());
        remove("not-a-task").unwrap();
        let cwd = std::env::current_dir().unwrap();
        let repo = crate::git::discover(&cwd).unwrap();
        assert_eq!(index_root().unwrap(), root_for(&repo).unwrap());
    }

    #[test]
    fn an_oversized_or_multiply_linked_entry_is_refused() {
        let (_dir, repo) = repo();
        let root = root_for(&repo).unwrap();
        confined(&root, true).unwrap();
        let path = entry_path(&root, ID);
        state::write_private_file(&path, &vec![b'{'; 65537]).unwrap();
        let error = read_entry(&root, ID)
            .expect_err("an entry larger than 64 KiB is refused")
            .to_string();
        assert!(error.contains("within 64 KiB"), "{error}");
        std::fs::remove_file(&path).unwrap();
        register(&repo.identity(), ID, &repo.root, StoreKind::Headless).unwrap();
        // A second link to an entry means another name can replace what this
        // index believes it owns, so the pointer is not read through either name.
        std::fs::hard_link(&path, entry_path(&root, SIBLING)).unwrap();
        for id in [ID, SIBLING] {
            let error = read_entry(&root, id)
                .expect_err("a multiply linked entry is refused")
                .to_string();
            assert!(error.contains("within 64 KiB"), "{error}");
        }
    }

    #[test]
    fn ambient_lookups_answer_for_the_repository_of_the_working_directory() {
        // Read-only, and deliberately compared against the explicit form: the
        // ambient repository here is ahu's own checkout, and a 2017-era UUID
        // cannot collide with a time-ordered id this build would mint.
        let cwd = std::env::current_dir().unwrap();
        let repo = crate::git::discover(&cwd).unwrap();
        assert_eq!(lookup(ID).unwrap(), lookup_in(&repo, ID).unwrap());
        assert_eq!(lookup(ID).unwrap(), None);
        let found = lookup_prefix("018f1a2b").unwrap();
        assert_eq!(found, lookup_prefix_in(&repo, "018f1a2b").unwrap());
        assert!(
            found
                .iter()
                .all(|entry| entry.task_id.starts_with("018f1a2b")
                    && entry.repo_identity == repo.identity()),
            "{found:?}"
        );
    }

    #[test]
    fn the_index_root_must_be_the_selected_primary_task_index() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, repo) = repo();
        let error = confined(Path::new("/"), false)
            .expect_err("a path with no repository above it is refused")
            .to_string();
        assert!(error.contains("invalid task index root"), "{error}");
        let sibling = root_for(&repo).unwrap().with_file_name("other-index");
        let error = confined(&sibling, false)
            .expect_err("another directory in the state root is not the index")
            .to_string();
        assert!(
            error.contains("must belong to the selected primary checkout"),
            "{error}"
        );
        let root = root_for(&repo).unwrap();
        confined(&root, true).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        let error = confined(&root, false)
            .expect_err("a group- or world-readable index is refused")
            .to_string();
        assert!(error.contains("owner-only directory"), "{error}");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        confined(&root, false).unwrap();
    }
}

#[cfg(test)]
mod legacy_tests {
    use super::tests::repo;
    use super::*;

    const ID: &str = "018f1a2b-3c4d-7e5f-8a9b-0c1d2e3f4a5b";

    /// Point a repository at an external index root it does not own, the way a
    /// migrated checkout does, and return the canonical root entries live in.
    fn configured_legacy_root(repo: &crate::git::Repo) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let declared = dir.path().join("task-index");
        state::create_private_dir_all(&declared).unwrap();
        let lookup = crate::storage::RepositoryStorage::new(repo)
            .unwrap()
            .primary
            .state_root()
            .unwrap()
            .join("legacy-lookup.json");
        state::write_private_file(
            &lookup,
            serde_json::to_string(&serde_json::json!({
                "schema_version": 1,
                "index_roots": [declared],
            }))
            .unwrap()
            .as_bytes(),
        )
        .unwrap();
        let canonical = crate::storage::external_root(&declared).unwrap();
        (dir, canonical)
    }

    fn legacy_entry(root: &Path, repo: &crate::git::Repo, task_id: &str) {
        state::write_json(
            &entry_path(root, task_id),
            &Entry {
                schema_version: 1,
                task_id: task_id.into(),
                repo_identity: repo.identity(),
                checkout: repo.root.clone(),
                store: StoreKind::Worktree,
            },
        )
        .unwrap();
    }

    #[test]
    fn legacy_entries_are_found_and_the_primary_index_wins() {
        let (_dir, repo) = repo();
        let (_legacy_dir, legacy) = configured_legacy_root(&repo);
        legacy_entry(&legacy, &repo, ID);
        let entry = lookup_in(&repo, ID).unwrap().expect("the legacy pointer");
        assert_eq!(entry.schema_version, 1);
        assert_eq!(entry.store, StoreKind::Worktree);
        assert_eq!(lookup_prefix_in(&repo, "018f1a2b").unwrap(), vec![entry]);
        // Registering the same task locally must not produce two pointers, and
        // the primary record is the one that answers.
        register(&repo.identity(), ID, &repo.root, StoreKind::Headless).unwrap();
        let entry = lookup_in(&repo, ID).unwrap().unwrap();
        assert_eq!(entry.schema_version, INDEX_SCHEMA_VERSION);
        assert_eq!(entry.store, StoreKind::Headless);
        let found = lookup_prefix_in(&repo, "018f1a2b").unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].schema_version, INDEX_SCHEMA_VERSION);
    }

    #[test]
    fn a_legacy_entry_for_another_repository_is_ignored_rather_than_returned() {
        let (_dir, repo) = repo();
        let (_other_dir, other) = super::tests::repo();
        let (_legacy_dir, legacy) = configured_legacy_root(&repo);
        legacy_entry(&legacy, &other, ID);
        assert_eq!(lookup_in(&repo, ID).unwrap(), None);
        assert_eq!(lookup_prefix_in(&repo, "018f1a2b").unwrap(), Vec::new());
    }

    #[test]
    fn a_shared_or_redirected_legacy_root_is_refused() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let (_dir, repo) = repo();
        let (_legacy_dir, legacy) = configured_legacy_root(&repo);
        legacy_entry(&legacy, &repo, ID);
        // A root that never existed is not an error: nothing was migrated.
        legacy_confined(&legacy.join("missing")).unwrap();
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o755)).unwrap();
        let error = legacy_confined(&legacy)
            .expect_err("a group-readable legacy root is refused")
            .to_string();
        assert!(error.contains("owner-only directory"), "{error}");
        let error = lookup_in(&repo, ID)
            .expect_err("lookup refuses the shared legacy root")
            .to_string();
        assert!(error.contains("owner-only directory"), "{error}");
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(lookup_in(&repo, ID).unwrap().is_some());
        let alias = legacy.with_file_name("alias");
        symlink(&legacy, &alias).unwrap();
        let error = legacy_confined(&alias)
            .expect_err("a redirected legacy root is refused")
            .to_string();
        assert!(error.contains("redirected"), "{error}");
    }

    #[test]
    fn a_legacy_root_reached_through_a_symlinked_parent_is_refused() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        state::create_private_dir_all(&real.join("task-index")).unwrap();
        symlink(&real, dir.path().join("alias")).unwrap();
        // The leaf is a real directory; the path that reaches it is not, so the
        // configured root and the root actually opened would differ.
        let error = legacy_confined(&dir.path().join("alias/task-index"))
            .expect_err("a legacy root reached through a link is refused")
            .to_string();
        assert!(error.contains("legacy index root is redirected"), "{error}");
    }
}

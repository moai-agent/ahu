//! Storage ownership and path construction; confinement stays with each backend.

use crate::util::Result;
use std::path::{Path, PathBuf};

/// Paths owned by one checkout, independent of the process working directory.
#[derive(Debug, Clone)]
pub struct CheckoutStorage {
    root: PathBuf,
}

impl CheckoutStorage {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn configuration(&self) -> crate::config::ProjectFiles {
        crate::config::ProjectFiles::new(&self.root)
    }
    pub fn state_root(&self) -> Result<PathBuf> {
        crate::state::checkout_root(&self.root)
    }
    pub fn repo_dir(&self, identity: &str) -> Result<PathBuf> {
        Ok(self.state_root()?.join("repos").join(identity))
    }
    pub fn tasks_dir(&self, identity: &str) -> Result<PathBuf> {
        Ok(self.repo_dir(identity)?.join("tasks"))
    }
    /// Plan-time location: the checkout need not exist yet. Validate at I/O time.
    pub fn task_dir(&self, identity: &str, task_id: &str) -> PathBuf {
        self.root
            .join(".ahu/state/repos")
            .join(identity)
            .join("tasks")
            .join(task_id)
    }
}

/// Checkout-local records and repository-wide coordination have distinct owners.
#[derive(Debug, Clone)]
pub struct RepositoryStorage {
    pub checkout: CheckoutStorage,
    pub primary: CheckoutStorage,
    pub identity: String,
}

impl RepositoryStorage {
    pub fn new(repo: &crate::git::Repo) -> Result<Self> {
        Ok(Self {
            checkout: CheckoutStorage::new(&repo.root),
            primary: CheckoutStorage::new(&repo.primary_root()?),
            identity: repo.identity(),
        })
    }
    pub fn coordination_dir(&self) -> Result<PathBuf> {
        self.primary.repo_dir(&self.identity)
    }
    pub fn worktrees_root(&self) -> PathBuf {
        self.primary.root.join(crate::state::WORKTREES_DIR)
    }
    /// Managed worktrees are scanned with task ownership checks, never as legacy stores.
    pub fn legacy_task_stores(&self) -> Result<Vec<PathBuf>> {
        let managed = self
            .checkout
            .root
            .canonicalize()
            .ok()
            .zip(self.worktrees_root().canonicalize().ok())
            .is_some_and(|(root, worktrees)| root.parent() == Some(worktrees.as_path()));
        let mut stores = Vec::new();
        if !managed {
            stores.push(self.checkout.tasks_dir(&self.identity)?);
        }
        let primary = self.primary.tasks_dir(&self.identity)?;
        if !stores.contains(&primary) {
            stores.push(primary);
        }
        Ok(stores)
    }
}

/// Record paths within an already selected task store, interactive or external.
/// Task ownership and backend confinement are validated before accessing them.
pub struct TaskStorage<'a> {
    directory: &'a Path,
}
impl<'a> TaskStorage<'a> {
    pub fn new(directory: &'a Path) -> Self {
        Self { directory }
    }
    pub fn record(&self) -> PathBuf {
        self.directory.join(crate::task::TASK_FILE)
    }
    pub fn prompt(&self) -> PathBuf {
        self.directory.join(crate::task::PROMPT_FILE)
    }
}

/// The primary checkout owns coordination; native data is never stored here.
#[derive(Debug, Clone)]
pub struct HeadlessStore {
    pub directory: PathBuf,
}
impl HeadlessStore {
    pub fn for_repo(repo: &crate::git::Repo) -> Result<Self> {
        let primary = repo.primary_root()?;
        let verified = verified_primary(&primary)?;
        if verified.identity != repo.identity() {
            crate::bail!("headless coordination repository identity changed");
        }
        Self::from_verified(&primary, &verified)
    }

    fn from_verified(primary: &Path, verified: &VerifiedPrimary) -> Result<Self> {
        Ok(Self {
            directory: crate::state::checkout_root(primary)?
                .join("repos")
                .join(&verified.identity)
                .join("headless"),
        })
    }

    /// Positively identify a primary-owned store before allowing repository I/O.
    pub fn containing(path: &Path) -> Result<Option<Self>> {
        for ancestor in path.ancestors() {
            if ancestor.file_name().is_none_or(|n| n != "headless") {
                continue;
            }
            let Some(identity) = ancestor.parent() else {
                continue;
            };
            let Some(repos) = identity.parent() else {
                continue;
            };
            let Some(state) = repos.parent() else {
                continue;
            };
            let Some(ahu) = state.parent() else { continue };
            let Some(primary) = ahu.parent() else {
                continue;
            };
            if repos.file_name().is_none_or(|n| n != "repos")
                || state.file_name().is_none_or(|n| n != "state")
                || ahu.file_name().is_none_or(|n| n != ".ahu")
            {
                continue;
            }
            let verified = verified_primary(primary)?;
            let expected = Self::from_verified(primary, &verified)?;
            if expected.directory != ancestor {
                crate::bail!("headless coordination repository identity mismatch");
            }
            crate::state::confine_existing_dir(path)?;
            return Ok(Some(expected));
        }
        Ok(None)
    }
}

// Verification is process-local evidence, not a persisted marker. Cache a
// positively Git-verified primary and invalidate it when its Git locator or
// ownership-defining metadata changes. Normal record writes require only fresh
// filesystem checks, not subprocesses. This is not a sandbox against the owner
// concurrently rewriting their own Git configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    device: u64,
    inode: u64,
    mode: u32,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl FileStamp {
    fn from(meta: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            device: meta.dev(),
            inode: meta.ino(),
            mode: meta.mode(),
            size: meta.len(),
            modified: (meta.mtime(), meta.mtime_nsec()),
            changed: (meta.ctime(), meta.ctime_nsec()),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct WatchedPath {
    path: PathBuf,
    stamp: Option<(FileStamp, PathBuf, FileStamp)>,
}
impl WatchedPath {
    fn read(path: PathBuf) -> Result<Self> {
        let stamp = match std::fs::symlink_metadata(&path) {
            Ok(meta) => Some((
                FileStamp::from(&meta),
                path.canonicalize()?,
                FileStamp::from(&std::fs::metadata(&path)?),
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        Ok(Self { path, stamp })
    }
    fn unchanged(&self) -> bool {
        Self::read(self.path.clone()).is_ok_and(|current| current == *self)
    }
}
#[derive(Debug)]
struct VerifiedPrimary {
    identity: String,
    watched: Vec<WatchedPath>,
}

fn verified_primary(primary: &Path) -> Result<std::sync::Arc<VerifiedPrimary>> {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex, OnceLock};
    static VERIFIED: OnceLock<Mutex<BTreeMap<PathBuf, Arc<VerifiedPrimary>>>> = OnceLock::new();
    let mut cache = VERIFIED
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| crate::util::Error::new("storage ownership verification unavailable"))?;
    if let Some(verified) = cache.get(primary)
        && verified.watched.iter().all(WatchedPath::unchanged)
    {
        return Ok(verified.clone());
    }
    cache.remove(primary);
    let marker = primary.join(".git");
    let before = WatchedPath::read(marker.clone())?;
    let marker_metadata = std::fs::symlink_metadata(&marker).map_err(|e| crate::util::Error::new(format!(
        "cannot verify the primary checkout's Git marker: {e}; Git must identify a primary checkout with its own .git marker. Separate Git directory layouts whose primary cannot be located are unsupported for coordination"
    )))?;
    if marker_metadata.file_type().is_symlink() {
        crate::bail!("primary Git marker must not be a symlink");
    }
    let repo = crate::git::discover(primary)?;
    if repo.root != primary || repo.primary_root()? != primary {
        crate::bail!("headless coordination must belong to the repository's primary checkout");
    }
    if !before.unchanged() {
        crate::bail!("Git ownership changed during storage verification");
    }
    let mut watched = vec![before, WatchedPath::read(primary.to_path_buf())?];
    for name in [
        "",
        "HEAD",
        "config",
        "commondir",
        "gitdir",
        "objects",
        "refs",
    ] {
        watched.push(WatchedPath::read(repo.common_dir.join(name))?);
    }
    let verified = Arc::new(VerifiedPrimary {
        identity: repo.identity(),
        watched,
    });
    // A process needs only a bounded working set; eviction means revalidation.
    if cache.len() >= 64 {
        cache.clear();
    }
    cache.insert(primary.to_path_buf(), verified.clone());
    Ok(verified)
}

/// Validate the opened object, not just the pathname inspected before open.
/// Legacy metadata may be owner-readable with public read bits; locks and new
/// coordination metadata must be owner-only. Neither accepts shared writers.
pub(crate) fn validate_owned_metadata(meta: &std::fs::Metadata, private: bool) -> Result<()> {
    // SAFETY: geteuid has no preconditions.
    validate_metadata_owner(meta, private, unsafe { libc::geteuid() })
}
fn validate_metadata_owner(meta: &std::fs::Metadata, private: bool, uid: u32) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    if !meta.is_file()
        || meta.uid() != uid
        || meta.nlink() != 1
        || meta.mode() & (if private { 0o7077 } else { 0o7022 }) != 0
    {
        crate::bail!(
            "metadata must be a singly linked regular file owned by the current user with safe permissions"
        );
    }
    Ok(())
}

/// Read task records with descriptor checks even during legacy owner discovery,
/// where recursing through the headless resolver would need this very record.
pub(crate) fn read_task_metadata(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    crate::state::confine_file(path)?;
    let private = HeadlessStore::containing(
        path.parent()
            .ok_or_else(|| crate::util::Error::new("missing metadata parent"))?,
    )?
    .is_some();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    validate_owned_metadata(&file.metadata()?, private)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyLookup {
    schema_version: u32,
    #[serde(default)]
    runtime_roots: Vec<PathBuf>,
    #[serde(default)]
    index_roots: Vec<PathBuf>,
}

/// Explicit old stores are references only. Discovery never migrates their data.
pub fn legacy_runtime_roots(repo: &crate::git::Repo) -> Result<Vec<PathBuf>> {
    legacy_roots(repo, false)
}

pub fn legacy_index_roots(repo: &crate::git::Repo) -> Result<Vec<PathBuf>> {
    legacy_roots(repo, true)
}

fn legacy_roots(repo: &crate::git::Repo, index: bool) -> Result<Vec<PathBuf>> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(if index {
            ".local/state/ahu/task-index"
        } else {
            ".local/state/ahu/runtime"
        }));
    }
    let path = RepositoryStorage::new(repo)?
        .primary
        .state_root()?
        .join("legacy-lookup.json");
    if crate::state::confine_file(&path)?.is_some() {
        use std::io::Read;
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&path)?;
        let meta = file.metadata()?;
        validate_owned_metadata(&meta, true)?;
        if meta.len() > 65536 {
            crate::bail!("legacy lookup configuration exceeds 64 KiB");
        }
        let mut bytes = Vec::new();
        file.take(65537).read_to_end(&mut bytes)?;
        if bytes.len() > 65536 {
            crate::bail!("legacy lookup configuration exceeds 64 KiB");
        }
        let config: LegacyLookup = serde_json::from_slice(&bytes)?;
        if config.schema_version != 1 || config.runtime_roots.len() + config.index_roots.len() > 32
        {
            crate::bail!("unsupported legacy lookup configuration");
        }
        roots.extend(if index {
            config.index_roots
        } else {
            config.runtime_roots
        });
    }
    for root in &mut roots {
        *root = external_root(root)?;
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}

pub(crate) fn external_root(raw: &Path) -> Result<PathBuf> {
    use crate::{bail, util::Error};
    if !raw.is_absolute()
        || raw
            .components()
            .any(|p| matches!(p, std::path::Component::ParentDir))
    {
        bail!("legacy root must be an absolute path without '..'");
    }
    let mut existing = raw;
    let mut tail = Vec::new();
    while !existing.exists() {
        if std::fs::symlink_metadata(existing).is_ok() {
            bail!("legacy root contains a dangling symlink");
        }
        tail.push(
            existing
                .file_name()
                .ok_or_else(|| Error::new("invalid legacy root"))?
                .to_os_string(),
        );
        existing = existing
            .parent()
            .ok_or_else(|| Error::new("invalid legacy root"))?;
    }
    if std::fs::symlink_metadata(existing)?
        .file_type()
        .is_symlink()
    {
        bail!("legacy root is redirected");
    }
    let mut root = existing.canonicalize()?;
    for component in tail.into_iter().rev() {
        root.push(component);
    }
    if root.ancestors().any(|p| p.join(".git").exists()) {
        bail!("legacy runtime must remain outside repository checkouts");
    }
    Ok(root)
}

#[cfg(test)]
mod ownership_tests {
    use super::*;

    #[test]
    fn descriptor_owner_validation_refuses_another_uid() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("metadata.json");
        crate::state::write_private_file(&path, b"{}").unwrap();
        let metadata = std::fs::File::open(path).unwrap().metadata().unwrap();
        assert!(validate_metadata_owner(&metadata, true, metadata.uid()).is_ok());
        assert!(validate_metadata_owner(&metadata, true, metadata.uid().wrapping_add(1)).is_err());
    }

    #[test]
    fn verified_handle_reused_for_record_writes() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(dir.path())
                .status()
                .unwrap()
                .success()
        );
        let primary = dir.path().canonicalize().unwrap();
        let first = verified_primary(&primary).unwrap();
        let directory = primary
            .join(".ahu/state/repos")
            .join(&first.identity)
            .join("headless");
        crate::state::create_private_dir_all(&directory).unwrap();
        // Creating .ahu changes the checkout metadata once. Subsequent writes
        // beneath it reuse the same positive verification, without Git probes.
        let stable = verified_primary(&primary).unwrap();
        for n in 0..3 {
            crate::headless::durable_json(
                &directory.join(format!("claim-{n}.json")),
                &serde_json::json!({"state":"refused"}),
            )
            .unwrap();
            assert!(std::sync::Arc::ptr_eq(
                &stable,
                &verified_primary(&primary).unwrap()
            ));
        }
    }
}

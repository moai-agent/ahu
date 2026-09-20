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
        Ok(Self {
            directory: RepositoryStorage::new(repo)?
                .coordination_dir()?
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
            // A real .git directory identifies the primary checkout directly.
            // Linked checkouts have a .git file and take the Git-verified path.
            // Avoid spawning Git for every broker heartbeat/claim while retaining
            // fresh path and identity checks on every operation.
            let marker = primary.join(".git");
            let expected = if std::fs::symlink_metadata(&marker)
                .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
            {
                let common = marker.canonicalize()?;
                let identity = crate::util::digest_bytes(common.as_os_str().as_encoded_bytes());
                let directory = crate::state::checkout_root(primary)?
                    .join("repos")
                    .join(&identity[..16])
                    .join("headless");
                Self { directory }
            } else {
                let repo = crate::git::discover(primary)?;
                if repo.primary_root()? != primary {
                    crate::bail!(
                        "headless coordination must belong to the repository's primary checkout"
                    );
                }
                Self::for_repo(&repo)?
            };
            if expected.directory != ancestor {
                crate::bail!("headless coordination repository identity mismatch");
            }
            crate::state::confine_existing_dir(path)?;
            return Ok(Some(expected));
        }
        Ok(None)
    }
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
    if let Some(meta) = crate::state::confine_file(&path)? {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions.
        if meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o077 != 0
            || meta.nlink() != 1
            || meta.len() > 65536
        {
            crate::bail!(
                "legacy lookup configuration must be an owner-only regular file within 64 KiB"
            );
        }
        let config: LegacyLookup =
            serde_json::from_slice(&crate::state::read_private_file(&path)?)?;
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

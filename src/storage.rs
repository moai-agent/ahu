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

/// External runtime placement is independent of checkout-local coordination.
#[derive(Debug, Clone)]
pub struct RuntimeStorage {
    root: PathBuf,
}
impl RuntimeStorage {
    /// The runtime backend validates and resolves this root before construction.
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }
    pub fn repo_dir(&self, identity: &str) -> PathBuf {
        self.root.join(identity)
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

use super::service::Entry;
use super::v1::{Error, Result, VERSION};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;

pub(super) enum Store {
    /// Immutable observations of validated legacy records. Never a launch store;
    /// legacy IDs need not be UUIDs and no read creates durable service state.
    Inspection { entries: BTreeMap<String, Entry> },
    Memory {
        entries: Mutex<BTreeMap<String, Entry>>,
        effects: Mutex<BTreeMap<String, Arc<AtomicBool>>>,
    },
    Local {
        directory: PathBuf,
        repository: String,
    },
}
pub(super) fn storage(error: impl std::fmt::Display) -> Error {
    Error::Storage(error.to_string())
}
pub(super) fn task_id(id: &str) -> Result<()> {
    if crate::task::is_canonical_task_uuid(id) {
        Ok(())
    } else {
        Err(Error::InvalidRequest)
    }
}

impl Store {
    pub(super) fn inspection(
        repo: &crate::git::Repo,
        mut entries: BTreeMap<String, Entry>,
    ) -> Result<Self> {
        if entries.is_empty() {
            return Ok(Self::Inspection { entries });
        }
        let directory = crate::storage::RepositoryStorage::new(repo)
            .map_err(storage)?
            .coordination_dir()
            .map_err(storage)?
            .join("agent-tasks-v1");
        crate::state::confine_existing_dir(&directory).map_err(storage)?;
        match std::fs::symlink_metadata(&directory) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(storage(e)),
            Ok(_) => {
                check_directory(&directory, false)?;
                for (id, observed) in &mut entries {
                    if !crate::task::is_canonical_task_uuid(id) {
                        continue;
                    }
                    // Atomic publication permits a coherent read without creating
                    // lock files. No reconciliation, claims or receipts are written.
                    if let Some(entry) = read(&directory.join(format!("{id}.json")))? {
                        validate(&entry, id, &repo.identity())?;
                        let mut identity = observed.snapshot.identity.clone();
                        identity.provider = entry.snapshot.identity.provider.clone();
                        if identity != entry.snapshot.identity {
                            return Err(Error::Conflict);
                        }
                        *observed = entry;
                    }
                }
            }
        }
        Ok(Self::Inspection { entries })
    }

    pub fn memory() -> Self {
        Self::Memory {
            entries: Mutex::new(BTreeMap::new()),
            effects: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn local(repo: &crate::git::Repo) -> Result<Self> {
        let selected = crate::storage::RepositoryStorage::new(repo).map_err(storage)?;
        crate::state::ensure_checkout_state(selected.primary.root()).map_err(storage)?;
        let directory = selected
            .coordination_dir()
            .map_err(storage)?
            .join("agent-tasks-v1");
        check_directory(&directory, true)?;
        // Persist new directory links as well as later atomic record renames.
        // Stop at the selected primary checkout; no external state locations.
        for ancestor in directory.ancestors() {
            std::fs::File::open(ancestor)
                .and_then(|file| file.sync_all())
                .map_err(storage)?;
            if ancestor == selected.primary.root() {
                break;
            }
        }
        Ok(Self::Local {
            directory,
            repository: selected.identity,
        })
    }

    pub fn execution_lock(&self, id: &str) -> Result<ExecutionLock> {
        if matches!(self, Self::Inspection { .. }) {
            return Err(Error::Denied);
        }
        task_id(id)?;
        match self {
            Self::Inspection { .. } => unreachable!(),
            Self::Memory { effects, .. } => {
                let flag = effects
                    .lock()
                    .map_err(|_| Error::Busy)?
                    .entry(id.into())
                    .or_default()
                    .clone();
                flag.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                    .map_err(|_| Error::Busy)?;
                Ok(ExecutionLock::Memory(flag))
            }
            Self::Local { directory, .. } => {
                check_directory(directory, false)?;
                Ok(ExecutionLock::Local(
                    crate::task::StateLock::try_acquire(
                        &directory.join(format!("{id}.execution.lock")),
                    )
                    .map_err(storage)?
                    .ok_or(Error::Busy)?,
                ))
            }
        }
    }

    /// Every read/modify/write shares one lock. Failed transitions leave the old
    /// value intact. Atomic replacement publishes receipts, events and state as
    /// one durable unit before an effect may be invoked.
    pub fn transaction<R>(
        &self,
        id: &str,
        f: impl FnOnce(&mut Option<Entry>) -> Result<R>,
    ) -> Result<R> {
        if let Self::Inspection { entries } = self {
            let before = entries.get(id).cloned();
            let mut entry = before.clone();
            let result = f(&mut entry)?;
            return if entry == before {
                Ok(result)
            } else {
                Err(Error::Denied)
            };
        }
        task_id(id)?;
        match self {
            Self::Inspection { .. } => unreachable!(),
            Self::Memory { entries, .. } => {
                let mut entries = entries.lock().map_err(|_| Error::Busy)?;
                let mut entry = entries.get(id).cloned();
                let result = f(&mut entry)?;
                if let Some(value) = entry {
                    value.validate()?;
                    encode(&value)?;
                    entries.insert(id.to_owned(), value);
                }
                Ok(result)
            }
            Self::Local {
                directory,
                repository,
            } => {
                check_directory(directory, false)?;
                let lock_path = directory.join(format!("{id}.lock"));
                let _lock = crate::task::StateLock::try_acquire(&lock_path)
                    .map_err(storage)?
                    .ok_or(Error::Busy)?;
                let path = directory.join(format!("{id}.json"));
                let mut entry = read(&path)?;
                if let Some(entry) = &entry {
                    validate(entry, id, repository)?;
                }
                let before = entry.clone();
                let result = f(&mut entry)?;
                if entry != before {
                    let entry = entry.as_ref().ok_or(Error::InvalidRequest)?;
                    validate(entry, id, repository)?;
                    let bytes = encode(entry)?;
                    crate::state::confine_file(&path).map_err(storage)?;
                    crate::private_io::atomic_write(
                        &path,
                        &bytes,
                        crate::private_io::Durability::Durable,
                    )
                    .map_err(storage)?;
                }
                Ok(result)
            }
        }
    }
}

fn encode(entry: &Entry) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(entry).map_err(storage)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(Error::Capacity);
    }
    Ok(bytes)
}
fn validate(entry: &Entry, id: &str, repository: &str) -> Result<()> {
    if entry.version != VERSION || entry.snapshot.version != VERSION {
        return Err(Error::UnsupportedVersion);
    }
    if entry.snapshot.identity.task != id || entry.snapshot.identity.repository != repository {
        return Err(Error::Conflict);
    }
    entry.validate()
}
fn check_directory(path: &std::path::Path, create: bool) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    crate::state::confine_existing_dir(path).map_err(storage)?;
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            // SAFETY: geteuid has no preconditions.
            if !meta.is_dir()
                || meta.uid() != unsafe { libc::geteuid() }
                || meta.mode() & 0o7077 != 0
            {
                return Err(storage("task service requires an owner-only directory"));
            }
        }
        Err(e) if create && e.kind() == std::io::ErrorKind::NotFound => {
            crate::state::create_private_dir_all(path).map_err(storage)?;
        }
        Err(e) => return Err(storage(e)),
    }
    Ok(())
}
fn read(path: &std::path::Path) -> Result<Option<Entry>> {
    use std::os::unix::fs::OpenOptionsExt;
    if crate::state::confine_file(path).map_err(storage)?.is_none() {
        return Ok(None);
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(storage)?;
    crate::storage::validate_owned_metadata(&file.metadata().map_err(storage)?, true)
        .map_err(storage)?;
    let bytes = crate::util::read_limited(&mut file, MAX_RECORD_BYTES as u64).map_err(storage)?;
    let version: serde_json::Value = serde_json::from_slice(&bytes).map_err(storage)?;
    if version.get("version").and_then(|v| v.as_u64()) != Some(VERSION.into()) {
        return Err(Error::UnsupportedVersion);
    }
    serde_json::from_slice(&bytes).map(Some).map_err(storage)
}

// Fields intentionally retained for RAII ownership through the adapter call.
#[allow(dead_code)]
pub(super) enum ExecutionLock {
    Memory(Arc<AtomicBool>),
    Local(crate::task::StateLock),
}

impl Drop for ExecutionLock {
    fn drop(&mut self) {
        if let Self::Memory(flag) = self {
            flag.store(false, Ordering::Release);
        }
    }
}

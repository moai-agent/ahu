//! Host-owned issue/task associations and private numeric reports.
//!
//! Association data is deliberately kept outside repositories, task records,
//! prompts, configuration snapshots, MCP, and telemetry exports.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::git::Repo;
use crate::telemetry::private::{AttemptObservation, PrivateMapping};
use crate::util::{Error, Result};

const STORE_NAME: &str = "private-measurements.json";
const MAX_STORE_BYTES: u64 = 1024 * 1024;
const MAX_LINKS: usize = 256;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Store {
    schema_version: u32,
    repo_identity: String,
    records: BTreeMap<String, BTreeSet<String>>,
}

fn invalid() -> Error {
    Error::new("private measurement store is invalid or unavailable")
}

fn validate_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.len() > 256
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    {
        return Err(Error::new(
            "record key must be 1–256 ASCII letters, digits, underscores, or hyphens",
        )
        .with_kind(crate::util::ErrorKind::Usage));
    }
    Ok(())
}

fn ensure_record_capacity(store: &Store, record_key: &str) -> Result<()> {
    if store.records.len() >= MAX_LINKS && !store.records.contains_key(record_key) {
        return Err(Error::new(
            "a repository may link at most 256 private records",
        ));
    }
    Ok(())
}

fn base_dir() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_STATE_HOME") {
        Some(path) => PathBuf::from(path),
        None => std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| Error::new("cannot locate the host state directory"))?
            .join(".local/state"),
    };
    if !base.is_absolute() {
        return Err(Error::new("host state directory must be absolute"));
    }
    Ok(base.join("ahu/private-measurements"))
}

fn ensure_private_tree(path: &Path, repo_root: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if !path.is_absolute() {
        return Err(Error::new("host state directory must be absolute"));
    }
    let root = repo_root
        .canonicalize()
        .map_err(|_| Error::new("cannot verify repository boundary for private measurements"))?;
    let mut cursor = PathBuf::new();
    let mut private_zone = false;
    for component in path.components() {
        match component {
            Component::RootDir => cursor.push("/"),
            Component::Normal(name) => {
                cursor.push(name);
                if cursor.ends_with("ahu/private-measurements") || private_zone {
                    private_zone = true;
                }
                match std::fs::symlink_metadata(&cursor) {
                    Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                        return Err(Error::new(
                            "host state path contains a link or non-directory",
                        ));
                    }
                    Ok(meta) => {
                        if private_zone
                            && (meta.uid() != unsafe { libc::geteuid() }
                                || meta.mode() & 0o077 != 0)
                        {
                            return Err(Error::new(
                                "private measurement directory must be owner-only",
                            ));
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        std::fs::create_dir(&cursor)?;
                        if private_zone {
                            std::fs::set_permissions(
                                &cursor,
                                std::fs::Permissions::from_mode(0o700),
                            )?;
                        }
                    }
                    Err(error) => return Err(error.into()),
                }
                let canonical = cursor.canonicalize()?;
                if canonical.starts_with(&root)
                    || canonical
                        .ancestors()
                        .any(|ancestor| ancestor.join(".git").exists())
                {
                    return Err(Error::new(
                        "private measurement state must be outside every checkout",
                    ));
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) => {
                return Err(Error::new("host state directory has an invalid path"));
            }
        }
    }
    Ok(())
}

fn store_dir(repo: &Repo) -> Result<PathBuf> {
    let path = base_dir()?.join(repo.identity());
    ensure_private_tree(&path, &repo.primary_root()?)?;
    Ok(path)
}

struct StoreLock(std::fs::File);

impl StoreLock {
    fn acquire(dir: &Path) -> Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let path = dir.join("lock");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| invalid())?;
        let metadata = file.metadata().map_err(|_| invalid())?;
        use std::os::unix::fs::MetadataExt;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(invalid());
        }
        // SAFETY: flock operates on the open lock descriptor and is released by Drop.
        if unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&file), libc::LOCK_EX) } != 0 {
            return Err(invalid());
        }
        Ok(Self(file))
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        // SAFETY: this is the descriptor acquired by StoreLock::acquire.
        unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&self.0), libc::LOCK_UN) };
    }
}

fn read_store(path: &Path, repo_identity: &str) -> Result<Store> {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::OpenOptionsExt;
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Store {
                schema_version: 1,
                repo_identity: repo_identity.to_owned(),
                records: BTreeMap::new(),
            });
        }
        Err(_) => return Err(invalid()),
    };
    let metadata = file.metadata().map_err(|_| invalid())?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.len() > MAX_STORE_BYTES
    {
        return Err(invalid());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_STORE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    let value: Store = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if value.schema_version != 1
        || value.repo_identity != repo_identity
        || value.records.len() > MAX_LINKS
        || value.records.iter().any(|(key, tasks)| {
            validate_key(key).is_err()
                || tasks.is_empty()
                || tasks.len() > 256
                || tasks
                    .iter()
                    .any(|task_id| !crate::task::is_canonical_task_uuid(task_id))
        })
    {
        return Err(invalid());
    }
    Ok(value)
}

fn write_store(path: &Path, store: &Store) -> Result<()> {
    if store.records.len() > MAX_LINKS
        || store.records.iter().any(|(key, tasks)| {
            validate_key(key).is_err()
                || tasks.is_empty()
                || tasks.len() > 256
                || tasks
                    .iter()
                    .any(|task_id| !crate::task::is_canonical_task_uuid(task_id))
        })
    {
        return Err(invalid());
    }
    let bytes = serde_json::to_vec_pretty(store).map_err(|_| invalid())?;
    if bytes.len() as u64 > MAX_STORE_BYTES {
        return Err(invalid());
    }
    crate::private_io::atomic_write(path, &bytes, crate::private_io::Durability::Durable)
        .map_err(|_| invalid())
}

fn resolve_owned_task(
    repo: &Repo,
    input: &str,
) -> Result<(String, crate::task::TaskRecord, PathBuf)> {
    let id = crate::task_ref::resolve(repo, input)?;
    let listing = crate::task::list(repo)?;
    listing
        .records
        .into_iter()
        .find(|(_, record)| record.task_id == id && record.repo_identity == repo.identity())
        .map(|(dir, record)| (id, record, dir))
        .ok_or_else(|| Error::new("task is not present in this repository's verified task list"))
}

pub fn link(repo: &Repo, record_key: &str, task_ref: &str) -> Result<()> {
    validate_key(record_key)?;
    let (task_id, _, _) = resolve_owned_task(repo, task_ref)?;
    if !crate::task::is_canonical_task_uuid(&task_id) {
        return Err(
            Error::new("private measurement links require canonical task UUIDs")
                .with_kind(crate::util::ErrorKind::Usage),
        );
    }
    let directory = store_dir(repo)?;
    let _lock = StoreLock::acquire(&directory)?;
    let path = directory.join(STORE_NAME);
    let mut store = read_store(&path, &repo.identity())?;
    ensure_record_capacity(&store, record_key)?;
    let tasks = store.records.entry(record_key.to_owned()).or_default();
    if tasks.len() >= 256 && !tasks.contains(&task_id) {
        return Err(Error::new("a private record may link at most 256 tasks"));
    }
    tasks.insert(task_id);
    write_store(&path, &store)
}

pub fn unlink(repo: &Repo, record_key: &str, task_ref: Option<&str>) -> Result<bool> {
    validate_key(record_key)?;
    let task_id = task_ref
        .map(|value| crate::task_ref::resolve(repo, value))
        .transpose()?;
    let directory = store_dir(repo)?;
    let _lock = StoreLock::acquire(&directory)?;
    let path = directory.join(STORE_NAME);
    let mut store = read_store(&path, &repo.identity())?;
    let changed = if let Some(task_id) = task_id {
        let removed = store.records.get_mut(record_key).is_some_and(|tasks| {
            let removed = tasks.remove(&task_id);
            if tasks.is_empty() {
                return removed;
            }
            removed
        });
        if store
            .records
            .get(record_key)
            .is_some_and(BTreeSet::is_empty)
        {
            store.records.remove(record_key);
        }
        removed
    } else {
        store.records.remove(record_key).is_some()
    };
    if changed {
        write_store(&path, &store)?;
    }
    Ok(changed)
}

pub fn report(repo: &Repo, record_key: &str) -> Result<serde_json::Value> {
    validate_key(record_key)?;
    let directory = store_dir(repo)?;
    let _lock = StoreLock::acquire(&directory)?;
    let store = read_store(&directory.join(STORE_NAME), &repo.identity())?;
    let task_ids = store.records.get(record_key).cloned().ok_or_else(|| {
        Error::new("no private task association has that record key")
            .with_kind(crate::util::ErrorKind::Usage)
    })?;
    drop(_lock);
    drop(store);
    let telemetry_config = crate::config::load(&repo.primary_root()?)?
        .map(|loaded| loaded.config.telemetry)
        .unwrap_or_default();
    if !telemetry_config.local_metrics {
        return Err(Error::new(
            "private reports require telemetry.local_metrics = true before launch",
        ));
    }
    let listing = crate::task::list(repo)?;
    let mut observations = Vec::new();
    let mut missing_tasks = 0usize;
    let mut tasks_without_completed_attempts = 0usize;
    for task_id in &task_ids {
        let Some((dir, record)) = listing.records.iter().find(|(_, record)| {
            record.task_id == *task_id && record.repo_identity == repo.identity()
        }) else {
            missing_tasks += 1;
            continue;
        };
        let before = observations.len();
        if crate::headless::review::is_headless(dir) {
            for attempt in crate::headless::private_attempt_metrics(
                dir,
                &record.identity.harness,
                4096 - observations.len(),
            )? {
                observations.push((record, dir, attempt));
            }
        }
        if observations.len() == before {
            tasks_without_completed_attempts += 1;
        }
    }
    let mapping_bytes = serde_json::to_vec(&serde_json::json!({
        "schema_version":1,
        "record_key":record_key,
        "repo_identity":repo.identity(),
        "tasks":&task_ids,
    }))?;
    let mapping = PrivateMapping::parse(&mapping_bytes)?;
    // The private mapping primitive enforces homogeneous groups and duplicate
    // conflict rules. Build one summary at a time from allowlisted projections.
    let mut report_groups = Vec::new();
    for (agent, harness, model, outcome) in group_keys(&observations) {
        let selected: Vec<_> = observations
            .iter()
            .filter(|(record, _, attempt)| {
                record.identity.identity_digest == agent
                    && record.identity.harness == harness
                    && record.identity.model == model
                    && attempt.outcome == outcome
            })
            .collect();
        let owned: Vec<_> = selected
            .iter()
            .map(|(record, _, attempt)| AttemptObservation {
                repo_identity: &record.repo_identity,
                task_id: &record.task_id,
                attempt: attempt.attempt,
                agent_identity_digest: record.identity.identity_digest.as_deref(),
                harness: &record.identity.harness,
                model: &record.identity.model,
                outcome: &attempt.outcome,
                elapsed_ms: attempt.elapsed_ms,
                usage: &attempt.usage,
                cost: &attempt.cost,
            })
            .collect();
        let summary = mapping
            .summarize(&telemetry_config, &owned)?
            .ok_or_else(|| Error::new("local metrics are disabled"))?;
        let mut summary = serde_json::to_value(summary)?;
        if let Some((record, _, _)) = selected.first() {
            summary["agent"] = serde_json::Value::String(record.agent_label());
        }
        report_groups.push(summary);
    }
    Ok(serde_json::json!({
        "schema_version":1,
        "record_key":record_key,
        "linked_tasks":task_ids.len(),
        "missing_tasks":missing_tasks,
        "tasks_without_headless_attempt_results":tasks_without_completed_attempts,
        "completed_attempts":observations.len(),
        "attempts_with_opt_in_metrics":observations.iter().filter(|(_,_,attempt)| attempt.metrics_observed).count(),
        "capacity":{
            "kind":"unavailable",
            "reason":"no trusted per-run or account capacity signal is collected"
        },
        "groups":report_groups,
        "limitations":[
            "Only headless attempts with opted-in local metrics are included.",
            "Interactive task usage is unavailable to this report.",
            "Token values are per-attempt observations and are not summed across retries or child agents.",
            "USD values are harness-reported and remain separated by source; quota and billing are not inferred."
        ]
    }))
}

fn group_keys(
    observations: &[(
        &crate::task::TaskRecord,
        &PathBuf,
        crate::headless::PrivateAttemptMetrics,
    )],
) -> BTreeSet<(Option<String>, String, String, String)> {
    observations
        .iter()
        .map(|(record, _, attempt)| {
            (
                record.identity.identity_digest.clone(),
                record.identity.harness.clone(),
                record.identity.model.clone(),
                attempt.outcome.clone(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    const REPO: &str = "0123456789abcdef";
    const TASK: &str = "00000000-0000-7000-8000-000000000001";

    fn platform_tempdir() -> tempfile::TempDir {
        let base = std::env::temp_dir().canonicalize().unwrap();
        tempfile::tempdir_in(base).unwrap()
    }

    #[test]
    fn record_keys_are_opaque_and_bounded() {
        for key in ["r-59", "work_42", "Ab9"] {
            validate_key(key).unwrap();
        }
        for key in [
            "",
            "https://github.com/private/1",
            "a b",
            "x".repeat(257).as_str(),
        ] {
            assert!(validate_key(key).is_err());
        }
    }

    #[test]
    fn external_store_is_owner_only_and_round_trips_validated_links() {
        let root = platform_tempdir();
        let checkout = platform_tempdir();
        let directory = root.path().join("ahu/private-measurements").join(REPO);
        ensure_private_tree(&directory, checkout.path()).unwrap();
        assert_eq!(std::fs::metadata(&directory).unwrap().mode() & 0o777, 0o700);
        let mut records = BTreeMap::new();
        records.insert("r-59".into(), BTreeSet::from([TASK.into()]));
        let store = Store {
            schema_version: 1,
            repo_identity: REPO.into(),
            records,
        };
        let path = directory.join(STORE_NAME);
        write_store(&path, &store).unwrap();
        assert_eq!(read_store(&path, REPO).unwrap().records, store.records);
        assert_eq!(std::fs::metadata(path).unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn store_key_capacity_is_consistent_for_link_read_and_write() {
        let root = platform_tempdir();
        let path = root.path().join(STORE_NAME);
        let mut records: BTreeMap<_, _> = (0..MAX_LINKS)
            .map(|index| (format!("r{index}"), BTreeSet::from([TASK.into()])))
            .collect();
        let store = Store {
            schema_version: 1,
            repo_identity: REPO.into(),
            records: records.clone(),
        };
        ensure_record_capacity(&store, "r255").unwrap();
        assert!(ensure_record_capacity(&store, "overflow").is_err());
        write_store(&path, &store).unwrap();
        assert_eq!(read_store(&path, REPO).unwrap().records.len(), MAX_LINKS);

        records.insert("overflow".into(), BTreeSet::from([TASK.into()]));
        let too_many = Store { records, ..store };
        assert!(write_store(&path, &too_many).is_err());
        assert_eq!(read_store(&path, REPO).unwrap().records.len(), MAX_LINKS);
    }

    #[test]
    fn external_store_rejects_checkout_paths_symlinks_and_wrong_owner_modes() {
        let root = platform_tempdir();
        let checkout = root.path().join("repo");
        std::fs::create_dir(&checkout).unwrap();
        std::fs::create_dir(checkout.join(".git")).unwrap();
        assert!(ensure_private_tree(&checkout.join(".ahu/state/secret"), &checkout).is_err());

        let external = platform_tempdir();
        let target = external.path().join("target");
        std::fs::create_dir(&target).unwrap();
        let link = external.path().join("linked");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(ensure_private_tree(&link.join("ahu/private-measurements"), &checkout).is_err());

        let broad = external.path().join("ahu/private-measurements");
        std::fs::create_dir_all(&broad).unwrap();
        std::fs::set_permissions(&broad, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(ensure_private_tree(&broad.join(REPO), &checkout).is_err());
    }

    #[test]
    fn store_rejects_wrong_repo_and_unknown_or_unbounded_entries() {
        let root = platform_tempdir();
        let path = root.path().join(STORE_NAME);
        let store = Store {
            schema_version: 1,
            repo_identity: REPO.into(),
            records: BTreeMap::from([("r-59".into(), BTreeSet::from([TASK.into()]))]),
        };
        write_store(&path, &store).unwrap();
        assert!(read_store(&path, "fedcba9876543210").is_err());

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        use std::io::Write;
        file.write_all(br#"{"schema_version":1,"repo_identity":"0123456789abcdef","records":{},"private":"marker"}"#).unwrap();
        assert_eq!(file.metadata().unwrap().uid(), unsafe { libc::geteuid() });
        drop(file);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(read_store(&path, REPO).is_err());
    }
}

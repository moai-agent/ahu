//! The committed project context lock.
//!
//! The lock fingerprints the repository context ahu recognizes. Launches require
//! both the lock and every recognized context input to be committed and clean.
//! This cannot lock harness built-ins, managed policy, or other opaque sources.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bail;
use crate::git::{self, Repo};
use crate::private_io::Durability;
use crate::snapshot::{ConfigSnapshot, SnapshotEntry};
use crate::util::{Error, Result, digest_bytes};

pub const LOCK_PATH: &str = "ahu.lock";
const LOCK_SCHEMA_VERSION: u32 = 1;
const MAX_LOCK_BYTES: u64 = 2 * 1024 * 1024;

/// User-local inputs are locked in owner-only host state so they affect launch
/// admission without adding user-specific fingerprints to shared `ahu.lock`.
const LOCAL_CONTEXT_PATHS: &[&str] = &[".claude/settings.local.json"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct LockFile {
    schema_version: u32,
    digest: String,
    entries: Vec<SnapshotEntry>,
    skipped_directories: Vec<String>,
    unscanned_config: Vec<String>,
    symlinks: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub current: bool,
    pub detail: String,
}

impl LockFile {
    fn from_snapshot(snapshot: &ConfigSnapshot) -> Self {
        let snapshot = shared_snapshot(snapshot);
        Self {
            schema_version: LOCK_SCHEMA_VERSION,
            digest: context_digest(&snapshot),
            entries: snapshot.entries,
            skipped_directories: snapshot.skipped_directories,
            unscanned_config: snapshot.unscanned_config,
            symlinks: snapshot.symlinks,
        }
    }

    fn parse(path: &Path) -> Result<Self> {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| Error::new(format!("cannot inspect {}: {error}", path.display())))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            bail!("{} must be a regular file, not a symlink", LOCK_PATH);
        }
        if metadata.len() > MAX_LOCK_BYTES {
            bail!("{} exceeds the 2 MiB size limit", LOCK_PATH);
        }
        let bytes = std::fs::read(path)?;
        let text = std::str::from_utf8(&bytes).map_err(|_| Error::new("ahu.lock is not UTF-8"))?;
        let lock: Self = toml::from_str(text)
            .map_err(|error| Error::new(format!("ahu.lock is not valid TOML: {error}")))?;
        if lock.schema_version != LOCK_SCHEMA_VERSION {
            bail!(
                "ahu.lock schema_version {} is unsupported; expected {}",
                lock.schema_version,
                LOCK_SCHEMA_VERSION
            );
        }
        Ok(lock)
    }
}

fn context_digest(snapshot: &ConfigSnapshot) -> String {
    lock_digest(
        &snapshot.entries,
        &snapshot.unscanned_config,
        &snapshot.symlinks,
    )
}

fn lock_digest(
    entries: &[SnapshotEntry],
    unscanned_config: &[String],
    symlinks: &[String],
) -> String {
    let mut entries = entries.to_vec();
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let canonical = serde_json::json!({
        "entries": entries,
        "unscanned_config": unscanned_config,
        "symlinks": symlinks,
    });
    digest_bytes(canonical.to_string().as_bytes())
}

fn is_local_context_path(path: &str) -> bool {
    LOCAL_CONTEXT_PATHS.contains(&path)
}

fn shared_snapshot(snapshot: &ConfigSnapshot) -> ConfigSnapshot {
    let mut shared = snapshot.clone();
    shared
        .entries
        .retain(|entry| !is_local_context_path(&entry.path));
    shared
        .unscanned_config
        .retain(|path| !is_local_context_path(path));
    shared.symlinks.retain(|path| !is_local_context_path(path));
    shared
}

fn local_fingerprints(snapshot: &ConfigSnapshot) -> BTreeMap<String, String> {
    snapshot
        .entries
        .iter()
        .filter(|entry| is_local_context_path(&entry.path))
        .map(|entry| (entry.path.clone(), entry.digest.clone()))
        .collect()
}

pub fn refresh(repo: &Repo, snapshot: &ConfigSnapshot) -> Result<PathBuf> {
    refresh_with_state_home(repo, snapshot, None)
}

/// Refresh project context as part of explicit setup. A first setup establishes
/// local acceptance, while rerunning setup never silently accepts later drift.
pub fn refresh_for_setup(repo: &Repo, snapshot: &ConfigSnapshot) -> Result<PathBuf> {
    reject_local_symlinks(snapshot)?;
    let path = write_shared_lock(repo, snapshot)?;
    crate::private_context_lock::initialize_if_missing(repo, &local_fingerprints(snapshot))?;
    Ok(path)
}

#[doc(hidden)]
pub fn refresh_with_state_home(
    repo: &Repo,
    snapshot: &ConfigSnapshot,
    state_home: Option<&Path>,
) -> Result<PathBuf> {
    reject_local_symlinks(snapshot)?;
    let path = write_shared_lock(repo, snapshot)?;
    let fingerprints = local_fingerprints(snapshot);
    // Explicit acceptance includes deletion of the last private input. The
    // private store avoids creating state when there is nothing to accept.
    crate::private_context_lock::accept(repo, &fingerprints, state_home)?;
    Ok(path)
}

fn write_shared_lock(repo: &Repo, snapshot: &ConfigSnapshot) -> Result<PathBuf> {
    let path = repo.root.join(LOCK_PATH);
    let lock = LockFile::from_snapshot(snapshot);
    let body = toml::to_string_pretty(&lock)
        .map_err(|error| Error::new(format!("cannot serialize ahu.lock: {error}")))?;
    crate::private_io::atomic_write(&path, body.as_bytes(), Durability::Durable)?;
    Ok(path)
}

/// Write only the shared lock for fixtures that intentionally contain an
/// unsafe local input, so they can assert that later launch admission refuses it.
#[doc(hidden)]
pub fn refresh_shared_only(repo: &Repo, snapshot: &ConfigSnapshot) -> Result<PathBuf> {
    write_shared_lock(repo, snapshot)
}

fn reject_local_symlinks(snapshot: &ConfigSnapshot) -> Result<()> {
    if snapshot
        .symlinks
        .iter()
        .any(|path| is_local_context_path(path))
    {
        return Err(Error::new(
            "private local agent settings cannot be symlinks; replace the link with a regular file before accepting context",
        ));
    }
    Ok(())
}

/// Compare the lock to current inputs, then require all those inputs and the
/// lock itself to match committed HEAD. Call this before any candidate launch.
pub fn check(repo: &Repo, snapshot: &ConfigSnapshot) -> Result<Status> {
    check_with_state_home(repo, snapshot, None)
}

#[doc(hidden)]
pub fn check_with_state_home(
    repo: &Repo,
    snapshot: &ConfigSnapshot,
    state_home: Option<&Path>,
) -> Result<Status> {
    let path = repo.root.join(LOCK_PATH);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Status {
                current: false,
                detail: format!(
                    "{} is missing; run `ahu lock --update`, then commit it",
                    LOCK_PATH
                ),
            });
        }
        Err(error) => {
            return Err(Error::new(format!(
                "cannot inspect {}: {error}",
                path.display()
            )));
        }
        Ok(_) => {}
    }
    let lock = LockFile::parse(&path)?;

    let current = LockFile::from_snapshot(snapshot);
    let mut problems = Vec::new();
    let obsolete_local_metadata = lock
        .entries
        .iter()
        .any(|entry| is_local_context_path(&entry.path))
        || lock
            .unscanned_config
            .iter()
            .any(|path| is_local_context_path(path))
        || lock.symlinks.iter().any(|path| is_local_context_path(path));
    if obsolete_local_metadata {
        problems.push("ahu.lock contains obsolete per-user metadata; refresh it".into());
    }
    let lock_entries: Vec<_> = lock
        .entries
        .iter()
        .filter(|entry| !is_local_context_path(&entry.path))
        .cloned()
        .collect();
    let lock_unscanned: Vec<_> = lock
        .unscanned_config
        .iter()
        .filter(|path| !is_local_context_path(path))
        .cloned()
        .collect();
    let lock_symlinks: Vec<_> = lock
        .symlinks
        .iter()
        .filter(|path| !is_local_context_path(path))
        .cloned()
        .collect();
    let comparable_lock_digest = if obsolete_local_metadata {
        lock_digest(&lock_entries, &lock_unscanned, &lock_symlinks)
    } else {
        lock.digest.clone()
    };
    if comparable_lock_digest != current.digest {
        let before: BTreeMap<_, _> = lock_entries.iter().map(|item| (&item.path, item)).collect();
        let after: BTreeMap<_, _> = current
            .entries
            .iter()
            .map(|item| (&item.path, item))
            .collect();
        let paths: BTreeSet<_> = before.keys().chain(after.keys()).copied().collect();
        let mut changed = Vec::new();
        for path in paths {
            if before.get(path) != after.get(path) {
                changed.push(path.as_str());
            }
        }
        if !changed.is_empty() {
            problems.push(format!(
                "context differs from ahu.lock: {}",
                changed.join(", ")
            ));
        }
        if lock_unscanned != current.unscanned_config || lock_symlinks != current.symlinks {
            problems.push("context scan coverage changed".into());
        }
        if problems.is_empty() {
            problems.push("context digest differs from ahu.lock".into());
        }
    }

    let lock_tracked = is_tracked(&repo.root, LOCK_PATH)?;
    if !lock_tracked {
        problems.push(format!("{LOCK_PATH} is not tracked by Git"));
    } else if !is_clean_at_head(&repo.root, LOCK_PATH)? {
        problems.push(format!("{LOCK_PATH} has uncommitted changes"));
    }

    // Include removed inputs from the lock as well as current files; a deleted
    // context file still has to be committed with the new lock.
    let mut paths: BTreeSet<&str> = snapshot
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    paths.extend(lock.entries.iter().map(|entry| entry.path.as_str()));
    for path in paths {
        if is_local_context_path(path) {
            continue;
        }
        if !is_tracked(&repo.root, path)? {
            problems.push(format!("{path} is not tracked by Git"));
        } else if !is_clean_at_head(&repo.root, path)? {
            problems.push(format!("{path} has uncommitted changes"));
        }
    }

    let shared = shared_snapshot(snapshot);
    if !shared.symlinks.is_empty() {
        problems.push(format!(
            "context symlinks cannot be locked safely: {}",
            shared.symlinks.join(", ")
        ));
    }
    if !shared.unscanned_config.is_empty() {
        problems.push(format!(
            "configuration inside skipped scan paths is not lockable: {}",
            shared.unscanned_config.join(", ")
        ));
    }

    let has_local_symlink = snapshot
        .symlinks
        .iter()
        .any(|path| is_local_context_path(path));
    if has_local_symlink
        || !crate::private_context_lock::is_current(
            repo,
            &local_fingerprints(snapshot),
            state_home,
        )?
    {
        problems.push("private local agent settings are new or changed for this user".into());
    }

    Ok(if problems.is_empty() {
        Status {
            current: true,
            detail: format!("{} matches committed context", LOCK_PATH),
        }
    } else {
        Status {
            current: false,
            detail: format!(
                "{}; run `ahu lock --update`, then review and commit ahu.lock if shared context changed",
                problems.join("; ")
            ),
        }
    })
}

fn is_tracked(root: &Path, path: &str) -> Result<bool> {
    let output = git::run(
        root,
        &[
            "--literal-pathspecs",
            "ls-files",
            "--error-unmatch",
            "--",
            path,
        ],
    )?;
    Ok(output.status.success())
}

fn is_clean_at_head(root: &Path, path: &str) -> Result<bool> {
    let output = git::run(
        root,
        &[
            "--literal-pathspecs",
            "diff",
            "--name-only",
            "HEAD",
            "--",
            path,
        ],
    )?;
    if output.status.success() {
        Ok(output.stdout.is_empty())
    } else {
        bail!(
            "cannot compare {path} with committed HEAD: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git;
    use std::process::Command;

    fn git_cmd(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "ahu tests")
            .env("GIT_AUTHOR_EMAIL", "tests@example.invalid")
            .env("GIT_COMMITTER_NAME", "ahu tests")
            .env("GIT_COMMITTER_EMAIL", "tests@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn lock_requires_update_commit_and_clean_context_before_it_passes() {
        let fixture = tempfile::tempdir().unwrap();
        git_cmd(fixture.path(), &["init", "-q", "-b", "main"]);
        git_cmd(fixture.path(), &["config", "user.name", "ahu tests"]);
        git_cmd(
            fixture.path(),
            &["config", "user.email", "tests@example.invalid"],
        );
        std::fs::create_dir_all(fixture.path().join(".agents/ahu")).unwrap();
        std::fs::write(
            fixture.path().join(".agents/ahu/config.toml"),
            "schema_version = 1\n",
        )
        .unwrap();
        let repo = git::discover(fixture.path()).unwrap();
        let snapshot = crate::snapshot::collect(fixture.path()).unwrap();
        refresh(&repo, &snapshot).unwrap();
        assert!(!check(&repo, &snapshot).unwrap().current);
        git_cmd(fixture.path(), &["add", "-A"]);
        git_cmd(fixture.path(), &["commit", "-q", "-m", "lock"]);
        let repo = git::discover(fixture.path()).unwrap();
        assert!(check(&repo, &snapshot).unwrap().current);

        std::fs::write(
            fixture.path().join(".agents/ahu/config.toml"),
            "schema_version = 2\n",
        )
        .unwrap();
        let changed = crate::snapshot::collect(fixture.path()).unwrap();
        let status = check(&repo, &changed).unwrap();
        assert!(!status.current);
        assert!(status.detail.contains("uncommitted changes"));

        refresh(&repo, &changed).unwrap();
        assert!(!check(&repo, &changed).unwrap().current);
    }

    #[test]
    fn ignored_claude_local_settings_use_private_acceptance_without_churning_shared_lock() {
        let fixture = tempfile::tempdir().unwrap();
        git_cmd(fixture.path(), &["init", "-q", "-b", "main"]);
        git_cmd(fixture.path(), &["config", "user.name", "ahu tests"]);
        git_cmd(
            fixture.path(),
            &["config", "user.email", "tests@example.invalid"],
        );
        std::fs::create_dir_all(fixture.path().join(".agents/ahu")).unwrap();
        std::fs::create_dir_all(fixture.path().join(".claude")).unwrap();
        std::fs::write(
            fixture.path().join(".gitignore"),
            ".claude/settings.local.json\n",
        )
        .unwrap();
        std::fs::write(
            fixture.path().join(".agents/ahu/config.toml"),
            "schema_version = 1\n",
        )
        .unwrap();
        let local_settings = fixture.path().join(".claude/settings.local.json");
        std::fs::write(
            &local_settings,
            r#"{"enabledMcpjsonServers":["ahu"],"privateNote":"LOCAL-ONLY-DETAIL"}"#,
        )
        .unwrap();

        let repo = git::discover(fixture.path()).unwrap();
        let snapshot = crate::snapshot::collect(fixture.path()).unwrap();
        assert!(
            snapshot
                .entries
                .iter()
                .any(|entry| entry.path == ".claude/settings.local.json")
        );
        let state_home_dir = tempfile::tempdir().unwrap();
        let state_home = state_home_dir.path().canonicalize().unwrap();
        refresh_with_state_home(&repo, &snapshot, Some(&state_home)).unwrap();
        let lock_text = std::fs::read_to_string(fixture.path().join(LOCK_PATH)).unwrap();
        assert!(!lock_text.contains(".claude/settings.local.json"));
        assert!(!lock_text.contains("LOCAL-ONLY-DETAIL"));

        git_cmd(
            fixture.path(),
            &["add", ".gitignore", ".agents/ahu/config.toml", LOCK_PATH],
        );
        git_cmd(
            fixture.path(),
            &["commit", "-q", "-m", "lock public context"],
        );
        let repo = git::discover(fixture.path()).unwrap();
        assert!(
            check_with_state_home(&repo, &snapshot, Some(&state_home))
                .unwrap()
                .current
        );

        // A value change affects agent behavior, so it must make this user's
        // local acceptance stale without changing the shared lock.
        std::fs::write(
            &local_settings,
            r#"{"enabledMcpjsonServers":["ahu"],"privateNote":"CHANGED-LOCAL-DETAIL"}"#,
        )
        .unwrap();
        let changed = crate::snapshot::collect(fixture.path()).unwrap();
        let status = check_with_state_home(&repo, &changed, Some(&state_home)).unwrap();
        assert!(!status.current);
        assert!(status.detail.contains("private local agent settings"));
        assert!(!status.detail.contains("CHANGED-LOCAL-DETAIL"));

        refresh_with_state_home(&repo, &changed, Some(&state_home)).unwrap();
        let updated_lock = std::fs::read_to_string(fixture.path().join(LOCK_PATH)).unwrap();
        assert_eq!(updated_lock, lock_text);
        assert!(
            check_with_state_home(&repo, &changed, Some(&state_home))
                .unwrap()
                .current
        );
        assert!(
            git::run(fixture.path(), &["status", "--porcelain"])
                .unwrap()
                .stdout
                .is_empty()
        );

        // Removing the final private input needs explicit acceptance too.
        std::fs::remove_file(&local_settings).unwrap();
        let removed = crate::snapshot::collect(fixture.path()).unwrap();
        assert!(local_fingerprints(&removed).is_empty());
        assert!(
            !check_with_state_home(&repo, &removed, Some(&state_home))
                .unwrap()
                .current
        );
        refresh_with_state_home(&repo, &removed, Some(&state_home)).unwrap();
        assert!(
            check_with_state_home(&repo, &removed, Some(&state_home))
                .unwrap()
                .current
        );
        assert_eq!(
            std::fs::read_to_string(fixture.path().join(LOCK_PATH)).unwrap(),
            lock_text
        );

        // No private inputs must not create a store, but an existing unsafe
        // acceptance must still be refused rather than silently replaced.
        let fresh = tempfile::tempdir().unwrap();
        refresh_with_state_home(&repo, &removed, Some(fresh.path())).unwrap();
        assert!(!fresh.path().join("ahu").exists());
        let acceptance = state_home
            .join("ahu/context-locks")
            .join(repo.identity())
            .join("acceptance.json");
        std::fs::write(&acceptance, "invalid").unwrap();
        assert!(refresh_with_state_home(&repo, &removed, Some(&state_home)).is_err());
        assert_eq!(std::fs::read_to_string(acceptance).unwrap(), "invalid");
    }

    #[test]
    fn a_missing_or_symlink_lock_is_not_accepted() {
        let fixture = tempfile::tempdir().unwrap();
        git_cmd(fixture.path(), &["init", "-q", "-b", "main"]);
        git_cmd(fixture.path(), &["config", "user.name", "ahu tests"]);
        git_cmd(
            fixture.path(),
            &["config", "user.email", "tests@example.invalid"],
        );
        std::fs::write(fixture.path().join("README.md"), "fixture\n").unwrap();
        git_cmd(fixture.path(), &["add", "README.md"]);
        git_cmd(fixture.path(), &["commit", "-q", "-m", "base"]);
        let repo = git::discover(fixture.path()).unwrap();
        assert!(!check(&repo, &ConfigSnapshot::default()).unwrap().current);
        #[cfg(unix)]
        {
            let external = fixture.path().join("external-lock");
            std::fs::write(&external, "schema_version = 1\n").unwrap();
            std::os::unix::fs::symlink(&external, fixture.path().join(LOCK_PATH)).unwrap();
            assert!(check(&repo, &ConfigSnapshot::default()).is_err());
        }
    }
}

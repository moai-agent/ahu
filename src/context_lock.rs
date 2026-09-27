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
        Self {
            schema_version: LOCK_SCHEMA_VERSION,
            digest: context_digest(snapshot),
            entries: snapshot.entries.clone(),
            skipped_directories: snapshot.skipped_directories.clone(),
            unscanned_config: snapshot.unscanned_config.clone(),
            symlinks: snapshot.symlinks.clone(),
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
    let mut entries = snapshot.entries.clone();
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    let canonical = serde_json::json!({
        "entries": entries,
        "unscanned_config": snapshot.unscanned_config,
        "symlinks": snapshot.symlinks,
    });
    digest_bytes(canonical.to_string().as_bytes())
}

pub fn refresh(repo: &Repo, snapshot: &ConfigSnapshot) -> Result<PathBuf> {
    let path = repo.root.join(LOCK_PATH);
    let lock = LockFile::from_snapshot(snapshot);
    let body = toml::to_string_pretty(&lock)
        .map_err(|error| Error::new(format!("cannot serialize ahu.lock: {error}")))?;
    crate::private_io::atomic_write(&path, body.as_bytes(), Durability::Durable)?;
    Ok(path)
}

/// Compare the lock to current inputs, then require all those inputs and the
/// lock itself to match committed HEAD. Call this before any candidate launch.
pub fn check(repo: &Repo, snapshot: &ConfigSnapshot) -> Result<Status> {
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
    if lock.digest != current.digest {
        let before: BTreeMap<_, _> = lock.entries.iter().map(|item| (&item.path, item)).collect();
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
        if lock.unscanned_config != current.unscanned_config || lock.symlinks != current.symlinks {
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
        if !is_tracked(&repo.root, path)? {
            problems.push(format!("{path} is not tracked by Git"));
        } else if !is_clean_at_head(&repo.root, path)? {
            problems.push(format!("{path} has uncommitted changes"));
        }
    }

    if !snapshot.symlinks.is_empty() {
        problems.push(format!(
            "context symlinks cannot be locked safely: {}",
            snapshot.symlinks.join(", ")
        ));
    }
    if !snapshot.unscanned_config.is_empty() {
        problems.push(format!(
            "configuration inside skipped scan paths is not lockable: {}",
            snapshot.unscanned_config.join(", ")
        ));
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
                "{}; run `ahu lock --update`, review the lock diff, and commit it with the context changes",
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
        &["--literal-pathspecs", "diff", "--quiet", "HEAD", "--", path],
    )?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!(
            "cannot compare {path} with committed HEAD: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
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

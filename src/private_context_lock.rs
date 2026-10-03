//! Per-user acceptance for local context that changes agent behavior.
//!
//! Fingerprints live in owner-only host state outside checkouts. The shared
//! `ahu.lock` therefore stays collaborative and does not churn with local MCP
//! or permission settings.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::git::Repo;
use crate::util::{Error, Result};

const FILE_NAME: &str = "acceptance.json";
const MAX_BYTES: u64 = 64 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Acceptance {
    schema_version: u32,
    repo_identity: String,
    fingerprints: BTreeMap<String, String>,
}

fn invalid() -> Error {
    Error::new("private local-context acceptance is invalid or unavailable")
}

fn default_state_home() -> Result<PathBuf> {
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
    Ok(base)
}

fn state_dir(repo: &Repo, state_home: &Path, create: bool) -> Result<Option<PathBuf>> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    if !state_home.is_absolute()
        || state_home
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(Error::new(
            "host state directory must be an absolute normalized path",
        ));
    }
    // macOS exposes standard temporary and home locations through symlinked
    // ancestors (for example /var -> /private/var). Resolve the caller-chosen
    // state root once, then continue refusing links below the private Ahu dir.
    let state_home = canonicalize_or_append(state_home)?;
    let primary = repo
        .primary_root()?
        .canonicalize()
        .map_err(|_| Error::new("cannot verify repository for private local context"))?;
    let private_root = state_home.join("ahu");
    let target = private_root.join("context-locks").join(repo.identity());
    let mut cursor = PathBuf::new();
    let mut private_zone = false;

    for component in target.components() {
        match component {
            Component::RootDir => cursor.push("/"),
            Component::Normal(name) => {
                cursor.push(name);
                if cursor == private_root || private_zone {
                    private_zone = true;
                }
                let metadata = match std::fs::symlink_metadata(&cursor) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound && !create => {
                        return Ok(None);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        std::fs::create_dir(&cursor)?;
                        if private_zone {
                            std::fs::set_permissions(
                                &cursor,
                                std::fs::Permissions::from_mode(0o700),
                            )?;
                        }
                        std::fs::symlink_metadata(&cursor)?
                    }
                    Err(error) => return Err(error.into()),
                };
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(invalid());
                }
                if private_zone
                    && (metadata.uid() != unsafe { libc::geteuid() }
                        || metadata.mode() & 0o077 != 0)
                {
                    return Err(invalid());
                }
                let canonical = cursor.canonicalize()?;
                if canonical.starts_with(&primary)
                    || canonical
                        .ancestors()
                        .any(|ancestor| ancestor.join(".git").exists())
                {
                    return Err(Error::new(
                        "private local-context state must remain outside every checkout",
                    ));
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::Prefix(_) => {
                return Err(Error::new("host state directory has an invalid path"));
            }
        }
    }
    Ok(Some(target))
}

fn canonicalize_or_append(path: &Path) -> Result<PathBuf> {
    if let Ok(canonical) = path.canonicalize() {
        return Ok(canonical);
    }
    let mut existing = path;
    let mut suffix = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .ok_or_else(|| Error::new("host state directory cannot be resolved"))?;
        suffix.push(name.to_os_string());
        existing = existing
            .parent()
            .ok_or_else(|| Error::new("host state directory cannot be resolved"))?;
    }
    let mut resolved = existing
        .canonicalize()
        .map_err(|_| Error::new("host state directory cannot be resolved"))?;
    for component in suffix.into_iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

struct AcceptanceLock(std::fs::File);

impl AcceptanceLock {
    fn acquire(directory: &Path) -> Result<Self> {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(directory.join("lock"))
            .map_err(|_| invalid())?;
        let metadata = file.metadata().map_err(|_| invalid())?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(invalid());
        }
        // SAFETY: flock is tied to the validated descriptor and released by Drop.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(invalid());
        }
        Ok(Self(file))
    }
}

impl Drop for AcceptanceLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: this is the descriptor acquired by AcceptanceLock::acquire.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn read_acceptance(path: &Path, identity: &str) -> Result<Option<Acceptance>> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(invalid()),
    };
    let metadata = file.metadata().map_err(|_| invalid())?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.len() > MAX_BYTES
    {
        return Err(invalid());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    let acceptance: Acceptance = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    if acceptance.schema_version != 1
        || acceptance.repo_identity != identity
        || acceptance.fingerprints.len() > 64
        || acceptance.fingerprints.iter().any(|(path, digest)| {
            path != ".claude/settings.local.json"
                || digest.len() != 64
                || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return Err(invalid());
    }
    Ok(Some(acceptance))
}

/// Is this user's accepted local context identical to the current snapshot?
/// No store is required when no local-only input currently affects the agent.
pub(crate) fn is_current(
    repo: &Repo,
    fingerprints: &BTreeMap<String, String>,
    state_home: Option<&Path>,
) -> Result<bool> {
    if fingerprints.is_empty()
        && std::env::var_os("XDG_STATE_HOME").is_none()
        && std::env::var_os("HOME").is_none()
        && state_home.is_none()
    {
        return Ok(true);
    }
    let home = match state_home {
        Some(path) => path.to_path_buf(),
        None => default_state_home()?,
    };
    let Some(directory) = state_dir(repo, &home, false)? else {
        return Ok(fingerprints.is_empty());
    };
    let Some(acceptance) = read_acceptance(&directory.join(FILE_NAME), &repo.identity())? else {
        return Ok(fingerprints.is_empty());
    };
    Ok(&acceptance.fingerprints == fingerprints)
}

/// Accept current local inputs after the user explicitly runs `ahu lock --update`.
pub(crate) fn accept(
    repo: &Repo,
    fingerprints: &BTreeMap<String, String>,
    state_home: Option<&Path>,
) -> Result<()> {
    if fingerprints.is_empty()
        && std::env::var_os("XDG_STATE_HOME").is_none()
        && std::env::var_os("HOME").is_none()
        && state_home.is_none()
    {
        return Ok(());
    }
    let home = match state_home {
        Some(path) => path.to_path_buf(),
        None => default_state_home()?,
    };
    let directory = if fingerprints.is_empty() {
        match state_dir(repo, &home, false)? {
            None => return Ok(()),
            Some(directory) => directory,
        }
    } else {
        state_dir(repo, &home, true)?.ok_or_else(invalid)?
    };
    let _lock = AcceptanceLock::acquire(&directory)?;
    let path = directory.join(FILE_NAME);
    // Refuse malformed, redirected, or non-owner state rather than replacing it.
    let existing = read_acceptance(&path, &repo.identity())?;
    if fingerprints.is_empty() && existing.is_none() {
        return Ok(());
    }
    let value = Acceptance {
        schema_version: 1,
        repo_identity: repo.identity(),
        fingerprints: fingerprints.clone(),
    };
    let bytes = serde_json::to_vec_pretty(&value).map_err(|_| invalid())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid());
    }
    crate::private_io::atomic_write(&path, &bytes, crate::private_io::Durability::Durable)
        .map_err(|_| invalid())
}

/// Establish a first-run baseline during explicit setup, but never overwrite an
/// existing user's acceptance. Later changes require `ahu lock --update`.
pub(crate) fn initialize_if_missing(
    repo: &Repo,
    fingerprints: &BTreeMap<String, String>,
) -> Result<()> {
    initialize_if_missing_at(repo, fingerprints, &default_state_home()?)
}

fn initialize_if_missing_at(
    repo: &Repo,
    fingerprints: &BTreeMap<String, String>,
    home: &Path,
) -> Result<()> {
    if fingerprints.is_empty() {
        return Ok(());
    }
    let directory = state_dir(repo, home, true)?.ok_or_else(invalid)?;
    let _lock = AcceptanceLock::acquire(&directory)?;
    if read_acceptance(&directory.join(FILE_NAME), &repo.identity())?.is_none() {
        let value = Acceptance {
            schema_version: 1,
            repo_identity: repo.identity(),
            fingerprints: fingerprints.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&value).map_err(|_| invalid())?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(invalid());
        }
        crate::private_io::atomic_write(
            &directory.join(FILE_NAME),
            &bytes,
            crate::private_io::Durability::Durable,
        )
        .map_err(|_| invalid())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn users_accept_local_context_independently_without_touching_the_shared_checkout() {
        let checkout = tempfile::tempdir().unwrap();
        git_cmd(checkout.path(), &["init", "-q", "-b", "main"]);
        git_cmd(checkout.path(), &["config", "user.name", "ahu tests"]);
        git_cmd(
            checkout.path(),
            &["config", "user.email", "tests@example.invalid"],
        );
        std::fs::write(checkout.path().join("README.md"), "fixture\n").unwrap();
        git_cmd(checkout.path(), &["add", "README.md"]);
        git_cmd(checkout.path(), &["commit", "-q", "-m", "base"]);
        let repo = crate::git::discover(checkout.path()).unwrap();

        let alice_state = tempfile::tempdir().unwrap();
        let bob_state = tempfile::tempdir().unwrap();
        let alice_home = alice_state.path().canonicalize().unwrap();
        let bob_home = bob_state.path().canonicalize().unwrap();
        let alice = BTreeMap::from([(".claude/settings.local.json".into(), "a".repeat(64))]);
        let bob = BTreeMap::from([(".claude/settings.local.json".into(), "b".repeat(64))]);

        assert!(!is_current(&repo, &alice, Some(&alice_home)).unwrap());
        accept(&repo, &alice, Some(&alice_home)).unwrap();
        assert!(is_current(&repo, &alice, Some(&alice_home)).unwrap());
        let alice_directory = state_dir(&repo, &alice_home, false).unwrap().unwrap();
        assert!(!alice_directory.starts_with(checkout.path()));
        let acceptance_bytes = std::fs::read(alice_directory.join(FILE_NAME)).unwrap();
        let acceptance_text = String::from_utf8(acceptance_bytes).unwrap();
        assert!(!acceptance_text.contains("privateNote"));
        assert!(!acceptance_text.contains("enabledMcpjsonServers"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&alice_directory)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(alice_directory.join(FILE_NAME))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert!(!is_current(&repo, &bob, Some(&bob_home)).unwrap());
        accept(&repo, &bob, Some(&bob_home)).unwrap();
        assert!(is_current(&repo, &bob, Some(&bob_home)).unwrap());
        assert!(is_current(&repo, &alice, Some(&alice_home)).unwrap());
        assert!(
            crate::git::run(checkout.path(), &["status", "--porcelain"])
                .unwrap()
                .stdout
                .is_empty()
        );
    }

    #[test]
    fn setup_only_initializes_acceptance_and_does_not_accept_later_changes() {
        let checkout = tempfile::tempdir().unwrap();
        git_cmd(checkout.path(), &["init", "-q", "-b", "main"]);
        git_cmd(checkout.path(), &["config", "user.name", "ahu tests"]);
        git_cmd(
            checkout.path(),
            &["config", "user.email", "tests@example.invalid"],
        );
        std::fs::write(checkout.path().join("README.md"), "fixture\n").unwrap();
        git_cmd(checkout.path(), &["add", "README.md"]);
        git_cmd(checkout.path(), &["commit", "-q", "-m", "base"]);
        let repo = crate::git::discover(checkout.path()).unwrap();
        let state = tempfile::tempdir().unwrap();
        let home = state.path().canonicalize().unwrap();
        let initial = BTreeMap::from([(".claude/settings.local.json".into(), "a".repeat(64))]);
        let changed = BTreeMap::from([(".claude/settings.local.json".into(), "b".repeat(64))]);

        initialize_if_missing_at(&repo, &initial, &home).unwrap();
        assert!(is_current(&repo, &initial, Some(&home)).unwrap());
        initialize_if_missing_at(&repo, &changed, &home).unwrap();
        assert!(!is_current(&repo, &changed, Some(&home)).unwrap());
        accept(&repo, &changed, Some(&home)).unwrap();
        assert!(is_current(&repo, &changed, Some(&home)).unwrap());
    }
}

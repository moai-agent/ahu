//! Repository path resolution that refuses to traverse symlinks.

use std::path::{Path, PathBuf};

use super::{Error, Result};

/// Resolve `relative` under `root`, refusing to traverse a symlink.
///
/// Every path ahu writes to, deletes, or creates inside a repository goes
/// through this. A repository can commit a symlink at any path it likes — at
/// `.agents/ahu/agents`, at `.claude`, at `.worktrees` — and following one lets
/// the repository choose where ahu's filesystem operations land. Checking only
/// the final component is not enough: the escape is usually a parent.
///
/// `create_missing_dirs` creates intermediate directories as real directories
/// when they are absent; otherwise a missing component is simply reported back
/// through the returned path, which the caller may or may not require to exist.
pub fn resolve_within(root: &Path, relative: &str, create_missing_dirs: bool) -> Result<PathBuf> {
    let parts: Vec<&str> = relative.split('/').filter(|p| !p.is_empty()).collect();
    let Some((last, directories)) = parts.split_last() else {
        return Err(Error::new(format!("empty path under {}", root.display())));
    };
    if parts.iter().any(|p| *p == ".." || *p == ".") {
        return Err(Error::new(format!(
            "{relative} is not a plain path under {}",
            root.display()
        )));
    }
    let mut current = root.to_path_buf();
    for directory in directories {
        current.push(directory);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(symlink_refusal(&current, relative));
            }
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(Error::new(format!(
                    "{} exists and is not a directory.",
                    current.display()
                )));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if !create_missing_dirs {
                    return Err(Error::new(format!("{} does not exist.", current.display())));
                }
                std::fs::create_dir(&current)
                    .map_err(|e| Error::new(format!("cannot create {}: {e}", current.display())))?;
            }
            Err(e) => {
                return Err(Error::new(format!(
                    "cannot inspect {}: {e}",
                    current.display()
                )));
            }
        }
    }
    current.push(last);
    if let Ok(meta) = std::fs::symlink_metadata(&current)
        && meta.file_type().is_symlink()
    {
        return Err(symlink_refusal(&current, relative));
    }
    Ok(current)
}

/// Resolve `relative` under `root` for *reading*, refusing to traverse a symlink.
///
/// [`resolve_within`] guards every path ahu writes to. Reads need the same
/// guard for the same reason — a repository can commit a symlink at
/// `.agents/ahu/config.toml` or `.agents/ahu/agents/x.md`, and reading
/// through one lets the repository choose which file ahu opens and then quotes
/// back in a parse error. That is an arbitrary-file read with the contents
/// disclosed, from nothing more than checking out a repository.
///
/// Reads differ from writes in one way: a component that does not exist is the
/// ordinary "not configured yet" case, not a failure. So absence is reported as
/// `Ok(None)` and only a symlink, or a non-directory where a directory must be,
/// is an error.
///
/// A symlink swapped in between this call and the open is still possible. The
/// committed-symlink case — the one a repository controls — is closed here;
/// winning that race additionally requires write access to the checkout while
/// ahu is running.
pub fn resolve_existing_within(root: &Path, relative: &str) -> Result<Option<PathBuf>> {
    let parts: Vec<&str> = relative.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err(Error::new(format!("empty path under {}", root.display())));
    }
    if parts.iter().any(|p| *p == ".." || *p == ".") {
        return Err(Error::new(format!(
            "{relative} is not a plain path under {}",
            root.display()
        )));
    }
    let mut current = root.to_path_buf();
    for (index, part) in parts.iter().enumerate() {
        current.push(part);
        let last = index + 1 == parts.len();
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(symlink_refusal(&current, relative));
            }
            Ok(meta) if last || meta.is_dir() => {}
            Ok(_) => {
                return Err(Error::new(format!(
                    "{} exists and is not a directory.",
                    current.display()
                )));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(Error::new(format!(
                    "cannot inspect {}: {e}",
                    current.display()
                )));
            }
        }
    }
    Ok(Some(current))
}

/// The message shown when ahu refuses to act through a symlink.
pub fn symlink_refusal(found_at: &Path, relative: &str) -> Error {
    let points_to = std::fs::read_link(found_at)
        .map(|t| t.to_string_lossy().to_string())
        .unwrap_or_else(|_| "an unreadable target".to_string());
    Error::new(format!(
        "refusing to act through a symlink.\n\
         {} is a symlink pointing at {points_to}, and ahu was about to use it for {relative}.\n\
         Following it would let this repository choose where ahu reads, writes, or deletes, so \
         nothing was done. Inspect that path in the repository before trying again.",
        found_at.display()
    ))
}

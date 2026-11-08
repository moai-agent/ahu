//! Bounded native reads. Home aliases must be canonicalized by the caller.
use crate::{Error, Result};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::{
    fs,
    io::Read,
    path::{Component, Path},
};
pub(crate) const LIMIT: usize = 1024 * 1024;
pub(crate) fn check_components(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err(Error::invalid());
    }
    let mut cursor = std::path::PathBuf::new();
    for part in path.components() {
        if matches!(part, Component::ParentDir | Component::CurDir) {
            return Err(Error::invalid());
        }
        cursor.push(part);
        match fs::symlink_metadata(&cursor) {
            Ok(m) if m.file_type().is_symlink() => return Err(Error::invalid()),
            Ok(_) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(Error::invalid()),
        }
    }
    Ok(())
}
pub(crate) fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    check_components(path)?;
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Error::invalid()),
    };
    let m = file.metadata()?;
    // SAFETY: geteuid has no arguments or pointer preconditions.
    if !m.is_file()
        || m.nlink() != 1
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o022 != 0
        || m.len() > LIMIT as u64
    {
        return Err(Error::invalid());
    }
    let mut bytes = Vec::new();
    file.take(LIMIT as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > LIMIT {
        return Err(Error::invalid());
    }
    Ok(Some(bytes))
}

/// Recognized project inputs only. Detects changes, never claims prevention.
pub(crate) fn snapshot(project: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    let mut count = 0usize;
    let mut total = 0usize;
    fn visit(
        path: &Path,
        depth: usize,
        count: &mut usize,
        total: &mut usize,
        hash: &mut Sha256,
    ) -> Result<()> {
        *count += 1;
        if *count > 8192 || depth > 16 {
            return Err(Error::invalid());
        }
        hash.update(path.as_os_str().as_encoded_bytes());
        let meta = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                hash.update(b"absent");
                return Ok(());
            }
            Err(_) => return Err(Error::invalid()),
        };
        if meta.file_type().is_symlink() {
            return Err(Error::invalid());
        }
        hash.update(meta.mode().to_le_bytes());
        if meta.is_dir() {
            let mut children = Vec::new();
            for entry in fs::read_dir(path)? {
                children.push(entry?.path());
                if children.len() > 8192 {
                    return Err(Error::invalid());
                }
            }
            children.sort();
            for child in children {
                visit(&child, depth + 1, count, total, hash)?;
            }
        } else {
            let bytes = read(path)?.ok_or_else(Error::invalid)?;
            *total += bytes.len();
            if *total > 32 * LIMIT {
                return Err(Error::invalid());
            }
            hash.update(bytes);
        }
        Ok(())
    }
    for relative in [
        ".agents",
        ".claude",
        ".codex",
        ".opencode",
        ".kiro",
        ".kiroignore",
        "AGENTS.md",
        "AGENTS.override.md",
        "CLAUDE.md",
        "GEMINI.md",
        ".mcp.json",
        "opencode.json",
        "opencode.jsonc",
    ] {
        visit(
            &project.join(relative),
            0,
            &mut count,
            &mut total,
            &mut hasher,
        )?;
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub(crate) fn private_scope(root: &Path, path: &Path) -> Result<()> {
    check_components(path)?;
    let relative = path.strip_prefix(root).map_err(|_| Error::invalid())?;
    let mut cursor = root.to_path_buf();
    for part in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(part) = part {
            cursor.push(part);
        }
        match fs::symlink_metadata(&cursor) {
            Ok(m) => {
                // SAFETY: geteuid has no arguments or pointer preconditions.
                if m.uid() != unsafe { libc::geteuid() }
                    || m.mode() & 0o022 != 0
                    || m.file_type().is_symlink()
                {
                    return Err(Error::invalid());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(Error::invalid()),
        }
    }
    Ok(())
}

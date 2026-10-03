//! Private fingerprints for native MCP configuration owned by a user.
//!
//! Contents and absolute paths never enter the committed project lock.

use crate::util::{Error, Result, digest_bytes};
use std::path::{Path, PathBuf};

pub(crate) const ANTIGRAVITY_KEY: &str = "user:antigravity-mcp";
const MAX_BYTES: u64 = 1024 * 1024;

pub(crate) fn antigravity_config_path() -> Result<Option<PathBuf>> {
    let Some(home) = std::env::var_os("HOME") else {
        return Ok(None);
    };
    let home = PathBuf::from(home);
    if !home.is_absolute() {
        return Err(invalid());
    }
    Ok(Some(home.join(".gemini/config/mcp_config.json")))
}

fn invalid() -> Error {
    Error::new(
        "native Antigravity MCP settings are unsafe or unavailable; review the native configuration before accepting context",
    )
}

/// Read a bounded regular file without following the native config directories
/// or leaf through symlinks. HOME itself may use normal OS aliases.
pub(crate) fn read_antigravity_config(path: &Path) -> Result<Option<Vec<u8>>> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let Some(home) = path.parent().and_then(Path::parent).and_then(Path::parent) else {
        return Err(invalid());
    };
    if path != home.join(".gemini/config/mcp_config.json")
        || !home.is_absolute()
        || path
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(invalid());
    }
    let mut cursor = home.canonicalize().map_err(|_| invalid())?;
    for component in [".gemini", "config"] {
        cursor.push(component);
        let metadata = match std::fs::symlink_metadata(&cursor) {
            Ok(value) => value,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(invalid()),
        };
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o022 != 0
        {
            return Err(invalid());
        }
    }
    cursor.push("mcp_config.json");
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(cursor)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(invalid()),
    };
    let meta = file.metadata().map_err(|_| invalid())?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o022 != 0
        || meta.len() > MAX_BYTES
    {
        return Err(invalid());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid());
    }
    Ok(Some(bytes))
}

pub(crate) fn fingerprint_antigravity() -> Result<Option<String>> {
    match antigravity_config_path()? {
        Some(path) => Ok(read_antigravity_config(&path)?.map(|bytes| digest_bytes(&bytes))),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn native_input_is_bounded_private_and_never_follows_links() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".gemini/config/mcp_config.json");
        assert!(read_antigravity_config(&path).unwrap().is_none());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = br#"{"mcpServers":{"ahu":{"command":"synthetic-ahu"}}}"#;
        std::fs::write(&path, original).unwrap();
        assert_eq!(read_antigravity_config(&path).unwrap().unwrap(), original);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(read_antigravity_config(&path).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(&path, vec![0; MAX_BYTES as usize + 1]).unwrap();
        assert!(read_antigravity_config(&path).is_err());
        std::fs::remove_file(&path).unwrap();
        let external = home.path().join("external");
        std::fs::write(&external, original).unwrap();
        symlink(&external, &path).unwrap();
        assert!(read_antigravity_config(&path).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(path.parent().unwrap()).unwrap();
        symlink(home.path(), path.parent().unwrap()).unwrap();
        assert!(read_antigravity_config(&path).is_err());
    }
}

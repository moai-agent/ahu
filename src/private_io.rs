//! Atomic replacement mechanics. Callers validate their own storage boundary.

use crate::util::{Error, Result};
use std::io::Write;
use std::path::Path;

#[derive(Clone, Copy)]
pub(crate) enum Durability {
    Atomic,
    Durable,
}

pub(crate) fn atomic_write(path: &Path, body: &[u8], durability: Durability) -> Result<()> {
    atomic_write_with(path, durability, |file| file.write_all(body))
}

/// Pin the parent for creation, replacement, cleanup, and (when requested) fsync.
/// Confinement above this directory remains the caller's policy.
pub(crate) fn atomic_write_with(
    path: &Path,
    durability: Durability,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> Result<()> {
    use std::os::unix::{
        ffi::OsStrExt,
        fs::{OpenOptionsExt, PermissionsExt},
        io::{AsRawFd, FromRawFd},
    };
    let parent = path
        .parent()
        .ok_or_else(|| Error::new("state file needs a parent"))?;
    let name = std::ffi::CString::new(
        path.file_name()
            .ok_or_else(|| Error::new("state file needs a name"))?
            .as_bytes(),
    )
    .map_err(|_| Error::new("state file name contains NUL"))?;
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(parent)
        .map_err(|e| crate::state::state_io_error("open parent directory", parent, e))?;
    let temp =
        std::ffi::CString::new(format!(".write-{}", crate::orchestration::new_nonce()?)).unwrap();
    // SAFETY: the parent descriptor is open and the temporary name is NUL terminated.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            temp.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(crate::state::state_io_error(
            "create temporary file",
            path,
            std::io::Error::last_os_error(),
        ));
    }
    // SAFETY: openat returned a fresh descriptor whose ownership is transferred here.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let result = (|| -> std::io::Result<()> {
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        write(&mut file)?;
        if matches!(durability, Durability::Durable) {
            file.sync_all()?;
        }
        // The caller checked the destination with its own confinement policy.
        // renameat replaces the entry itself and never follows a destination link.
        // SAFETY: both names are valid and relative to the owned directory descriptor.
        if unsafe {
            libc::renameat(
                directory.as_raw_fd(),
                temp.as_ptr(),
                directory.as_raw_fd(),
                name.as_ptr(),
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        if matches!(durability, Durability::Durable) {
            directory.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        // SAFETY: remove only the temporary entry in the pinned parent, never a target.
        unsafe {
            libc::unlinkat(directory.as_raw_fd(), temp.as_ptr(), 0);
        }
    }
    result.map_err(|e| crate::state::state_io_error("replace file", path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn both_policies_replace_owner_only_and_preserve_previous_file_on_write_failure() {
        for policy in [Durability::Atomic, Durability::Durable] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("record.json");
            atomic_write(&path, b"original", policy).unwrap();
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert!(
                atomic_write_with(&path, policy, |file| {
                    file.write_all(b"partial")?;
                    Err(std::io::Error::other("injected failure"))
                })
                .is_err()
            );
            assert_eq!(std::fs::read(&path).unwrap(), b"original");
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
            atomic_write(&path, b"replacement", policy).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
        }
    }

    #[test]
    fn failed_write_cleanup_uses_pinned_parent_after_directory_swap() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("parent");
        let moved = dir.path().join("moved");
        let outside = dir.path().join("outside");
        std::fs::create_dir(&parent).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let mut target = None;
        let result = atomic_write_with(&parent.join("record"), Durability::Durable, |_| {
            let name = std::fs::read_dir(&parent)?.next().unwrap()?.file_name();
            let sentinel = outside.join(name);
            std::fs::write(&sentinel, b"keep")?;
            target = Some(sentinel);
            std::fs::rename(&parent, &moved)?;
            symlink(&outside, &parent)?;
            Err(std::io::Error::other("injected failure"))
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(target.unwrap()).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(&moved).unwrap().count(), 0);
    }

    #[test]
    fn concurrent_replacements_do_not_share_temporary_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("record");
        std::thread::scope(|scope| {
            for byte in 0..16 {
                let path = &path;
                scope.spawn(move || {
                    atomic_write(path, &vec![byte; 8192], Durability::Atomic).unwrap()
                });
            }
        });
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(bytes.len(), 8192);
        assert!(bytes.iter().all(|b| *b == bytes[0]));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

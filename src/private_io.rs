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

/// Publish a private file only after all of its contents have been written.
/// Returns `false` when the destination already exists and leaves it intact.
pub(crate) fn atomic_create(path: &Path, body: &[u8], durability: Durability) -> Result<bool> {
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
    let temp = std::ffi::CString::new(format!(".create-{}", crate::orchestration::new_nonce()?))
        .expect("nonce contains no NUL bytes");
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
    let result = (|| -> std::io::Result<bool> {
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(body)?;
        file.sync_all()?;

        rename_noreplace(&directory, &temp, &name)
    })();

    if !matches!(&result, Ok(true)) {
        // A successful rename consumed temp: do not unlink a name that may
        // since have been reused. On failure, never remove the destination.
        // SAFETY: temp is our created entry, relative to the still-pinned dir.
        let cleanup = unsafe { libc::unlinkat(directory.as_raw_fd(), temp.as_ptr(), 0) };
        if cleanup != 0 && matches!(&result, Ok(false)) {
            return Err(crate::state::state_io_error(
                "remove temporary file",
                path,
                std::io::Error::last_os_error(),
            ));
        }
    }
    if matches!(&result, Ok(true)) && matches!(durability, Durability::Durable) {
        directory
            .sync_all()
            .map_err(|e| crate::state::state_io_error("sync parent directory", parent, e))?;
    }
    result.map_err(|e| crate::state::state_io_error("create file", path, e))
}

/// Move a completed temporary entry to an absent destination in the pinned dir.
/// Callers supply distinct leaf names. Success consumes the source name without
/// a hardlink's visible nlink=2 window; an existing destination is never replaced.
fn rename_noreplace(
    directory: &std::fs::File,
    source: &std::ffi::CStr,
    destination: &std::ffi::CStr,
) -> std::io::Result<bool> {
    #[cfg(any(
        target_os = "macos",
        all(target_os = "linux", any(target_env = "gnu", target_env = "musl"))
    ))]
    {
        use std::os::unix::io::AsRawFd;

        #[cfg(target_os = "linux")]
        // SAFETY: the directory stays open and both C strings stay valid for
        // the call. RENAME_NOREPLACE makes collision detection and publication
        // one operation on the pinned directory, without following the target.
        let status = unsafe {
            libc::renameat2(
                directory.as_raw_fd(),
                source.as_ptr(),
                directory.as_raw_fd(),
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(target_os = "macos")]
        // SAFETY: the same descriptor/name lifetimes apply; RENAME_EXCL is
        // macOS's atomic no-replace operation, including symlink collisions.
        let status = unsafe {
            libc::renameatx_np(
                directory.as_raw_fd(),
                source.as_ptr(),
                directory.as_raw_fd(),
                destination.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        if status == 0 {
            Ok(true)
        } else {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(false)
            } else {
                // Unsupported kernels/filesystems fail closed. Neither a
                // check-then-rename nor a hardlink is a safe fallback here.
                Err(error)
            }
        }
    }
    #[cfg(not(any(
        target_os = "macos",
        all(target_os = "linux", any(target_env = "gnu", target_env = "musl"))
    )))]
    {
        let _ = (directory, source, destination);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic no-replace rename is unsupported on this platform",
        ))
    }
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
    #[cfg(any(
        target_os = "macos",
        all(target_os = "linux", any(target_env = "gnu", target_env = "musl"))
    ))]
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
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

    #[cfg(any(
        target_os = "macos",
        all(target_os = "linux", any(target_env = "gnu", target_env = "musl"))
    ))]
    #[test]
    fn atomic_create_publishes_complete_owner_only_contents_without_replacing() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("binding.json");
        assert!(atomic_create(&path, b"complete binding", Durability::Durable).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"complete binding");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        assert!(!atomic_create(&path, b"replacement", Durability::Durable).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"complete binding");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(any(
        target_os = "macos",
        all(target_os = "linux", any(target_env = "gnu", target_env = "musl"))
    ))]
    #[test]
    fn concurrent_atomic_creates_publish_one_complete_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("binding.json");
        let created = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..16)
                .map(|byte| {
                    let path = &path;
                    scope.spawn(move || {
                        let created =
                            atomic_create(path, &vec![byte; 8192], Durability::Durable).unwrap();
                        assert_eq!(std::fs::symlink_metadata(path).unwrap().nlink(), 1);
                        created
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .filter(|created| *created)
                .count()
        });

        assert_eq!(created, 1);
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(bytes.len(), 8192);
        assert!(bytes.iter().all(|byte| *byte == bytes[0]));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(any(
        target_os = "macos",
        all(target_os = "linux", any(target_env = "gnu", target_env = "musl"))
    ))]
    #[test]
    fn publication_immediately_has_one_link_and_consumes_source_name() {
        let dir = tempfile::tempdir().unwrap();
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(dir.path())
            .unwrap();
        let source = dir.path().join("source");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&source)
            .unwrap();
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .unwrap();
        file.write_all(b"complete private contents").unwrap();
        file.sync_all().unwrap();

        assert!(rename_noreplace(&directory, c"source", c"published").unwrap());
        // Check at the publication boundary, with no unlink/cleanup in between.
        // Substituting linkat for the rename deterministically yields nlink=2.
        assert_eq!(file.metadata().unwrap().nlink(), 1);
        assert_eq!(
            std::fs::symlink_metadata(&source).unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        let published = dir.path().join("published");
        let metadata = std::fs::symlink_metadata(&published).unwrap();
        assert_eq!(metadata.ino(), file.metadata().unwrap().ino());
        assert_eq!(metadata.nlink(), 1);
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::read(published).unwrap(),
            b"complete private contents"
        );
    }

    #[cfg(any(
        target_os = "macos",
        all(target_os = "linux", any(target_env = "gnu", target_env = "musl"))
    ))]
    #[test]
    fn publication_collisions_preserve_source_and_every_destination_type() {
        for kind in ["file", "symlink", "dangling", "directory", "nonempty"] {
            let dir = tempfile::tempdir().unwrap();
            let source = dir.path().join("source");
            let destination = dir.path().join("destination");
            let referent = dir.path().join("referent");
            std::fs::write(&source, b"new contents").unwrap();
            std::fs::write(&referent, b"keep referent").unwrap();
            match kind {
                "file" => std::fs::write(&destination, b"keep destination").unwrap(),
                "symlink" => symlink(&referent, &destination).unwrap(),
                "dangling" => symlink(dir.path().join("missing"), &destination).unwrap(),
                _ => {
                    std::fs::create_dir(&destination).unwrap();
                    if kind == "nonempty" {
                        std::fs::write(destination.join("child"), b"keep child").unwrap();
                    }
                }
            }
            let directory = std::fs::File::open(dir.path()).unwrap();
            let before = std::fs::symlink_metadata(&destination).unwrap();
            assert!(!rename_noreplace(&directory, c"source", c"destination").unwrap());
            assert_eq!(std::fs::read(&source).unwrap(), b"new contents");
            assert_eq!(std::fs::symlink_metadata(&source).unwrap().nlink(), 1);
            for policy in [Durability::Atomic, Durability::Durable] {
                assert!(!atomic_create(&destination, b"replacement", policy).unwrap());
                // The unpublished temporary file is cleaned up, not the
                // destination, unrelated source, or a symlink's referent.
                assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3);
                assert_eq!(
                    std::fs::symlink_metadata(&destination).unwrap().ino(),
                    before.ino()
                );
                assert_eq!(std::fs::read(&source).unwrap(), b"new contents");
                assert_eq!(std::fs::read(&referent).unwrap(), b"keep referent");
                match kind {
                    "file" => assert_eq!(std::fs::read(&destination).unwrap(), b"keep destination"),
                    "symlink" => assert_eq!(std::fs::read_link(&destination).unwrap(), referent),
                    "dangling" => assert_eq!(
                        std::fs::read_link(&destination).unwrap(),
                        dir.path().join("missing")
                    ),
                    "nonempty" => assert_eq!(
                        std::fs::read(destination.join("child")).unwrap(),
                        b"keep child"
                    ),
                    _ => assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 0),
                }
            }
        }
    }

    #[cfg(any(
        target_os = "macos",
        all(target_os = "linux", any(target_env = "gnu", target_env = "musl"))
    ))]
    #[test]
    fn publication_uses_pinned_directory_after_path_swap() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("parent");
        let moved = dir.path().join("moved");
        let outside = dir.path().join("outside");
        std::fs::create_dir(&parent).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(parent.join("source"), b"publish").unwrap();
        std::fs::write(outside.join("source"), b"keep source").unwrap();
        std::fs::write(outside.join("destination"), b"keep destination").unwrap();
        let directory = std::fs::File::open(&parent).unwrap();
        std::fs::rename(&parent, &moved).unwrap();
        symlink(&outside, &parent).unwrap();

        assert!(rename_noreplace(&directory, c"source", c"destination").unwrap());
        assert_eq!(
            std::fs::read(moved.join("destination")).unwrap(),
            b"publish"
        );
        assert_eq!(std::fs::read_dir(&moved).unwrap().count(), 1);
        assert_eq!(
            std::fs::read(outside.join("source")).unwrap(),
            b"keep source"
        );
        assert_eq!(
            std::fs::read(outside.join("destination")).unwrap(),
            b"keep destination"
        );
    }

    #[test]
    fn failed_atomic_create_cleans_only_its_own_temporary_entry() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join(".create-unrelated");
        std::fs::write(&sentinel, b"keep").unwrap();
        // The temporary file can be created, but the destination leaf exceeds
        // Linux/macOS NAME_MAX, so publication fails rather than colliding.
        let invalid = dir.path().join("x".repeat(4096));
        assert!(atomic_create(&invalid, b"private", Durability::Durable).is_err());
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[cfg(not(any(
        target_os = "macos",
        all(target_os = "linux", any(target_env = "gnu", target_env = "musl"))
    )))]
    #[test]
    fn unsupported_publication_leaves_source_and_destination_intact() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("source"), b"source").unwrap();
        std::fs::write(dir.path().join("destination"), b"destination").unwrap();
        let directory = std::fs::File::open(dir.path()).unwrap();
        assert_eq!(
            rename_noreplace(&directory, c"source", c"destination")
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::Unsupported
        );
        assert_eq!(std::fs::read(dir.path().join("source")).unwrap(), b"source");
        assert_eq!(
            std::fs::read(dir.path().join("destination")).unwrap(),
            b"destination"
        );
        assert!(atomic_create(&dir.path().join("absent"), b"new", Durability::Durable).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }
}

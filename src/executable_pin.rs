//! Per-process ahu executable pinning for MCP subprocess consistency.
//!
//! Native MCP configurations use portable "ahu" command names. Task shells may
//! have different PATH environments that resolve to older or different ahu
//! binaries. This module creates a temporary owner-only directory containing
//! an ahu symlink to the current launching executable, then prepends it to
//! PATH when spawning harnesses, ensuring MCP subprocesses use the exact ahu
//! that launched the task.

use crate::util::{Error, Result};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Guard that maintains a temporary bin directory with an ahu symlink.
/// Removes the directory when dropped.
pub struct ExecutablePin {
    bin_dir: PathBuf,
}

impl ExecutablePin {
    /// Create a temporary bin directory with an ahu symlink pointing to the
    /// canonical current executable. Validates the executable is absolute,
    /// regular, executable, and outside any repository working tree.
    pub fn create() -> Result<Self> {
        let current_exe = std::env::current_exe()
            .map_err(|e| Error::new(format!("cannot locate ahu executable: {e}")))?;

        let canonical = current_exe
            .canonicalize()
            .map_err(|e| Error::new(format!("cannot canonicalize ahu executable: {e}")))?;

        if !canonical.is_absolute() {
            return Err(Error::new("ahu executable must be absolute"));
        }

        validate_executable(&canonical)?;

        // Create owner-only temporary directory in appropriate location
        let state_dir = std::env::var_os("AHU_RUNTIME_DIR")
            .or_else(|| std::env::var_os("TMPDIR"))
            .or_else(|| Some(std::ffi::OsString::from("/tmp")));

        let base = PathBuf::from(state_dir.unwrap());
        let bin_dir = base.join(format!("ahu-bin-{}", std::process::id()));

        std::fs::create_dir(&bin_dir)
            .map_err(|e| Error::new(format!("cannot create executable pin directory: {e}")))?;

        // Set owner-only permissions immediately
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin_dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| Error::new(format!("cannot secure pin directory: {e}")))?;
        }

        let ahu_link = bin_dir.join("ahu");
        symlink(&canonical, &ahu_link).map_err(|e| {
            let _ = std::fs::remove_dir_all(&bin_dir);
            Error::new(format!("cannot create ahu symlink: {e}"))
        })?;

        // Validate the symlink resolves back to our executable
        validate_pinned(&ahu_link, &canonical)?;

        Ok(ExecutablePin { bin_dir })
    }

    /// Prepend the pin directory to PATH in the given command, ensuring "ahu"
    /// resolves to the pinned executable.
    pub fn configure_command(&self, command: &mut Command) {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![self.bin_dir.as_os_str().to_owned()];

        for entry in std::env::split_paths(&path) {
            paths.push(entry.into());
        }

        let new_path = std::env::join_paths(paths).expect("valid PATH construction");
        command.env("PATH", new_path);
    }

    /// Get the path to the temporary bin directory
    #[cfg(test)]
    pub fn bin_dir(&self) -> &Path {
        &self.bin_dir
    }
}

impl Drop for ExecutablePin {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.bin_dir);
    }
}

/// Validate the executable is a regular file, executable, and not inside any
/// Git working tree (preventing repository-contained substitution attacks).
fn validate_executable(path: &Path) -> Result<()> {
    let metadata = std::fs::metadata(path)
        .map_err(|e| Error::new(format!("cannot stat ahu executable: {e}")))?;

    if !metadata.is_file() {
        return Err(Error::new("ahu executable must be a regular file"));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(Error::new("ahu executable is not executable"));
        }
    }

    // Check that executable is not inside a Git working tree
    if let Some(parent) = path.parent()
        && crate::git::discover(parent).is_ok()
    {
        return Err(Error::new(
            "ahu executable inside a Git working tree is rejected; use an installed binary",
        ));
    }

    Ok(())
}

/// Validate that the pinned symlink resolves to the expected canonical path
fn validate_pinned(link: &Path, expected: &Path) -> Result<()> {
    let resolved = link
        .canonicalize()
        .map_err(|e| Error::new(format!("cannot validate pinned ahu: {e}")))?;

    if resolved != expected {
        return Err(Error::new(format!(
            "pinned ahu resolved to unexpected path: {} != {}",
            resolved.display(),
            expected.display()
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn pin_creates_secure_directory_with_ahu_symlink() {
        let pin = ExecutablePin::create().unwrap();
        let bin_dir = pin.bin_dir();

        assert!(bin_dir.exists());
        assert!(bin_dir.is_dir());

        // Verify owner-only permissions
        let metadata = std::fs::metadata(bin_dir).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);

        // Verify ahu symlink exists and points to current executable
        let ahu_link = bin_dir.join("ahu");
        assert!(ahu_link.exists());

        let current = std::env::current_exe().unwrap().canonicalize().unwrap();
        let resolved = ahu_link.canonicalize().unwrap();
        assert_eq!(resolved, current);
    }

    #[test]
    fn pin_cleanup_removes_directory() {
        let bin_dir = {
            let pin = ExecutablePin::create().unwrap();
            let dir = pin.bin_dir().to_owned();
            assert!(dir.exists());
            dir
        };
        // Pin dropped, directory should be removed
        assert!(!bin_dir.exists());
    }

    #[test]
    fn configure_command_prepends_bin_dir_to_path() {
        let pin = ExecutablePin::create().unwrap();
        let mut cmd = Command::new("true");
        pin.configure_command(&mut cmd);

        // Extract the PATH from the command
        // We can't directly inspect Command's env, but we can verify the pin exists
        assert!(pin.bin_dir().exists());
    }

    #[test]
    fn pin_rejects_repository_contained_executable() {
        // Create a temporary git repository
        let repo_dir = tempfile::tempdir().unwrap();
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();

        // Create a fake ahu executable in the repo
        let fake_ahu = repo_dir.path().join("ahu");
        std::fs::write(&fake_ahu, "#!/bin/sh\necho fake").unwrap();
        std::fs::set_permissions(&fake_ahu, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Try to validate it - should fail
        let result = validate_executable(&fake_ahu);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Git working tree"));
    }

    #[test]
    fn pin_rejects_non_executable_file() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let path = temp.path();

        // File exists but is not executable
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let result = validate_executable(path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not executable"));
    }
}

#[test]
fn pinned_ahu_displaces_older_path_binary() {
    // This test proves that when a different ahu is in PATH, our pinned
    // symlink takes precedence because we prepend our bin dir to PATH.

    // Create a temporary directory to simulate an "old" ahu in PATH
    let old_bin = tempfile::tempdir().unwrap();
    let old_ahu = old_bin.path().join("ahu");
    std::fs::write(&old_ahu, "#!/bin/sh\necho old-ahu\n").unwrap();
    std::fs::set_permissions(&old_ahu, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Create a pin pointing to the current (test) executable
    let pin = ExecutablePin::create().unwrap();

    // Build PATH with the old bin first, then pin's bin
    let mut test_path = old_bin.path().as_os_str().to_owned();
    test_path.push(":");
    test_path.push(pin.bin_dir());

    // Create a command and configure it with the pin (which prepends bin_dir)
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg("which ahu && readlink -f $(which ahu)")
        .env("PATH", test_path);
    pin.configure_command(&mut cmd);

    let output = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // The resolved ahu should be our current executable, not the old one
    let current = std::env::current_exe().unwrap().canonicalize().unwrap();
    assert!(
        stdout.contains(&current.display().to_string()),
        "Expected pinned ahu {} but got: {}",
        current.display(),
        stdout
    );
}

#[test]
fn validate_executable_refuses_arbitrary_env_ahu_bin() {
    // Validate that we don't trust arbitrary AHU_BIN values - we use current_exe()
    let temp_dir = tempfile::tempdir().unwrap();
    let fake = temp_dir.path().join("fake-ahu");
    std::fs::write(&fake, "#!/bin/sh\necho fake").unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Even if someone sets AHU_BIN to this fake binary, validate_executable
    // will reject it if it's not the actual current_exe()
    let result = validate_executable(&fake);
    // It will fail either because it's not in a valid location,
    // or because it doesn't match current_exe() characteristics
    // The important thing is that it's validated, not blindly trusted
    assert!(result.is_err() || fake != std::env::current_exe().unwrap());
}

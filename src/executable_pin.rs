//! Resolve portable MCP command `ahu` to the task-launching executable.
//!
//! The private PATH entry lasts through the harness process. Native clients
//! with an explicit server path or their own PATH override still require live
//! verification. This is not an immutable binary snapshot or a sandbox.

use crate::util::{Error, Result};
use std::ffi::{CString, OsStr, OsString};
use std::os::unix::{ffi::OsStringExt, fs::MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct ExecutablePin {
    bin_dir: PathBuf,
    executable: PathBuf,
}

impl ExecutablePin {
    pub fn create() -> Result<Self> {
        // Trust the controller the operator actually executed, never AHU_BIN or
        // a repository/PATH candidate. This also supports cargo-run binaries.
        Self::for_executable(&std::env::current_exe()?, &std::env::temp_dir())
    }

    fn for_executable(executable: &Path, base: &Path) -> Result<Self> {
        let executable = executable.canonicalize()?;
        let metadata = std::fs::metadata(&executable)?;
        if !metadata.is_file() || metadata.mode() & 0o111 == 0 {
            return Err(Error::new(
                "the current ahu executable must be an executable regular file",
            ));
        }
        let base = base.canonicalize()?;
        validate_temporary_base(&base)?;
        // mkdtemp creates a unique 0700 directory atomically. PID-only names
        // collide on repeated launches and parallel tests in the same process.
        let template = CString::new(base.join("ahu-mcp-XXXXXX").into_os_string().into_vec())
            .map_err(|_| Error::new("invalid temporary directory for MCP executable pin"))?;
        let mut bytes = template.into_bytes_with_nul();
        // SAFETY: writable, NUL-terminated template ends in the six Xs required
        // by mkdtemp; its pointer stays valid for the duration of the call.
        if unsafe { libc::mkdtemp(bytes.as_mut_ptr().cast()) }.is_null() {
            return Err(Error::new(format!(
                "cannot create private MCP executable directory: {}",
                std::io::Error::last_os_error()
            )));
        }
        bytes.pop();
        let pin = Self {
            bin_dir: PathBuf::from(OsString::from_vec(bytes)),
            executable,
        };
        // From here the guard also cleans up constructor errors.
        std::os::unix::fs::symlink(&pin.executable, pin.bin_dir.join("ahu"))?;
        Ok(pin)
    }

    pub fn configure_command(&self, command: &mut Command) -> Result<()> {
        let inherited = command
            .get_envs()
            .find(|(key, _)| *key == OsStr::new("PATH"))
            .map(|(_, value)| value.map(OsStr::to_os_string))
            .unwrap_or_else(|| std::env::var_os("PATH"))
            .unwrap_or_default();
        let paths = std::iter::once(self.bin_dir.clone()).chain(std::env::split_paths(&inherited));
        let path = std::env::join_paths(paths)
            .map_err(|_| Error::new("MCP executable directory cannot be represented in PATH"))?;
        command.env("PATH", path).env("AHU_BIN", &self.executable);
        Ok(())
    }
}

fn validate_temporary_base(base: &Path) -> Result<()> {
    // Canonicalization permits standard /tmp and /var aliases, but a private
    // executable directory must not be created in a checkout or unsafe parent.
    for path in base.ancestors() {
        if std::fs::symlink_metadata(path.join(".git")).is_ok() {
            return Err(Error::new(
                "MCP executable temporary storage must be outside Git checkouts",
            ));
        }
        let metadata = std::fs::metadata(path)?;
        // SAFETY: geteuid has no preconditions.
        let trusted_owner = metadata.uid() == 0 || metadata.uid() == unsafe { libc::geteuid() };
        let protected_shared_directory = metadata.mode() & 0o1000 != 0;
        if !metadata.is_dir()
            || !trusted_owner
            || (metadata.mode() & 0o022 != 0 && !protected_shared_directory)
        {
            return Err(Error::new(
                "MCP executable temporary storage has an unsafe parent directory",
            ));
        }
    }
    Ok(())
}

impl Drop for ExecutablePin {
    fn drop(&mut self) {
        // Remove only the entry we created. Never recursively delete additional
        // files that appeared in this directory after it was handed to a client.
        let _ = std::fs::remove_file(self.bin_dir.join("ahu"));
        let _ = std::fs::remove_dir(&self.bin_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn executable(dir: &Path, name: &str, output: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' '{output}'\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn repeated_pins_are_private_unique_and_cleaned_up() {
        let first = ExecutablePin::create().unwrap();
        let second = ExecutablePin::create().unwrap();
        assert_ne!(first.bin_dir, second.bin_dir);
        assert_eq!(
            std::fs::metadata(&first.bin_dir).unwrap().mode() & 0o777,
            0o700
        );
        assert_eq!(
            first.bin_dir.join("ahu").canonicalize().unwrap(),
            std::env::current_exe().unwrap().canonicalize().unwrap()
        );
        let path = first.bin_dir.clone();
        drop(first);
        assert!(!path.exists());
        assert!(second.bin_dir.exists());
    }

    #[test]
    fn native_child_uses_selected_ahu_and_retains_its_other_path_tools() {
        let temp = tempfile::tempdir().unwrap();
        let selected = executable(temp.path(), "renamed-release-runner", "selected");
        let older = temp.path().join("old-bin");
        std::fs::create_dir(&older).unwrap();
        let old_ahu = executable(&older, "ahu", "old");
        executable(&older, "fixture-helper", "helper");
        let pin = ExecutablePin::for_executable(&selected, temp.path()).unwrap();
        let mut child = Command::new("/bin/sh");
        child
            .args(["-c", "ahu; fixture-helper; \"$AHU_BIN\""])
            .env("PATH", &older)
            .env("AHU_BIN", old_ahu);
        pin.configure_command(&mut child).unwrap();
        let output = child.output().unwrap();
        assert!(output.status.success(), "{:?}", output.stderr);
        assert_eq!(output.stdout, b"selected\nhelper\nselected\n");
    }

    #[test]
    fn unsafe_storage_and_invalid_executables_fail_without_repository_writes() {
        let temp = tempfile::tempdir().unwrap();
        let selected = executable(temp.path(), "runner", "ok");
        let repo = temp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        std::fs::write(repo.join(".git"), "gitdir: fixture").unwrap();
        assert!(ExecutablePin::for_executable(&selected, &repo).is_err());
        assert_eq!(std::fs::read_dir(&repo).unwrap().count(), 1);
        let unsafe_dir = temp.path().join("unsafe");
        std::fs::create_dir(&unsafe_dir).unwrap();
        std::fs::set_permissions(&unsafe_dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(ExecutablePin::for_executable(&selected, &unsafe_dir).is_err());
        assert!(ExecutablePin::for_executable(temp.path(), temp.path()).is_err());
        std::fs::set_permissions(&selected, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(ExecutablePin::for_executable(&selected, temp.path()).is_err());
    }

    #[test]
    fn path_encoding_errors_are_returned_and_cleanup_is_not_recursive() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("colon:directory");
        std::fs::create_dir(&base).unwrap();
        let selected = executable(temp.path(), "runner", "ok");
        let pin = ExecutablePin::for_executable(&selected, &base).unwrap();
        assert!(pin.configure_command(&mut Command::new("/bin/sh")).is_err());
        let path = pin.bin_dir.clone();
        std::fs::write(path.join("preserve"), "unexpected file").unwrap();
        drop(pin);
        assert!(!path.join("ahu").exists());
        assert_eq!(
            std::fs::read_to_string(path.join("preserve")).unwrap(),
            "unexpected file"
        );
    }
}

//! Native integration checks remain in core even without the optional session plugin.
pub mod integration;
pub use crate::interactive::repository;
pub use crate::interactive::v1::{CreatedWorkspace, Group, WorkspaceInfo};
pub use crate::interactive::{startup_command, workspace_identity, workspace_title};
use crate::util::{Error, Result};
use std::path::{Path, PathBuf};

#[cfg(feature = "cmux-plugin")]
mod host;

#[cfg(feature = "cmux-plugin")]
pub struct Cmux(ahu_cmux_plugin::Cmux);

#[cfg(feature = "cmux-plugin")]
impl Cmux {
    pub fn discover() -> Result<Self> {
        crate::interactive::plugins::require_cmux()?;
        Self::connect_recorded()
    }

    /// Legacy task records bind cmux independently of selection for new tasks.
    pub(crate) fn connect_recorded() -> Result<Self> {
        Ok(Self(ahu_cmux_plugin::Cmux::connect(resolve_executable()?)?))
    }
}

#[cfg(feature = "cmux-plugin")]
impl std::ops::Deref for Cmux {
    type Target = ahu_cmux_plugin::Cmux;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Resolve the native CLI independently of application/socket reachability.
pub fn resolve_executable() -> Result<PathBuf> {
    match std::env::var_os("AHU_CMUX_BIN") {
        Some(path) => {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                Ok(path.canonicalize().unwrap_or(path))
            } else if path.as_os_str().as_encoded_bytes().contains(&b'/') {
                let path = std::env::current_dir()?.join(path);
                Ok(path.canonicalize().unwrap_or(path))
            } else {
                resolve_explicit_name(&path)
            }
        }
        None => crate::selection::resolve_utility("cmux").map_err(|error| {
            Error::new(format!(
                "{error}\nInstall cmux, or set AHU_CMUX_BIN to its executable."
            ))
            .with_kind(crate::util::ErrorKind::Prerequisite)
        }),
    }
}

/// Explicit user selection retains PATH semantics, including relative entries
/// and repository-local commands. The implicit resolver remains restricted.
fn resolve_explicit_name(name: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let cwd = std::env::current_dir()?;
    let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
    for directory in std::env::split_paths(&path) {
        let candidate = cwd.join(directory).join(name);
        if std::fs::metadata(&candidate)
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        {
            return candidate.canonicalize().map_err(Error::from);
        }
    }
    Err(Error::new(
        "explicit AHU_CMUX_BIN command was not found on PATH",
    ))
}

#[cfg(test)]
mod presentation_tests {
    use super::*;
    #[test]
    fn task_startup_command_quotes_both_owned_paths() {
        let command = startup_command(Path::new("/tmp/ahu agent"), Path::new("/tmp/task's dir"));
        assert_eq!(
            command,
            "'/tmp/ahu agent' run-task --task-dir '/tmp/task'\\''s dir'"
        );
    }

    #[test]
    fn title_budget_belongs_to_the_task() {
        assert_eq!(
            workspace_title("agent@1.2.3", "## **Fix parser**"),
            "Fix parser"
        );
        let title = workspace_title(&"agent".repeat(100), &"界🦀".repeat(100));
        assert_eq!(title.chars().count(), 60);
        assert!(title.ends_with('…'));
        assert!(!title.contains("agent"));
        assert_eq!(workspace_identity("sable", "model-a"), "sable · model-a");
    }
}

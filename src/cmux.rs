//! cmux support for coordinator sessions.
//!
//! When the `cmux` CLI is installed outside every Git working tree, a coordinator
//! started with `ahu claude`, `ahu codex` and the other harness shortcuts gets
//! whatever cmux offers for that harness: cmux's own launch wrapper where it
//! ships one (Claude Code, Codex), and cmux's native hook integration where it
//! does not (OpenCode, Kiro, Antigravity). ahu keeps no cmux workspace, session
//! or group state, and delegated tasks never involve cmux.
//!
//! cmux's wrappers locate the real harness through `PATH` unless told otherwise,
//! so the launch passes the executable ahu already vetted through the wrappers'
//! documented override variables.

use crate::util::{Error, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const HOOKS_TIMEOUT: Duration = Duration::from_secs(30);

/// How cmux participates in one coordinator launch.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Launch {
    /// The program to exec instead of the harness executable.
    pub program: Option<PathBuf>,
    /// Variables telling the wrapper which real executable to run.
    pub env: Vec<(&'static str, PathBuf)>,
    /// What ahu did on cmux's behalf, for the user.
    pub notes: Vec<String>,
}

/// An installed cmux CLI, resolved outside Git working trees.
#[derive(Debug)]
pub(crate) struct Integration {
    cli: PathBuf,
}

/// cmux's wrapper script name and the variable that names the real executable.
fn wrapper(harness: &str) -> Option<(&'static str, &'static str)> {
    match harness {
        "claude-code" => Some(("cmux-claude-wrapper", "CMUX_CUSTOM_CLAUDE_PATH")),
        "codex" => Some(("cmux-codex-wrapper", "CMUX_CUSTOM_CODEX_PATH")),
        _ => None,
    }
}

/// The agent name `cmux hooks setup --agent` uses, for harnesses with no wrapper.
fn hook_agent(harness: &str) -> Option<&'static str> {
    match harness {
        "opencode" => Some("opencode"),
        "kiro" => Some("kiro"),
        "antigravity" => Some("antigravity"),
        _ => None,
    }
}

impl Integration {
    pub(crate) fn detect() -> Option<Self> {
        let cli = crate::selection::resolve_executable("cmux")?;
        Some(Self {
            cli: PathBuf::from(cli),
        })
    }

    /// Decide the launch for `harness`, running cmux's hook installer first when
    /// the harness has no wrapper. A failure is reported, never fatal: the
    /// coordinator must start with or without cmux's help.
    pub(crate) fn prepare(&self, harness: &str, executable: &Path) -> Launch {
        let mut launch = Launch::default();
        if let Some((name, variable)) = wrapper(harness) {
            match self.wrapper_path(name) {
                Ok(path) => {
                    launch.program = Some(path);
                    launch.env.push((variable, executable.to_path_buf()));
                    launch
                        .notes
                        .push(format!("cmux: launching {harness} through {name}"));
                }
                Err(error) => launch
                    .notes
                    .push(format!("cmux: {error}; launching {harness} directly")),
            }
        } else if let Some(agent) = hook_agent(harness) {
            match self.setup_hooks(agent) {
                Ok(()) => launch
                    .notes
                    .push(format!("cmux: hooks are set up for {agent}")),
                Err(error) => launch
                    .notes
                    .push(format!("cmux: hook setup for {agent} failed: {error}")),
            }
        }
        launch
    }

    /// cmux ships its wrappers beside its CLI. Resolve the CLI's real location
    /// and require a regular executable file that is not inside a repository.
    fn wrapper_path(&self, name: &str) -> Result<PathBuf> {
        let real = self.cli.canonicalize()?;
        let candidate = real
            .parent()
            .ok_or_else(|| Error::new("cmux CLI has no containing directory"))?
            .join(name);
        let metadata = std::fs::metadata(&candidate)
            .map_err(|_| Error::new(format!("cmux does not ship {name}")))?;
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
        };
        if !executable {
            return Err(Error::new(format!("{name} is not an executable file")));
        }
        if crate::selection::is_excluded(&candidate) {
            return Err(Error::new(format!("{name} is inside a Git working tree")));
        }
        Ok(candidate)
    }

    /// `cmux hooks setup --agent <agent> --yes`. cmux owns what it writes; ahu
    /// only asks, with a bounded wait and no inherited stdin.
    fn setup_hooks(&self, agent: &str) -> Result<()> {
        let mut child = Command::new(&self.cli)
            .args(["hooks", "setup", "--agent", agent, "--yes"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Error::new(format!("cannot start cmux: {e}")))?;
        let deadline = Instant::now() + HOOKS_TIMEOUT;
        loop {
            match child.try_wait()? {
                Some(status) if status.success() => return Ok(()),
                Some(status) => return Err(Error::new(format!("cmux exited with {status}"))),
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Error::new("cmux did not finish within 30 seconds"));
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fake(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn harnesses_map_to_wrappers_or_hooks_but_never_both() {
        for harness in ["claude-code", "codex"] {
            assert!(wrapper(harness).is_some() && hook_agent(harness).is_none());
        }
        for harness in ["opencode", "kiro", "antigravity"] {
            assert!(wrapper(harness).is_none() && hook_agent(harness).is_some());
        }
        assert!(wrapper("other").is_none() && hook_agent("other").is_none());
    }

    #[test]
    fn a_wrapper_launch_names_the_vetted_real_executable() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake(dir.path(), "cmux", "#!/bin/sh\n");
        let wrapper_file = fake(dir.path(), "cmux-codex-wrapper", "#!/bin/sh\n");
        let integration = Integration { cli };
        let real = Path::new("/opt/real/codex");
        let launch = integration.prepare("codex", real);
        assert_eq!(
            launch.program.as_deref(),
            Some(wrapper_file.canonicalize().unwrap().as_path())
        );
        assert_eq!(
            launch.env,
            vec![("CMUX_CUSTOM_CODEX_PATH", real.to_path_buf())]
        );
    }

    #[test]
    fn a_missing_wrapper_falls_back_to_the_direct_launch() {
        let dir = tempfile::tempdir().unwrap();
        let cli = fake(dir.path(), "cmux", "#!/bin/sh\n");
        let launch = Integration { cli }.prepare("claude-code", Path::new("/opt/claude"));
        assert!(launch.program.is_none() && launch.env.is_empty());
        assert!(
            launch.notes[0].contains("launching claude-code directly"),
            "{:?}",
            launch.notes
        );
    }

    #[test]
    fn hook_setup_runs_cmux_with_the_agent_and_reports_failure_without_failing() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("args");
        let cli = fake(
            dir.path(),
            "cmux",
            &format!("#!/bin/sh\necho \"$@\" > {}\n", record.display()),
        );
        let launch = Integration { cli }.prepare("kiro", Path::new("/opt/kiro"));
        assert!(launch.program.is_none());
        assert_eq!(
            std::fs::read_to_string(record).unwrap().trim(),
            "hooks setup --agent kiro --yes"
        );
        assert!(launch.notes[0].contains("hooks are set up for kiro"));

        let cli = fake(dir.path(), "cmux", "#!/bin/sh\nexit 3\n");
        let launch = Integration { cli }.prepare("opencode", Path::new("/opt/opencode"));
        assert!(launch.notes[0].contains("failed"), "{:?}", launch.notes);
    }
}

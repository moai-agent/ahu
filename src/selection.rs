//! Automatic harness/model selection for launches without a named agent.
//!
//! Selection walks the project-agreed rankings deterministically. Local
//! prerequisites are checked *after* the pair is resolved: a missing
//! installation on one machine is a diagnostic for that user, never a different
//! selection. Changing what a project selects requires changing the project's
//! configuration, not the machine it runs on.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bail;
use crate::catalog;
use crate::config::LoadedConfig;
use crate::util::{Error, Result};

/// The frozen harness/model choice for one task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedPair {
    pub harness: String,
    pub model: String,
    /// Why this pair, in terms a reviewer can check against the configuration.
    pub basis: String,
    pub policy_digest: String,
    pub catalog_version: String,
}

/// Whether the harness this pair names is actually usable on this machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Prerequisite {
    pub executable: String,
    pub found_at: Option<String>,
    pub version: Option<String>,
    /// Non-fatal notes, such as a missing or unparseable installed CLI version.
    pub notes: Vec<String>,
}

impl Prerequisite {
    pub fn satisfied(&self) -> bool {
        self.found_at.is_some()
    }
}

/// Resolve the project's preferred pair from its rankings.
pub fn resolve_automatic(loaded: &LoadedConfig) -> Result<ResolvedPair> {
    let config = &loaded.config;
    let mut rejected: Vec<String> = Vec::new();
    for harness_id in &config.harness_preferences {
        let Some(harness) = catalog::harness(harness_id) else {
            rejected.push(format!(
                "{harness_id}: not in catalog {}",
                config.catalog_version
            ));
            continue;
        };
        if !harness.adapter_available {
            rejected.push(format!("{harness_id}: ahu has no validated adapter"));
            continue;
        }
        let ranked = ranked_models(loaded, harness_id);
        let Some(model) = ranked.first() else {
            rejected.push(format!(
                "{harness_id}: no model ranked for it in model_rankings"
            ));
            continue;
        };
        let position = config
            .harness_preferences
            .iter()
            .position(|h| h == harness_id)
            .unwrap_or(0)
            + 1;
        return Ok(ResolvedPair {
            harness: harness_id.clone(),
            model: model.clone(),
            basis: format!(
                "harness_preferences #{position} with a validated adapter; \
                 highest-ranked model for it in model_rankings"
            ),
            policy_digest: loaded.digest.clone(),
            catalog_version: config.catalog_version.clone(),
        });
    }
    bail!(
        "no harness in this project's agreed order can be launched by ahu.\n{}\n\
         This is a project policy question, not a per-machine one: change the order in {} \
         or install an ahu release with the adapter you need.",
        rejected
            .iter()
            .map(|r| format!("  - {r}"))
            .collect::<Vec<_>>()
            .join("\n"),
        loaded.path.display()
    );
}

/// The project's explicit model order for a harness. Missing or empty rankings
/// return no models; automatic selection does not fall back to catalog order.
pub fn ranked_models(loaded: &LoadedConfig, harness_id: &str) -> Vec<String> {
    if let Some(ranked) = loaded.config.model_rankings.get(harness_id)
        && !ranked.is_empty()
    {
        return ranked.clone();
    }
    Vec::new()
}

/// Probe the local machine for a harness executable.
///
/// The result never changes a selection. It exists so a user whose machine
/// cannot run the project's choice gets an accurate diagnostic.
pub fn check_prerequisite(harness_id: &str) -> Prerequisite {
    let entry = catalog::harness(harness_id);
    let executable = entry
        .map(|e| e.executable)
        .unwrap_or(harness_id)
        .to_string();
    let found_at = resolve_executable(&executable);
    let mut notes = Vec::new();
    // Probe the resolved absolute path so version checks obey the same
    // repository and relative-PATH exclusions as actual launches.
    let version = found_at.as_deref().and_then(probe_version);
    if found_at.is_some()
        && version
            .as_deref()
            .and_then(catalog::version_token)
            .is_none()
    {
        notes.push(format!(
            "installed {executable} did not report a parseable semantic version; Ahu cannot record or pin this CLI"
        ));
    }
    Prerequisite {
        executable,
        found_at,
        version,
        notes,
    }
}

/// Repositories a harness binary must never be resolved from.
///
/// A harness resolved from inside the tree an agent is about to edit is exactly
/// the case worth refusing, and the refusal has to apply to *every* invocation,
/// including the `--version` probes that run before a preview is printed. Those
/// probes have no repository argument to check against -- `enforcement()` is
/// called from the adapters, which know nothing about the checkout -- so the
/// exclusion lives here, registered once by [`crate::git::discover`] for every
/// repository ahu opens in this process.
static EXCLUDED_ROOTS: std::sync::LazyLock<std::sync::RwLock<Vec<PathBuf>>> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(Vec::new()));

/// Refuse to resolve any harness executable from inside `root`.
///
/// Idempotent, and it stores the canonical path so a later comparison is not
/// defeated by a symlinked PATH entry.
pub fn exclude_root(root: &Path) {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let Ok(mut roots) = EXCLUDED_ROOTS.write() else {
        return;
    };
    if !roots.contains(&canonical) {
        roots.push(canonical);
    }
}

/// Whether `candidate` lies inside a registered excluded root.
pub fn is_excluded(candidate: &Path) -> bool {
    let resolved = candidate
        .canonicalize()
        .unwrap_or_else(|_| candidate.to_path_buf());
    let Ok(roots) = EXCLUDED_ROOTS.read() else {
        // A poisoned lock must not silently turn the exclusion off.
        return true;
    };
    roots.iter().any(|root| resolved.starts_with(root))
}

/// Resolve an executable through `PATH`, returning its absolute path.
///
/// This is the only place ahu turns a harness program name into something it
/// will run, and every harness invocation goes through it -- launches, the
/// `run-task` exec, and the `--version` probes alike. It refuses relative `PATH`
/// entries and any candidate inside a repository ahu has opened.
pub fn resolve_executable(executable: &str) -> Option<String> {
    which(executable, Some).map(|path| path.to_string_lossy().into_owned())
}

/// Bootstrap utilities without Git, including candidates in unopened worktrees.
pub(crate) fn resolve_utility(executable: &str) -> Result<PathBuf> {
    which(executable, |candidate| {
        let candidate = candidate.canonicalize().ok()?;
        if is_excluded(&candidate) {
            return None;
        }
        // Inspect markers only; never follow .git or parse its untrusted pointers.
        // Bound ancestry work and reject candidates whose ancestry cannot be checked.
        for (depth, ancestor) in candidate.parent()?.ancestors().enumerate() {
            if depth >= 256 {
                return None;
            }
            match std::fs::symlink_metadata(ancestor.join(".git")) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return None,
            }
        }
        Some(candidate)
    })
    .ok_or_else(|| {
        Error::new(format!(
            "{executable} was not found on PATH outside Git working trees."
        ))
    })
}

/// Ask a resolved harness binary for its version.
///
/// Takes an already-resolved absolute path, so a caller cannot accidentally
/// re-run the PATH lookup this module exists to constrain.
pub fn probe_version(resolved: &str) -> Option<String> {
    let path = Path::new(resolved);
    if !path.is_absolute() || is_excluded(path) {
        return None;
    }
    probe_explicit_utility_version(path)
}

/// Bounded probe for an explicitly selected utility, after its caller pins the
/// executable. This does not change the implicit harness/utility exclusions.
pub(crate) fn probe_explicit_utility_version(path: &Path) -> Option<String> {
    if !path.is_absolute() {
        return None;
    }
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let mut child = std::process::Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let pipe = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let ok = pipe.take(65537).read_to_end(&mut bytes).is_ok() && bytes.len() <= 65536;
        let _ = tx.send(ok.then_some(bytes));
    });
    let pid = child.id();
    let deadline = Instant::now() + Duration::from_secs(5);
    let exited = loop {
        match crate::headless::child_exited(pid) {
            Ok(true) => break true,
            Err(_) => break false,
            Ok(false) if Instant::now() >= deadline => break false,
            Ok(false) => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    // SAFETY: child is not reaped yet, so this process group id cannot be reused.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    let status = child.wait().ok()?;
    if !exited || !status.success() {
        return None;
    }
    let bytes = rx.recv_timeout(Duration::from_millis(100)).ok()??;
    Some(String::from_utf8_lossy(&bytes).trim().to_string())
}

/// Resolve `program` and ask it for its version, for an enforcement report.
pub fn installed_version(program: &str) -> Option<String> {
    probe_version(&resolve_executable(program)?)
}

fn which(executable: &str, accept: impl Fn(PathBuf) -> Option<PathBuf>) -> Option<PathBuf> {
    // A program name carrying a path separator is not a PATH lookup at all; it
    // would be resolved against the current directory, which for `ahu run-task`
    // is the task worktree.
    if executable.is_empty() || executable.contains('/') || executable.contains('\\') {
        return None;
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        // A `PATH` entry that is empty or relative resolves against the current
        // working directory, which for `ahu run-task` is the task worktree — a
        // checkout of the repository. `PATH=/usr/bin:` is enough: the empty
        // trailing entry makes `dir` empty, `dir.join("claude")` is the bare
        // relative name `claude`, and a `claude` committed at the top of the
        // repository is what gets executed. The name check in `run_task` cannot
        // catch it, because the file name of `claude` is `claude`.
        //
        // ahu resolves a harness to run it, so it only ever accepts an absolute
        // path. A relative `PATH` entry is skipped rather than treated as the
        // shell would treat it.
        if !dir.is_absolute() {
            continue;
        }
        let candidate = dir.join(executable);
        if !is_executable(&candidate) {
            continue;
        }
        // A PATH entry can be an absolute path *into* the checkout, or hold a
        // symlink pointing into it. Both are refused here rather than only at
        // exec time, so a probe cannot run what a launch would reject.
        if is_excluded(&candidate) {
            continue;
        }
        if let Some(candidate) = accept(candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Check native configuration without inference or changing the selected model.
/// Call on the submitting checkout, then again on the materialized worktree
/// immediately before execution: provider configuration and Codex trust are
/// directory-dependent. This does not establish entitlement or inference access.
pub fn check_launch_compatibility(
    executable: &Path,
    harness: &str,
    model: &str,
    permissions: crate::agent::Permissions,
    cwd: &Path,
) -> Result<()> {
    check_launch_compatibility_with_policy(executable, harness, model, permissions, cwd, None)
}

pub(crate) fn check_launch_compatibility_with_policy(
    executable: &Path,
    harness: &str,
    model: &str,
    permissions: crate::agent::Permissions,
    cwd: &Path,
    policy: Option<&crate::cmux::integration::HeadlessPolicy>,
) -> Result<()> {
    use crate::agent::Permissions;
    let (args, capture) = match harness {
        "codex" if permissions == Permissions::Auto => {
            (crate::harness::codex::auto_mcp_probe_args(), false)
        }
        "opencode" => {
            // Validate before using the provider as a positional CLI argument.
            crate::harness::model_args(harness, model)?;
            let provider = model.split_once('/').expect("validated model").0;
            (vec!["models".into(), provider.into()], true)
        }
        _ => return Ok(()),
    };
    if !executable.is_absolute() || is_excluded(executable) {
        bail!("compatibility probe requires a resolved harness outside the repository");
    }
    // Native config loading may migrate files even for a listing command.
    // Refuse newly changed project context before executing an agent.
    let before = crate::snapshot::collect(cwd)?.digest();
    let mut command = std::process::Command::new(executable);
    command.args(&args).current_dir(cwd);
    if let Some(policy) = policy {
        crate::cmux::integration::sanitize(&mut command, Some(policy));
    }
    let output = bounded_config_probe(&mut command, capture, std::time::Duration::from_secs(8));
    if crate::snapshot::collect(cwd)?.digest() != before {
        bail!(
            "native compatibility inspection changed project agent context; review the changes, \
             run `ahu lock --update`, commit the context and lock, then retry"
        );
    }
    if harness == "codex" {
        if output.is_none() {
            bail!(
                "Codex cannot load its effective MCP configuration with ahu's auto tool approvals. \
                 An untrusted project may hide .codex/config.toml, leaving an ahu server with tools \
                 but no transport. Review project trust in Codex directly in this checkout and verify \
                 `codex mcp list` recognizes the intended ahu server, then retry. A trusted project \
                 transport is sufficient; global registration is not required. ahu will not \
                 grant trust, copy a project command into CLI overrides, or remove MCP approval gates. \
                 The native config probe failed or timed out; its output is withheld."
            );
        }
    } else {
        let Some(output) = output else {
            bail!(
                "OpenCode model availability could not be verified: the bounded `opencode models \
                 <provider>` probe failed or timed out. Run it in this checkout and repair native \
                 provider configuration before retrying; ahu refuses a possible model fallback. \
                 Native diagnostics are withheld."
            );
        };
        if !output.lines().any(|line| line.trim() == model) {
            bail!(
                "OpenCode did not list the exact requested model {model:?} in this checkout. \
                 Configure and enable that provider/model in OpenCode, verify it with \
                 `opencode models <provider>`, then retry. ahu refuses a possible model fallback."
            );
        }
    }
    Ok(())
}

/// Bound elapsed time, captured bytes and pipe lifetime. Config diagnostics can
/// contain secrets, so stderr is never captured and callers never echo stdout.
/// The Codex check discards stdout too; only the OpenCode model list is read.
fn bounded_config_probe(
    command: &mut std::process::Command,
    capture: bool,
    timeout: std::time::Duration,
) -> Option<String> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    command
        .stdin(Stdio::null())
        .stdout(if capture {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = command.spawn().ok()?;
    let mut reader = child.stdout.take();
    let pid = child.id();
    let deadline = Instant::now() + timeout;
    let mut bytes = Vec::new();
    let exited = (|| {
        if let Some(pipe) = &reader {
            use std::os::fd::AsRawFd;
            let fd = pipe.as_raw_fd();
            // SAFETY: pipe owns this live descriptor for the whole probe.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return false;
            }
        }
        loop {
            if Instant::now() >= deadline {
                return false;
            }
            // Drain without waiting for EOF: a detached descendant could keep
            // the pipe open after the leader exits. No reader thread survives.
            if let Some(pipe) = &mut reader {
                loop {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    let mut buffer = [0; 8192];
                    match pipe.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(n) if bytes.len() + n <= 1_048_576 => {
                            bytes.extend_from_slice(&buffer[..n])
                        }
                        Ok(_) => return false,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => return false,
                    }
                }
            }
            match crate::headless::child_exited(pid) {
                // One final drain after observing exit captures bytes written
                // between the last read and the exit observation.
                Ok(true) => {
                    if let Some(pipe) = &mut reader {
                        let mut tail = Vec::new();
                        match pipe
                            .take((1_048_577 - bytes.len()) as u64)
                            .read_to_end(&mut tail)
                        {
                            Ok(_) => (),
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => (),
                            Err(_) => return false,
                        }
                        bytes.extend(tail);
                    }
                    return bytes.len() <= 1_048_576;
                }
                Err(_) => return false,
                Ok(false) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    })();
    // SAFETY: the child has not been reaped, so the process group cannot be reused.
    // Kill descendants too: they may otherwise hold the capture pipe indefinitely.
    unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
    let status = child.wait().ok()?;
    if !exited || !status.success() {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    use crate::agent::Permissions;
    use std::os::unix::fs::PermissionsExt;

    fn fake(root: &Path, body: &str) -> PathBuf {
        let file = root.join("native-probe");
        std::fs::write(&file, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        file
    }

    #[test]
    fn opencode_requires_exact_available_model_and_uses_project_directory() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("models.txt"), "ollama/glm-5.3:cloud\n").unwrap();
        let exe = fake(
            root.path(),
            "test \"$1\" = models && test \"$2\" = ollama || exit 2\ncat models.txt",
        );
        let check = || {
            check_launch_compatibility(
                &exe,
                "opencode",
                "ollama/glm-5.3:cloud",
                Permissions::Prompt,
                &project,
            )
        };
        check().unwrap();
        for unavailable in [
            "opencode/big-pickle\n",
            "ollama/glm-5.3:cloud-other\n",
            "disabled: ollama/glm-5.3:cloud\n",
            "ollama/other\n",
            "",
        ] {
            std::fs::write(project.join("models.txt"), unavailable).unwrap();
            assert!(
                check()
                    .unwrap_err()
                    .to_string()
                    .contains("refuses a possible model fallback")
            );
        }
        let exe = fake(root.path(), "printf 'ollama/glm-5.3:cloud\\n'; exit 1");
        assert!(
            check_launch_compatibility(
                &exe,
                "opencode",
                "ollama/glm-5.3:cloud",
                Permissions::Auto,
                &project
            )
            .is_err()
        );
        assert!(
            check_launch_compatibility(
                &exe,
                "opencode",
                "--help/model",
                Permissions::Prompt,
                &project
            )
            .is_err()
        );
    }

    #[test]
    fn codex_auto_checks_transport_without_changing_trust_or_provider_approvals() {
        let root = tempfile::tempdir().unwrap();
        let args = crate::harness::codex::auto_mcp_probe_args();
        assert_eq!(&args[args.len() - 2..], ["mcp", "list"]);
        for tool in crate::harness::codex::AUTO_LOCAL_MCP_TOOLS {
            assert!(args.contains(&crate::harness::codex::auto_local_mcp_config(tool)));
        }
        assert!(!args.iter().any(|arg| arg.contains("trust")
            || arg.contains("command=")
            || arg.contains("ahu_typed_decide")
            || arg.contains("ahu_skills_suggest")));
        let exe = fake(root.path(), "test -f transport-present");
        let check = |permissions| {
            check_launch_compatibility(&exe, "codex", "gpt-6-astra", permissions, root.path())
        };
        let error = check(Permissions::Auto).unwrap_err().to_string();
        assert!(error.contains("Review project trust"));
        assert!(error.contains("no transport"));
        check(Permissions::Prompt).unwrap();
        check(Permissions::AcceptEdits).unwrap();
        std::fs::write(root.path().join("transport-present"), "").unwrap();
        check(Permissions::Auto).unwrap();
    }

    #[test]
    fn native_migration_requires_accepting_context_before_launch() {
        let root = tempfile::tempdir().unwrap();
        let exe = fake(
            root.path(),
            "printf '{}\\n' > opencode.json\nprintf 'ollama/glm-5.3:cloud\\n'",
        );
        let result = check_launch_compatibility(
            &exe,
            "opencode",
            "ollama/glm-5.3:cloud",
            Permissions::Prompt,
            root.path(),
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("inspection changed project agent context")
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("opencode.json")).unwrap(),
            "{}\n"
        );
    }

    #[test]
    fn probes_are_bounded_and_do_not_return_failed_or_oversized_output() {
        use std::time::{Duration, Instant};
        let root = tempfile::tempdir().unwrap();
        for body in [
            "printf 'synthetic diagnostic'; exit 1",
            "yes x | head -c 1048577",
        ] {
            let exe = fake(root.path(), body);
            assert!(
                bounded_config_probe(
                    &mut std::process::Command::new(exe),
                    true,
                    Duration::from_secs(2)
                )
                .is_none()
            );
        }
        let exe = fake(root.path(), "sleep 30 &\nwait");
        let start = Instant::now();
        assert!(
            bounded_config_probe(
                &mut std::process::Command::new(exe),
                true,
                Duration::from_millis(40)
            )
            .is_none()
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        let exe = fake(root.path(), "sleep 30 &\nprintf 'ollama/glm-5.3:cloud\\n'");
        assert_eq!(
            bounded_config_probe(
                &mut std::process::Command::new(exe),
                true,
                Duration::from_secs(2)
            )
            .as_deref(),
            Some("ollama/glm-5.3:cloud\n")
        );
    }
}

//! Automatic harness/model selection for launches without a named agent.
//!
//! Selection walks the project-agreed rankings deterministically. Local
//! prerequisites are checked *after* the pair is resolved: a missing
//! installation on one machine is a diagnostic for that user, never a different
//! selection. Changing what a project selects requires changing the project's
//! configuration, not the machine it runs on.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::bail;
use crate::catalog;
use crate::config::LoadedConfig;
use crate::util::Result;

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
    /// Non-fatal notes, such as a version ahu has not verified the adapter on.
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
            rejected.push(format!("{harness_id}: ahu 0.1.1 has no validated adapter"));
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
        "no harness in this project's agreed order can be launched by ahu 0.1.1.\n{}\n\
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

/// The project's model order for a harness, falling back to the catalog order
/// only when the project has ranked nothing for it.
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
    let found_at = which(&executable);
    let mut notes = Vec::new();
    let version = found_at.as_ref().and_then(|_| {
        std::process::Command::new(&executable)
            .arg("--version")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    });
    if let (Some(entry), Some(version)) = (entry, version.as_deref())
        && !entry.verified_versions.is_empty()
        && !version.contains(entry.verified_versions)
    {
        notes.push(format!(
            "installed {executable} reports {version:?}; the ahu adapter was verified against {}",
            entry.verified_versions
        ));
    }
    Prerequisite {
        executable,
        found_at,
        version,
        notes,
    }
}

/// Resolve an executable through `PATH`, returning its absolute path.
///
/// ahu resolves the harness binary once, at submission, and records the result.
/// The launch then execs that exact path instead of consulting `PATH` again in
/// the task workspace's shell.
pub fn resolve_executable(executable: &str) -> Option<String> {
    which(executable)
}

fn which(executable: &str) -> Option<String> {
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
        if is_executable(&candidate) {
            return Some(candidate.to_string_lossy().to_string());
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

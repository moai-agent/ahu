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
    which(executable)
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
    let output = std::process::Command::new(path)
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Resolve `program` and ask it for its version, for an enforcement report.
pub fn installed_version(program: &str) -> Option<String> {
    probe_version(&resolve_executable(program)?)
}

fn which(executable: &str) -> Option<String> {
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
        return Some(candidate.to_string_lossy().to_string());
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

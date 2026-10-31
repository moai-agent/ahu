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
use crate::config::LoadedConfig;
use crate::git::Repo;
use crate::util::{Error, Result};

/// The frozen harness/model choice for one task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedPair {
    pub harness: String,
    pub model: String,
    /// Why this pair, in terms a reviewer can check against the configuration.
    pub basis: String,
    pub policy_digest: String,
    pub runtime_binding_digest: String,
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
pub fn resolve_automatic(repo: &Repo, loaded: &LoadedConfig) -> Result<ResolvedPair> {
    for (index, harness) in loaded.config.harness_preferences.iter().enumerate() {
        let ranked = ranked_models(loaded, harness);
        let Some(model) = ranked.first() else {
            continue;
        };
        // Once policy names a ranked pair, missing installation is an error,
        // never permission to try a lower-ranked provider on this machine.
        let runtime = crate::harness_runtime::load(repo, harness)?;
        if runtime.descriptor().id != *harness {
            bail!("project rankings require exact harness IDs, not shortcut aliases");
        }
        if !crate::models::valid_id(model) {
            bail!("invalid exact model identity");
        }
        return Ok(ResolvedPair {
            harness: harness.clone(),
            model: model.clone(),
            basis: format!(
                "harness_preferences #{}; highest exact model in project rankings",
                index + 1
            ),
            policy_digest: loaded.digest.clone(),
            runtime_binding_digest: runtime.binding_digest(),
        });
    }
    bail!("project policy has no ranked harness/model pair; run ahu setup")
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

/// Explicit native prerequisite inspection after selecting an installed descriptor.
/// A missing native executable is diagnostic; a failed plugin probe is an error.
pub fn check_prerequisite(repo: &Repo, harness_id: &str) -> Result<Prerequisite> {
    let runtime = crate::harness_runtime::load(repo, harness_id)?;
    let executable = runtime.descriptor().program.clone();
    let found_at = resolve_executable(&executable);
    let version = if found_at.is_some() {
        Some(probe_runtime(repo, &runtime)?)
    } else {
        None
    };
    Ok(Prerequisite {
        executable,
        found_at,
        version,
        notes: Vec::new(),
    })
}

pub fn probe_runtime(repo: &Repo, runtime: &crate::harness_runtime::Runtime) -> Result<String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Probe {
        version: String,
        #[serde(rename = "compatibility")]
        _compatibility: serde_json::Value,
        #[serde(rename = "reviewed_version")]
        _reviewed_version: bool,
        #[serde(rename = "enforcement")]
        _enforcement: serde_json::Value,
    }
    let before = crate::snapshot::collect(&repo.root)?.digest();
    let result = runtime.call(
        "probe",
        crate::harness_runtime::native_params(repo, runtime.descriptor())?,
    );
    if crate::snapshot::collect(&repo.root)?.digest() != before {
        bail!("native inspection changed project context; review changes before retrying");
    }
    let response: Probe =
        serde_json::from_value(result?).map_err(|_| Error::new("invalid native probe response"))?;
    if !crate::util::is_semver(&response.version) || response.version.len() > 128 {
        bail!("invalid native version evidence");
    }
    Ok(response.version)
}

/// Repositories a harness binary must never be resolved from.
///
/// A harness resolved from inside the tree an agent is about to edit is exactly
/// the case worth refusing, and the refusal has to apply to *every* invocation,
/// including explicit native inspection parameters passed to installed runtimes.
/// The exclusion is registered by [`crate::git::discover`] for every repository
/// ahu opens in this process.
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
/// `run-task` exec and explicit runtime inspection alike. It refuses relative `PATH`
/// entries and any candidate inside a repository ahu has opened.
pub fn resolve_executable(executable: &str) -> Option<String> {
    which(executable, |candidate| {
        (!is_excluded(&candidate)).then_some(candidate)
    })
    .map(|path| path.to_string_lossy().into_owned())
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
        // trailing entry makes `dir` empty, `dir.join("native")` is the bare
        // relative name `native`, and a `native` committed at the top of the
        // repository is what gets executed. The name check in `run_task` cannot
        // catch it, because the file name of `native` is `native`.
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

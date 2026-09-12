//! The agent-configuration snapshot and how it is materialized into a task
//! worktree.
//!
//! A task worktree inherits the *complete* parent agent configuration as it
//! stands in the working tree at submission — additions, modifications, and
//! deletions, committed or not, ignored or not. It does not inherit unrelated
//! dirty source files, and it never reaches outside the repository.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bail;
use crate::util::{Error, Result, digest_bytes, digest_file};

/// Directory names whose contents are agent configuration wherever they appear.
const CONFIG_DIR_NAMES: &[&str] = &[".agents", ".claude", ".codex", ".agent"];

/// File names that are agent configuration wherever they appear.
const CONFIG_FILE_NAMES: &[&str] = &[
    "CLAUDE.md",
    "CLAUDE.local.md",
    "AGENTS.md",
    "AGENTS.override.md",
    ".mcp.json",
];

/// Directories never descended into while looking for configuration.
///
/// This keeps the scan bounded on large checkouts. It is a documented
/// limitation, not a claim of completeness: configuration buried inside one of
/// these directories is not inherited, and the inventory says so.
const SKIP_DIR_NAMES: &[&str] = &[
    ".git",
    // Task worktrees live in the repository. They are checkouts of it, so
    // descending into them would inventory a task's own copy of the
    // configuration and recurse.
    ".worktrees",
    "node_modules",
    "target",
    "dist",
    "build",
    "vendor",
    ".venv",
    "venv",
    ".next",
    ".cargo",
    ".tox",
    "__pycache__",
];

const MAX_DEPTH: usize = 12;

/// One configuration file in the snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotEntry {
    /// Repository-root-relative path, using `/` separators.
    pub path: String,
    pub digest: String,
    pub bytes: u64,
    /// Whether the file carries an executable bit.
    ///
    /// Hook scripts live in agent-configuration directories, so some of what a
    /// task worktree inherits is code the harness will run. The mode is part of
    /// the snapshot digest: a file becoming executable changes behaviour just as
    /// much as an edit to its contents.
    #[serde(default)]
    pub executable: bool,
}

/// The complete repository agent configuration at submission time.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ConfigSnapshot {
    pub entries: Vec<SnapshotEntry>,
    /// Directories the scan deliberately did not descend into, so the inventory
    /// can disclose the gap instead of implying the snapshot is exhaustive.
    pub skipped_directories: Vec<String>,
    /// Configuration paths that are symlinks. Never followed, never entries,
    /// and tracked separately so reconciliation can remove them from a task
    /// worktree instead of leaving them to be discovered by the harness.
    #[serde(default)]
    pub symlinks: Vec<String>,
}

impl ConfigSnapshot {
    /// Digest over every path and content digest, in order.
    pub fn digest(&self) -> String {
        let mut buffer = String::new();
        for entry in &self.entries {
            buffer.push_str(&entry.path);
            buffer.push(' ');
            buffer.push_str(&entry.digest);
            buffer.push(' ');
            buffer.push_str(if entry.executable { "x" } else { "-" });
            buffer.push('\n');
        }
        digest_bytes(buffer.as_bytes())
    }

    pub fn short_digest(&self) -> String {
        self.digest()[..12].to_string()
    }

    /// How many inherited files are executable, for the launch preview.
    pub fn executable_count(&self) -> usize {
        self.entries.iter().filter(|entry| entry.executable).count()
    }

    pub fn by_path(&self) -> BTreeMap<&str, &SnapshotEntry> {
        self.entries.iter().map(|e| (e.path.as_str(), e)).collect()
    }
}

fn is_config_path(relative: &Path) -> bool {
    for component in relative.components() {
        if let Component::Normal(name) = component {
            let name = name.to_string_lossy();
            if CONFIG_DIR_NAMES.iter().any(|d| *d == name) {
                return true;
            }
        }
    }
    relative
        .file_name()
        .map(|name| {
            let name = name.to_string_lossy();
            CONFIG_FILE_NAMES.iter().any(|f| *f == name)
        })
        .unwrap_or(false)
}

/// Walk `root` and collect every agent-configuration file.
pub fn collect(root: &Path) -> Result<ConfigSnapshot> {
    let mut snapshot = ConfigSnapshot::default();
    walk(root, root, 0, &mut snapshot)?;
    snapshot.entries.sort_by(|a, b| a.path.cmp(&b.path));
    snapshot.skipped_directories.sort();
    snapshot.skipped_directories.dedup();
    snapshot.symlinks.sort();
    snapshot.symlinks.dedup();
    Ok(snapshot)
}

fn walk(root: &Path, dir: &Path, depth: usize, snapshot: &mut ConfigSnapshot) -> Result<()> {
    if depth > MAX_DEPTH {
        return Ok(());
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
        Err(e) => bail!("cannot read {}: {e}", dir.display()),
    };
    let mut children: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry?;
        children.push(entry.path());
    }
    children.sort();
    for path in children {
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        // `symlink_metadata` so a link is classified by what it is, not what it
        // points at: following one could leave the repository.
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        if meta.is_dir() {
            if SKIP_DIR_NAMES.iter().any(|d| *d == name) {
                if is_config_path(relative) || CONFIG_DIR_NAMES.iter().any(|d| *d == name) {
                    snapshot
                        .skipped_directories
                        .push(to_relative_string(relative));
                }
                continue;
            }
            walk(root, &path, depth + 1, snapshot)?;
        } else if meta.is_file() {
            if !is_config_path(relative) {
                continue;
            }
            snapshot.entries.push(SnapshotEntry {
                path: to_relative_string(relative),
                digest: digest_file(&path)?,
                bytes: meta.len(),
                executable: is_executable(&meta),
            });
        } else if meta.file_type().is_symlink() && is_config_path(relative) {
            // A configuration symlink is never followed. It is recorded so it
            // can be disclosed and, in a task worktree, removed.
            snapshot.symlinks.push(to_relative_string(relative));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &std::fs::Metadata) -> bool {
    false
}

fn to_relative_string(relative: &Path) -> String {
    relative
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// What materializing a snapshot into a worktree actually changed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MaterializeReport {
    /// Paths copied in because they are new or differ from the worktree's HEAD.
    pub written: Vec<String>,
    /// Configuration present at HEAD but deleted in the parent working tree,
    /// so removed from the worktree too.
    pub removed: Vec<String>,
    /// Paths that changed in the parent between the snapshot and the copy.
    pub concurrently_modified: Vec<String>,
    /// Configuration symlinks removed from the task worktree.
    #[serde(default)]
    pub removed_symlinks: Vec<String>,
    /// Sources that became symlinks after collection and were not copied.
    #[serde(default)]
    pub refused_sources: Vec<String>,
}

impl MaterializeReport {
    pub fn is_empty(&self) -> bool {
        self.written.is_empty() && self.removed.is_empty() && self.removed_symlinks.is_empty()
    }
}

/// Copy the parent's agent configuration into a freshly created worktree.
///
/// The worktree starts at the base commit, so it already holds the committed
/// configuration. This reconciles it with the parent *working tree*: writing
/// added and modified files, and removing configuration the parent has deleted.
///
/// # Symlinks
///
/// Every write is confined to the worktree. `collect` already refuses to read
/// through a symlink; this refuses to write through one. A symlink at a
/// configuration path inside a freshly created worktree can only have come from
/// the base commit, and following it would let a repository direct ahu's writes
/// anywhere on the filesystem — so it aborts the launch and names the path
/// rather than silently replacing it.
pub fn materialize(
    parent_root: &Path,
    snapshot: &ConfigSnapshot,
    worktree_root: &Path,
) -> Result<MaterializeReport> {
    let mut report = MaterializeReport::default();
    let wanted = snapshot.by_path();

    // Remove configuration the worktree has at HEAD but the parent no longer has.
    // `collect` never descends through a symlink, so these are all real files
    // genuinely inside the worktree.
    let existing = collect(worktree_root)?;
    // A configuration symlink in the worktree can only have come from the base
    // commit. ahu never follows one, so it must not leave one behind either: the
    // harness would discover whatever it points at, including hook settings the
    // parent-side preview reported as absent.
    for link in &existing.symlinks {
        let target = worktree_root.join(link);
        std::fs::remove_file(&target)
            .map_err(|e| Error::new(format!("cannot remove symlink {}: {e}", target.display())))?;
        report.removed_symlinks.push(link.clone());
    }
    for entry in &existing.entries {
        if !wanted.contains_key(entry.path.as_str()) {
            let target = worktree_root.join(&entry.path);
            std::fs::remove_file(&target)
                .map_err(|e| Error::new(format!("cannot remove {}: {e}", target.display())))?;
            report.removed.push(entry.path.clone());
        }
    }

    for entry in &snapshot.entries {
        let source = parent_root.join(&entry.path);
        let target = safe_target(worktree_root, &entry.path)?;
        // `collect` recorded this path as a regular file. If it is a symlink
        // now, it was swapped between collection and copying -- the window in
        // which the user is reading the submission preview -- and following it
        // would read a file outside the repository into the task worktree.
        match std::fs::symlink_metadata(&source) {
            Ok(meta) if meta.file_type().is_symlink() => {
                report.concurrently_modified.push(entry.path.clone());
                report.refused_sources.push(entry.path.clone());
                continue;
            }
            Ok(_) => {}
            Err(_) => {
                report.concurrently_modified.push(entry.path.clone());
                continue;
            }
        }
        let current = match digest_file(&source) {
            Ok(digest) => digest,
            Err(_) => {
                // Deleted in the parent between snapshot and copy.
                report.concurrently_modified.push(entry.path.clone());
                continue;
            }
        };
        if current != entry.digest {
            report.concurrently_modified.push(entry.path.clone());
        }
        let already = digest_file(&target).ok();
        if already.as_deref() == Some(current.as_str()) {
            // Contents already match. The mode still might not: a hook script
            // that gained or lost its executable bit behaves differently, so
            // sync that before skipping the copy. `target` is known not to be a
            // symlink, so this cannot chmod anything outside the worktree.
            if sync_mode(&source, &target)? {
                report.written.push(entry.path.clone());
            }
            continue;
        }
        std::fs::copy(&source, &target).map_err(|e| {
            Error::new(format!(
                "cannot copy {} into the task worktree: {e}",
                entry.path
            ))
        })?;
        report.written.push(entry.path.clone());
    }
    Ok(report)
}

/// Resolve `relative` inside `worktree_root`, refusing to traverse or write
/// through a symlink, and creating missing parent directories.
fn safe_target(worktree_root: &Path, relative: &str) -> Result<PathBuf> {
    crate::util::resolve_within(worktree_root, relative, true)
}

/// Copy `source`'s permission bits onto `target` when they differ.
///
/// Returns whether anything changed.
#[cfg(unix)]
fn sync_mode(source: &Path, target: &Path) -> Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    let (Ok(from), Ok(to)) = (std::fs::metadata(source), std::fs::metadata(target)) else {
        return Ok(false);
    };
    let wanted = from.permissions().mode() & 0o777;
    if to.permissions().mode() & 0o777 == wanted {
        return Ok(false);
    }
    std::fs::set_permissions(target, std::fs::Permissions::from_mode(wanted))
        .map_err(|e| Error::new(format!("cannot set mode on {}: {e}", target.display())))?;
    Ok(true)
}

#[cfg(not(unix))]
fn sync_mode(_source: &Path, _target: &Path) -> Result<bool> {
    Ok(false)
}

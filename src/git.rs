//! Git operations, run through the `git` executable.
//!
//! ahu never stages, commits, stashes, resets, cleans, or switches branches in
//! the invoking checkout. The only mutating Git operation it performs is
//! creating and removing its own task worktrees and deleting their branches.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::bail;
use crate::util::{Error, Result, digest_bytes};

/// The repository a launch is being made from.
#[derive(Debug, Clone)]
pub struct Repo {
    /// Top level of the invoking working tree.
    pub root: PathBuf,
    /// `git rev-parse --path-format=absolute --git-common-dir`. Shared by every
    /// worktree of the same repository, which is what makes it usable as the
    /// repository identity.
    pub common_dir: PathBuf,
    /// HEAD commit of the invoking checkout, if the repository has one.
    pub head: Option<String>,
}

impl Repo {
    /// All ahu task checkouts are siblings under the primary checkout, even
    /// when the launch originates in a linked worktree.
    pub fn primary_root(&self) -> Result<PathBuf> {
        let listing = run_ok(&self.root, &["worktree", "list", "--porcelain", "-z"])?;
        let path = listing
            .split('\0')
            .next()
            .and_then(|line| line.strip_prefix("worktree "))
            .ok_or_else(|| Error::new("Git did not report a primary checkout"))?;
        Ok(PathBuf::from(path))
    }

    /// A stable per-repository identifier shared by all of its worktrees.
    ///
    /// Launching from two different worktrees of one repository must land in a
    /// single cmux group, so the identity is derived from the Git common
    /// directory rather than the current working tree.
    pub fn identity(&self) -> String {
        let canonical = self
            .common_dir
            .canonicalize()
            .unwrap_or_else(|_| self.common_dir.clone());
        digest_bytes(canonical.as_os_str().as_encoded_bytes())[..16].to_string()
    }

    /// Human-facing repository name, used for the cmux group title.
    pub fn display_name(&self) -> String {
        // For a linked worktree the common dir is `<main>/.git`; for the main
        // checkout it is also `<main>/.git`. Either way the repository name is
        // the parent directory of the common dir when it is named `.git`, and
        // the common dir's own stem for a bare repository.
        let candidate = if self.common_dir.file_name().and_then(|s| s.to_str()) == Some(".git") {
            self.common_dir.parent().map(Path::to_path_buf)
        } else {
            Some(self.common_dir.clone())
        };
        candidate
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            .map(|s| s.trim_end_matches(".git").to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "repository".to_string())
    }
}

pub(crate) fn run(dir: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new(crate::selection::resolve_utility("git")?)
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| Error::new(format!("cannot run git: {e}")))
}

pub(crate) fn run_ok(dir: &Path, args: &[&str]) -> Result<String> {
    let out = run(dir, args)?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Find the repository containing `start`.
pub fn discover(start: &Path) -> Result<Repo> {
    let probe = run(start, &["rev-parse", "--is-inside-work-tree"])?;
    if !probe.status.success() {
        bail!(
            "{} is not inside a Git repository.\n\
             ahu launches tasks into fresh worktrees, so it needs a repository. \
             Run ahu from a checkout, or create one with `git init`.",
            start.display()
        );
    }
    let root = PathBuf::from(run_ok(
        start,
        &["rev-parse", "--path-format=absolute", "--show-toplevel"],
    )?);
    let common_dir = PathBuf::from(run_ok(
        start,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?);
    let head_out = run(start, &["rev-parse", "HEAD"])?;
    let head = if head_out.status.success() {
        Some(String::from_utf8_lossy(&head_out.stdout).trim().to_string())
    } else {
        None
    };
    // Every harness invocation in this process -- launches, the `run-task` exec,
    // and the `--version` probes that run before any preview is printed -- must
    // refuse a binary that comes from a repository ahu has opened. The probes
    // have no repository to check against at their call site, so the exclusion
    // is registered here, at the one place a repository enters the process.
    crate::selection::exclude_root(&root);
    Ok(Repo {
        root,
        common_dir,
        head,
    })
}

/// `true` when the working tree or index has any change at all.
pub fn is_dirty(repo: &Repo) -> Result<bool> {
    let out = run_ok(&repo.root, &["status", "--porcelain"])?;
    Ok(!out.is_empty())
}

/// Create a worktree at `path` on a new branch `branch`, based on `base`.
pub fn add_worktree(repo: &Repo, path: &Path, branch: &str, base: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let path_str = path.to_string_lossy().to_string();
    // `--` closes the option list: a path is a path even when something has
    // arranged for it to begin with a hyphen. `base` stays after it, which is
    // where `git worktree add` expects the commit-ish.
    let out = run(
        &repo.root,
        &["worktree", "add", "-b", branch, "--", &path_str, base],
    )?;
    if !out.status.success() {
        let detail = String::from_utf8_lossy(&out.stderr);
        let permission_hint = if detail.contains("Permission denied")
            || detail.contains("Operation not permitted")
            || detail.contains("cannot lock ref")
        {
            "\nCreating a task needs write access to the repository's Git metadata. \
             If this command is running in a restricted agent session, request approval \
             to run ahu launch outside its sandbox, or run it in your terminal. \
             --allow-widened-approvals only controls the child agent; it cannot grant \
             permissions to the session launching ahu."
        } else {
            ""
        };
        bail!(
            "could not create worktree at {}: {}{}",
            path.display(),
            detail.trim(),
            permission_hint
        );
    }
    Ok(())
}

/// Remove a worktree that ahu created and, when it is safe, its branch.
///
/// This is only ever called to clean up a launch attempt that failed before any
/// process could have run in the worktree. It refuses to force-remove, so a
/// worktree holding changes is preserved and reported instead.
pub fn remove_worktree(repo: &Repo, path: &Path, branch: &str) -> Result<()> {
    remove_task_worktree(repo, path)?;
    // Best effort: a branch that still holds commits is left alone by `-d`.
    let _ = run(&repo.root, &["branch", "-d", branch]);
    Ok(())
}

/// Remove a task worktree without touching any branch.
///
/// `--` closes the option list, so the path is treated as a path even when
/// something has arranged for it to begin with a hyphen. Refuses to force, so a
/// worktree holding changes or untracked files is preserved and reported.
pub fn remove_task_worktree(repo: &Repo, path: &Path) -> Result<()> {
    let path_str = path.to_string_lossy().to_string();
    let out = run(&repo.root, &["worktree", "remove", "--", &path_str])?;
    if !out.status.success() {
        bail!(
            "could not remove worktree {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Delete a task branch with `git branch -d`, which refuses a branch whose
/// commits are not merged.
///
/// `--` closes the option list: the name is treated as a branch name even when
/// something has arranged for it to begin with a hyphen. Never `-D`, never
/// `refs/heads/` spelling — `git branch -d` does not accept the fully-qualified
/// form, and the gate on unmerged commits belongs to Git itself.
///
/// `primary` must be the repository's primary checkout, resolved while the
/// checkout that `remove` is tearing down still exists: `remove` deletes the
/// task's worktree first, and resolving the primary afterwards would ask Git
/// to run from a directory that is already gone. Running from the primary also
/// keeps `-d`'s own merged-check judging the same HEAD that
/// [`branch_merged_into_primary_head`] gates on.
pub fn delete_task_branch(primary: &Path, branch: &str) -> Result<()> {
    let out = run(primary, &["branch", "-d", "--", branch])?;
    if !out.status.success() {
        bail!(
            "could not delete branch {}: {}",
            branch,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// `true` when every commit on `branch` is already in the primary checkout's
/// current branch.
///
/// The `refs/heads/` spelling keeps a branch name that begins with `-` from
/// being read as an option. Runs from the primary checkout so "merged" is
/// judged against what the user sees there, not against a linked worktree.
pub fn branch_merged_into_primary_head(repo: &Repo, branch: &str) -> Result<bool> {
    let primary = repo.primary_root()?;
    let out = run(
        &primary,
        &[
            "merge-base",
            "--is-ancestor",
            &format!("refs/heads/{branch}"),
            "HEAD",
        ],
    )?;
    if out.status.success() {
        return Ok(true);
    }
    let code = out.status.code();
    if code == Some(1) {
        return Ok(false);
    }
    bail!(
        "cannot tell whether branch {} is merged: {}",
        branch,
        String::from_utf8_lossy(&out.stderr).trim()
    );
}

/// `true` when `branch` already exists in the repository.
pub fn branch_exists(repo: &Repo, branch: &str) -> Result<bool> {
    let out = run(
        &repo.root,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )?;
    Ok(out.status.success())
}

/// Branch names matching a glob, e.g. `ahu/*/<task-id>`.
///
/// Used to recover the branch belonging to a task whose record ahu cannot read.
/// The record is refused, but the task id is the state directory's own name, and
/// Git still knows what branch carries it — so the user can be told where their
/// leftover work is without ahu reinterpreting a schema it does not understand.
pub fn branches_matching(repo: &Repo, pattern: &str) -> Result<Vec<String>> {
    // `--format` rather than parsing `git branch` output, which decorates the
    // current branch with a marker. `--` is not accepted by `branch --list`, so
    // the pattern is passed as the single positional it expects; a pattern
    // starting with `-` would be read as an option, and ahu builds this one
    // itself from a validated task id rather than taking it from a repository.
    let out = run(
        &repo.root,
        &["branch", "--list", "--format=%(refname:short)", pattern],
    )?;
    if !out.status.success() {
        bail!(
            "cannot list branches matching {pattern:?}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

//! Paths for bounded eval results; raw execution artifacts always stay external.

use std::path::{Component, Path, PathBuf};

use crate::git::Repo;
use crate::util::{Error, ErrorKind, Result};

fn usage(message: &str) -> Error {
    Error::new(message).with_kind(ErrorKind::Usage)
}

/// Omission is resolved only after discovery. Explicit paths retain cwd semantics.
pub fn resolve_records(repo: &Repo, explicit: Option<&Path>) -> Result<PathBuf> {
    match explicit {
        Some(path) => Ok(path.to_path_buf()),
        None => Ok(repo.primary_root()?.join(".ahu/evals/runs.jsonl")),
    }
}

/// Validate both readable and writable bounded results, including derived summaries.
/// This is a pathname policy, not isolation from concurrent same-user changes.
pub fn results_path(repo: &Repo, path: &Path) -> Result<PathBuf> {
    if path.file_name().is_none() || path.as_os_str().is_empty() {
        return Err(usage("records path must name a file"));
    }
    let primary = repo.primary_root()?.canonicalize()?;
    let checkout = repo.root.canonicalize()?;
    let target = resolve_storage_path(path, &primary, &checkout)?;
    let local = primary.join(".ahu/evals");
    if target.starts_with(&local) && target != local {
        // A nested checkout cannot borrow its enclosing project's exception.
        if target
            .ancestors()
            .skip(1)
            .take_while(|p| *p != primary.as_path())
            .any(has_git_marker)
        {
            return Err(usage("eval results cannot belong to a nested checkout"));
        }
        ignored_untracked(&primary, &target)?;
        safe_directories(
            &primary.join(".ahu"),
            target.parent().unwrap(),
            false,
            false,
        )?;
    } else if target.starts_with(&primary)
        || target.starts_with(&checkout)
        || target.ancestors().skip(1).any(has_git_marker)
    {
        return Err(usage(
            "eval results must live outside this repository or in the primary checkout's ignored, untracked .ahu/evals/ directory",
        ));
    }
    if let Some(meta) =
        crate::state::confine_file(&target).map_err(|error| usage(&error.to_string()))?
    {
        crate::storage::validate_owned_metadata(&meta, false)
            .map_err(|error| usage(&error.to_string()))?;
    }
    Ok(target)
}

fn has_git_marker(path: &Path) -> bool {
    std::fs::symlink_metadata(path.join(".git")).is_ok()
}

fn ignored_untracked(primary: &Path, target: &Path) -> Result<()> {
    let relative = target
        .strip_prefix(primary)
        .map_err(|_| usage("invalid eval results path"))?;
    let relative = relative
        .to_str()
        .ok_or_else(|| usage("eval results path must be UTF-8"))?;
    // Literal pathspecs prevent metacharacters in a filename from hiding index entries.
    // Check ancestors too, including tracked gitlinks and files absent on disk.
    for ancestor in Path::new(relative)
        .ancestors()
        .filter(|p| !p.as_os_str().is_empty())
    {
        let name = ancestor
            .to_str()
            .ok_or_else(|| usage("invalid eval results path"))?;
        let tracked = crate::git::run(
            primary,
            &["--literal-pathspecs", "ls-files", "-z", "--", name],
        )?;
        if !tracked.status.success() {
            return Err(usage("cannot verify eval results against the Git index"));
        }
        if (name == relative && !tracked.stdout.is_empty())
            || tracked
                .stdout
                .split(|b| *b == 0)
                .any(|entry| entry == name.as_bytes())
        {
            return Err(usage(
                "eval results destination or ancestor is tracked by Git",
            ));
        }
    }
    let ignored = crate::git::run(
        primary,
        &["check-ignore", "--quiet", "--no-index", "--", relative],
    )?;
    if !ignored.status.success() {
        return Err(usage(
            "eval results must be ignored by Git under the primary checkout's .ahu/evals/ directory",
        ));
    }
    Ok(())
}

pub(super) fn writable_records(repo: &Repo, path: &Path) -> Result<PathBuf> {
    let target = results_path(repo, path)?;
    let primary = repo.primary_root()?.canonicalize()?;
    let parent = target.parent().unwrap();
    if target.starts_with(primary.join(".ahu/evals")) {
        safe_directories(&primary.join(".ahu"), parent, true, false)?;
    } else if !parent.is_dir() {
        return Err(usage(
            "records directory is unavailable; create the explicit external directory first",
        ));
    }
    Ok(target)
}

/// Existing external parent directories are user-selected and may use aliases
/// (including macOS /tmp and /var). Canonicalize those before applying the
/// repository boundary; never follow a leaf link, a repository-owned link, or
/// a link within .ahu or the raw-artifact store. Refuse traversal components.
/// As with state confinement, this does not isolate concurrent same-user edits.
fn resolve_storage_path(path: &Path, primary: &Path, checkout: &Path) -> Result<PathBuf> {
    resolve_storage_components(path, primary, checkout, false, 40)
}

fn resolve_storage_components(
    path: &Path,
    primary: &Path,
    checkout: &Path,
    directory_alias: bool,
    links_left: usize,
) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let parts: Vec<_> = absolute.components().collect();
    if parts
        .iter()
        .any(|part| matches!(part, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(usage("eval storage path contains a traversal component"));
    }
    let mut cursor = PathBuf::new();
    for (index, part) in parts.iter().enumerate() {
        match part {
            Component::RootDir | Component::Normal(_) => cursor.push(part),
            Component::CurDir => continue,
            _ => unreachable!("traversal components checked above"),
        }
        let leaf = index + 1 == parts.len();
        match std::fs::symlink_metadata(&cursor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let protected = cursor.components().any(|part| part.as_os_str() == ".ahu")
                    || cursor
                        .ancestors()
                        .any(|path| path.ends_with("ahu/eval-artifacts"));
                if (leaf && !directory_alias)
                    || protected
                    || in_repository(&cursor, primary, checkout)
                {
                    return Err(usage(
                        "eval storage path must not contain a leaf or protected-directory symlink",
                    ));
                }
                if links_left == 0 {
                    return Err(usage("eval storage parent has too many symlinks"));
                }
                let target = std::fs::read_link(&cursor)?;
                let target = if target.is_absolute() {
                    target
                } else {
                    cursor
                        .parent()
                        .ok_or_else(|| usage("invalid symlink parent"))?
                        .join(target)
                };
                // Inspect every hop: plain canonicalize would hide a chain
                // passing through a repository's redirected .ahu directory.
                let real =
                    resolve_storage_components(&target, primary, checkout, true, links_left - 1)?;
                if !real.is_dir() || in_repository(&real, primary, checkout) {
                    return Err(usage(
                        "eval storage parent symlink must resolve to a directory outside every repository",
                    ));
                }
                cursor = real;
            }
            Ok(meta) if !leaf && !meta.is_dir() => {
                return Err(usage("eval storage parent must be a directory"));
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(cursor)
}

fn in_repository(path: &Path, primary: &Path, checkout: &Path) -> bool {
    path.starts_with(primary) || path.starts_with(checkout) || path.ancestors().any(has_git_marker)
}

/// Create only missing components, mode 0700 at creation. Never chmod existing
/// directories (especially user-selected ancestors of external storage).
fn safe_directories(base: &Path, path: &Path, create: bool, private: bool) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let relative = path
        .strip_prefix(base)
        .map_err(|_| usage("invalid eval storage directory"))?;
    let mut cursor = base.to_path_buf();
    let mut paths = vec![cursor.clone()];
    for part in relative.components() {
        if !matches!(part, Component::Normal(_)) {
            return Err(usage("invalid eval storage component"));
        }
        cursor.push(part);
        paths.push(cursor.clone());
    }
    for directory in paths {
        let meta = match std::fs::symlink_metadata(&directory) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if !create {
                    continue;
                }
                std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
                std::fs::symlink_metadata(&directory)?
            }
            Err(error) => return Err(error.into()),
        };
        // SAFETY: geteuid has no preconditions.
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & (if private { 0o7077 } else { 0o7022 }) != 0
        {
            return Err(usage(
                "eval storage directory must be owned by the current user with safe permissions",
            ));
        }
    }
    Ok(())
}

pub(super) fn artifacts_dir(repo: &Repo) -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_STATE_HOME") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(
            std::env::var_os("HOME").ok_or_else(|| usage("cannot locate external ahu storage"))?,
        )
        .join(".local/state"),
    };
    external_artifacts_dir(repo, &base)
}

fn external_artifacts_dir(repo: &Repo, base: &Path) -> Result<PathBuf> {
    if !base.is_absolute() {
        return Err(usage("external ahu storage must be absolute"));
    }
    let primary = repo.primary_root()?.canonicalize()?;
    let checkout = repo.root.canonicalize()?;
    let root = resolve_storage_path(&base.join("ahu/eval-artifacts"), &primary, &checkout)?;
    if root.starts_with(primary)
        || root.starts_with(checkout)
        || root.ancestors().any(has_git_marker)
    {
        return Err(usage(
            "raw eval artifacts must remain outside every repository",
        ));
    }
    // The already-existing host ancestors are not ours to chmod. Create any
    // missing ancestors privately, then enforce owner-only access in our store.
    let mut missing = Vec::new();
    let mut existing = root.as_path();
    while !existing.exists() {
        missing.push(existing.to_path_buf());
        existing = existing
            .parent()
            .ok_or_else(|| usage("invalid external storage root"))?;
    }
    for path in missing.into_iter().rev() {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    let directory = root.join(repo.identity());
    safe_directories(&root, &directory, true, true)?;
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
    }

    fn repository(root: &Path) -> Repo {
        crate::git::run_ok(root, &["init", "-q"]).unwrap();
        crate::git::discover(root).unwrap()
    }

    fn ignore(repo: &Repo) {
        std::fs::write(repo.root.join(".gitignore"), "/.ahu/evals/\n").unwrap();
    }

    #[test]
    fn default_and_explicit_paths_are_distinct_and_read_resolution_creates_nothing() {
        let dir = tempdir();
        let repo = repository(dir.path());
        let default = resolve_records(&repo, None).unwrap();
        assert_eq!(default, repo.root.join(".ahu/evals/runs.jsonl"));
        assert_eq!(
            resolve_records(&repo, Some(Path::new("runs.jsonl"))).unwrap(),
            Path::new("runs.jsonl")
        );
        assert!(results_path(&repo, &default).is_err(), "not ignored yet");
        ignore(&repo);
        assert_eq!(results_path(&repo, &default).unwrap(), default);
        assert!(!repo.root.join(".ahu").exists());
        assert!(super::super::report(&repo, &default).is_err());
        assert!(!repo.root.join(".ahu").exists(), "report is read-only");
        assert_eq!(writable_records(&repo, &default).unwrap(), default);
        assert_eq!(
            std::fs::metadata(default.parent().unwrap()).unwrap().mode() & 0o777,
            0o700
        );
        assert!(!default.exists());
    }

    #[test]
    fn local_results_round_trip_with_identical_schema_and_fingerprint_to_external() {
        let dir = tempdir();
        let external = tempdir();
        let repo = repository(dir.path());
        ignore(&repo);
        let local = writable_records(&repo, &resolve_records(&repo, None).unwrap()).unwrap();
        let outside = writable_records(&repo, &external.path().join("runs.jsonl")).unwrap();
        let row = serde_json::json!({"schema_version":2,"case_id":"fixture","corpus_version":"1","harness":"fixture","model":"fixture","score":1.0,"passed":true});
        for path in [&local, &outside] {
            super::super::append_jsonl(path, &row).unwrap();
            assert_eq!(std::fs::metadata(path).unwrap().mode() & 0o777, 0o600);
        }
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o644)).unwrap();
        let local_report = super::super::report(&repo, &local).unwrap();
        let external_report = super::super::report(&repo, &outside).unwrap();
        assert_eq!(local_report.record_count, 1);
        assert_eq!(
            local_report.records_fingerprint,
            external_report.records_fingerprint
        );
        let summary = local.parent().unwrap().join("summary.json");
        assert_eq!(results_path(&repo, &summary).unwrap(), summary);
        for name in [
            "runs.jsonl",
            ".ahu/evals-other/runs.jsonl",
            ".ahu/state/runs.jsonl",
        ] {
            assert!(results_path(&repo, &repo.root.join(name)).is_err());
        }
        let other = repository(external.path());
        assert!(
            results_path(&repo, &outside).is_err(),
            "another checkout is not external storage"
        );
        assert!(
            results_path(&other, &local).is_err(),
            "the exception belongs to the selected project"
        );
    }

    #[test]
    fn trusted_external_parent_aliases_resolve_but_cannot_redirect_into_results_or_raw_store() {
        let dir = tempdir();
        let external = tempdir();
        let repo = repository(dir.path());
        ignore(&repo);
        let real_parent = external.path().join("real");
        std::fs::create_dir(&real_parent).unwrap();
        let alias = external.path().join("alias");
        symlink(&real_parent, &alias).unwrap();
        let expected = real_parent.join("runs.jsonl");
        assert_eq!(
            writable_records(&repo, &alias.join("runs.jsonl")).unwrap(),
            expected
        );
        std::fs::write(&expected, "\n").unwrap();
        std::fs::set_permissions(&expected, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            super::super::report(&repo, &alias.join("runs.jsonl"))
                .unwrap()
                .record_count,
            0
        );
        let artifacts = external_artifacts_dir(&repo, &alias).unwrap();
        assert!(artifacts.starts_with(&real_parent));
        let redirected_store = real_parent.join("ahu/eval-artifacts");
        std::fs::rename(&redirected_store, real_parent.join("saved-artifacts")).unwrap();
        symlink(real_parent.join("saved-artifacts"), &redirected_store).unwrap();
        assert!(external_artifacts_dir(&repo, &alias).is_err());

        std::fs::remove_file(&alias).unwrap();
        symlink(&repo.root, &alias).unwrap();
        assert!(results_path(&repo, &alias.join(".ahu/evals/runs.jsonl")).is_err());
        assert!(external_artifacts_dir(&repo, &alias).is_err());
        std::fs::create_dir(repo.root.join(".ahu")).unwrap();
        let chain = repo.root.join(".ahu/redirect");
        symlink(&real_parent, &chain).unwrap();
        std::fs::remove_file(&alias).unwrap();
        symlink(&chain, &alias).unwrap();
        assert!(results_path(&repo, &alias.join("runs.jsonl")).is_err());
        assert!(external_artifacts_dir(&repo, &alias).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_tmp_and_var_aliases_support_existing_and_new_external_storage() {
        let dir = tempdir();
        let repo = repository(dir.path());
        for system_parent in ["/tmp", "/var/tmp"] {
            let external = tempfile::tempdir_in(system_parent).unwrap();
            let canonical = external.path().canonicalize().unwrap();
            let path = external.path().join("runs.jsonl");
            assert_eq!(
                writable_records(&repo, &path).unwrap(),
                canonical.join("runs.jsonl")
            );
            std::fs::write(&path, "\n").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(super::super::report(&repo, &path).unwrap().record_count, 0);
            let artifacts = external_artifacts_dir(&repo, external.path()).unwrap();
            assert!(artifacts.starts_with(&canonical));
            assert_eq!(std::fs::metadata(artifacts).unwrap().mode() & 0o777, 0o700);
            let link = external.path().join("link.jsonl");
            symlink(&path, &link).unwrap();
            assert!(results_path(&repo, &link).is_err());
            let redirect = external.path().join("checkout");
            symlink(&repo.root, &redirect).unwrap();
            assert!(results_path(&repo, &redirect.join(".ahu/evals/runs.jsonl")).is_err());
        }
    }

    #[test]
    fn tracked_and_unignored_destinations_are_refused_even_when_absent_or_literal_patterns() {
        let dir = tempdir();
        let repo = repository(dir.path());
        ignore(&repo);
        let path = writable_records(&repo, &repo.root.join(".ahu/evals/[run].jsonl")).unwrap();
        std::fs::write(&path, b"original").unwrap();
        crate::git::run_ok(
            &repo.root,
            &[
                "--literal-pathspecs",
                "add",
                "-f",
                "--",
                ".ahu/evals/[run].jsonl",
            ],
        )
        .unwrap();
        assert!(
            results_path(&repo, &path)
                .unwrap_err()
                .to_string()
                .contains("tracked")
        );
        std::fs::remove_file(&path).unwrap();
        assert!(
            writable_records(&repo, &path).is_err(),
            "an absent indexed file remains tracked"
        );
        std::fs::write(
            repo.root.join(".gitignore"),
            "/.ahu/evals/*\n!/.ahu/evals/public.jsonl\n",
        )
        .unwrap();
        assert!(results_path(&repo, &repo.root.join(".ahu/evals/public.jsonl")).is_err());
        assert!(results_path(&repo, &repo.root.join(".ahu/evals/private.jsonl")).is_ok());
    }

    #[test]
    fn redirected_ancestors_leaves_traversal_and_hardlinks_are_refused() {
        let dir = tempdir();
        let external = tempdir();
        let repo = repository(dir.path());
        ignore(&repo);
        let default = resolve_records(&repo, None).unwrap();
        symlink(external.path(), repo.root.join(".ahu")).unwrap();
        assert!(writable_records(&repo, &default).is_err());
        std::fs::remove_file(repo.root.join(".ahu")).unwrap();
        writable_records(&repo, &default).unwrap();
        for target in [
            external.path().join("absent"),
            repo.root.join("tracked.json"),
        ] {
            symlink(target, &default).unwrap();
            assert!(results_path(&repo, &default).is_err());
            std::fs::remove_file(&default).unwrap();
        }
        let link = external.path().join("redirect");
        symlink(default.parent().unwrap(), &link).unwrap();
        assert!(results_path(&repo, &link.join("runs.jsonl")).is_err());
        symlink(external.path(), default.parent().unwrap().join("nested")).unwrap();
        assert!(
            writable_records(&repo, &default.parent().unwrap().join("nested/runs.jsonl")).is_err()
        );
        assert!(results_path(&repo, &default.parent().unwrap().join("../runs.jsonl")).is_err());
        let target = external.path().join("original");
        std::fs::write(&target, b"keep").unwrap();
        std::fs::hard_link(&target, &default).unwrap();
        assert!(results_path(&repo, &default).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"keep");
    }

    #[test]
    fn unsafe_local_modes_and_nested_checkouts_fail_without_chmod() {
        let dir = tempdir();
        let repo = repository(dir.path());
        ignore(&repo);
        let path = writable_records(&repo, &resolve_records(&repo, None).unwrap()).unwrap();
        let parent = path.parent().unwrap();
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(writable_records(&repo, &path).is_err());
        assert_eq!(std::fs::metadata(parent).unwrap().mode() & 0o777, 0o777);
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(results_path(&repo, &path).is_err());
        std::fs::remove_dir(&path).unwrap();
        std::fs::create_dir(parent.join(".git")).unwrap();
        assert!(results_path(&repo, &path).is_err());
    }

    #[test]
    fn linked_worktree_defaults_share_primary_results_and_reject_checkout_local_override() {
        let dir = tempdir();
        let linked = tempdir();
        let repo = repository(dir.path());
        ignore(&repo);
        crate::git::run_ok(
            &repo.root,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "fixture",
            ],
        )
        .unwrap();
        let worktree = linked.path().join("task");
        crate::git::run_ok(
            &repo.root,
            &[
                "worktree",
                "add",
                "--detach",
                worktree.to_str().unwrap(),
                "HEAD",
            ],
        )
        .unwrap();
        let child = crate::git::discover(&worktree).unwrap();
        let primary_default = resolve_records(&repo, None).unwrap();
        assert_eq!(resolve_records(&child, None).unwrap(), primary_default);
        assert_eq!(
            writable_records(&child, &primary_default).unwrap(),
            primary_default
        );
        assert!(writable_records(&child, &worktree.join(".ahu/evals/runs.jsonl")).is_err());
        assert!(!worktree.join(".ahu").exists());
    }

    #[test]
    fn raw_artifacts_have_separate_owner_only_external_storage_without_chmod() {
        let dir = tempdir();
        let external = tempdir();
        let repo = repository(dir.path());
        ignore(&repo);
        let records = writable_records(&repo, &resolve_records(&repo, None).unwrap()).unwrap();
        let before = std::fs::metadata(external.path()).unwrap().mode();
        let artifacts = external_artifacts_dir(&repo, external.path()).unwrap();
        let run = super::super::create_private_run_dir(&artifacts, "fixture").unwrap();
        assert_eq!(std::fs::metadata(&run).unwrap().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(&artifacts).unwrap().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(external.path()).unwrap().mode(), before);
        assert!(!run.starts_with(records.parent().unwrap()));
        assert!(external_artifacts_dir(&repo, records.parent().unwrap()).is_err());
        std::fs::set_permissions(&artifacts, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(external_artifacts_dir(&repo, external.path()).is_err());
        assert_eq!(std::fs::metadata(&artifacts).unwrap().mode() & 0o777, 0o755);
        let redirected = external.path().join("redirect");
        symlink(&repo.root, &redirected).unwrap();
        assert!(external_artifacts_dir(&repo, &redirected).is_err());
    }
}

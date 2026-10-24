//! Exact-worktree trust for explicitly authorized interactive auto tasks.
//! The lock serializes Ahu writers only. Native applications do not participate;
//! rereading before replacement detects some races, not an atomic transaction.
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agent::Permissions;
use crate::task::TaskRecord;
use crate::util::{Error, Result};

const LIMIT: u64 = 4 * 1024 * 1024;
const RECEIPT: &str = "native-trust.json";

fn refused() -> Error {
    Error::new(
        "native task trust refused: unsafe, changed, unsupported, or malformed state; native configuration contents are withheld",
    )
}

fn eligible(harness: &str, permissions: Permissions) -> bool {
    permissions == Permissions::Auto && matches!(harness, "claude-code" | "antigravity")
}

fn default_store(harness: &str, is_set: impl Fn(&str) -> bool) -> Result<()> {
    let overrides: &[&str] = if harness == "claude-code" {
        &["CLAUDE_CONFIG_DIR"]
    } else {
        &["GEMINI_CLI_HOME", "AGY_CONFIG_DIR"]
    };
    for key in overrides {
        if is_set(key) {
            return Err(Error::new(format!(
                "native task trust refused: {key} is set; relocated native trust stores are unsupported. Retry with the original HOME and {key} unset for this invocation; do not delete the task's native-trust receipt"
            )));
        }
    }
    Ok(())
}

fn home(harness: &str) -> Result<PathBuf> {
    default_store(harness, |key| std::env::var_os(key).is_some())?;
    let home = PathBuf::from(std::env::var_os("HOME").ok_or_else(refused)?);
    if !home.is_absolute() || home.canonicalize()? != home {
        return Err(refused());
    }
    checked(&home, true)?;
    Ok(home)
}

/// Called only after run-task's frozen identity, ownership and command checks.
pub(crate) fn prepare(dir: &Path, record: &TaskRecord) -> Result<()> {
    if !eligible(&record.identity.harness, record.identity.permissions) {
        return Ok(());
    }
    verify_scope(
        dir,
        &record.worktree,
        &record.task_id,
        &record.repo_identity,
    )?;
    eprintln!(
        "ahu: automatic permissions authorize native folder trust for this exact task worktree; Ahu-added trust is removed by `ahu remove`"
    );
    update(
        &home(&record.identity.harness)?,
        dir,
        &record.identity.harness,
        &record.worktree,
        false,
    )
}

fn verify_scope(dir: &Path, worktree: &Path, task_id: &str, repo_identity: &str) -> Result<()> {
    let repo = crate::git::discover(worktree)?;
    if repo.identity() != repo_identity
        || repo.root.canonicalize()? != worktree
        || crate::state::worktree_dir(&repo.root, task_id)? != worktree
        || worktree.canonicalize()? != worktree
        || crate::state::enclosing_checkout(dir).as_deref() != Some(worktree)
    {
        return Err(refused());
    }
    crate::state::verify_worktree_inside_repo(&repo.root, worktree)?;
    Ok(())
}

pub(crate) fn cleanup(dir: &Path, record: &TaskRecord) -> Result<()> {
    if fs::symlink_metadata(dir.join(RECEIPT))
        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(());
    }
    if !eligible(&record.identity.harness, record.identity.permissions) {
        return Err(refused());
    }
    update(
        &home(&record.identity.harness)?,
        dir,
        &record.identity.harness,
        &record.worktree,
        true,
    )
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema: u8,
    harness: String,
    worktree: PathBuf,
    home: PathBuf,
    // Claude's prior false value is restored; absent fields remain absent.
    prior_false: bool,
    project_added: bool,
}

fn checked(path: &Path, directory: bool) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(m)
            if !m.file_type().is_symlink()
                && m.uid() == unsafe { libc::geteuid() }
                && m.mode() & 0o022 == 0
                && (if directory {
                    m.is_dir()
                } else {
                    m.is_file() && m.nlink() == 1
                }) =>
        {
            Ok(Some(m))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        _ => Err(refused()),
    }
}

fn read(path: &Path, limit: u64) -> Result<Option<Vec<u8>>> {
    let Some(m) = checked(path, false)? else {
        return Ok(None);
    };
    if m.len() > limit {
        return Err(refused());
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| refused())?;
    let opened = file.metadata()?;
    if opened.ino() != m.ino() || opened.dev() != m.dev() {
        return Err(refused());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| refused())?;
    if bytes.len() as u64 > limit {
        return Err(refused());
    }
    Ok(Some(bytes))
}

fn replace(path: &Path, previous: Option<&[u8]>, bytes: &[u8], limit: u64) -> Result<()> {
    if bytes.len() as u64 > limit {
        return Err(refused());
    }
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().ok_or_else(refused)?)
        .map_err(|_| refused())?;
    tmp.write_all(bytes).map_err(|_| refused())?;
    tmp.as_file().sync_all().map_err(|_| refused())?;
    if read(path, limit)?.as_deref() != previous {
        return Err(refused());
    }
    tmp.persist(path).map_err(|_| refused())?;
    fs::File::open(path.parent().ok_or_else(refused)?)?.sync_all()?;
    Ok(())
}

// Keep the lock inode stable across calls and crashes; never unlink it.
fn lock_native(path: &Path) -> Result<fs::File> {
    use std::os::fd::AsRawFd;
    read(path, 128)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| refused())?;
    checked(path, false)?.ok_or_else(refused)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(file);
        }
        let error = std::io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(libc::EWOULDBLOCK))
            || std::time::Instant::now() >= deadline
        {
            return Err(refused());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn update(home: &Path, dir: &Path, harness: &str, worktree: &Path, remove: bool) -> Result<()> {
    // Validate every component before creating or reading state. No native
    // document is copied into task state, diagnostics, or a backup file.
    checked(home, true)?.ok_or_else(refused)?;
    crate::state::confine_existing_dir(dir)?;
    checked(dir, true)?.ok_or_else(refused)?;
    let lock = home.join(".ahu-native-trust.lock");
    let _lock = lock_native(&lock)?;
    let path = match harness {
        "claude-code" => home.join(".claude.json"),
        "antigravity" => {
            let mut parent = home.to_path_buf();
            for component in [".gemini", "antigravity-cli"] {
                parent.push(component);
                if checked(&parent, true)?.is_none() && !remove {
                    use std::os::unix::fs::DirBuilderExt;
                    fs::DirBuilder::new()
                        .mode(0o700)
                        .create(&parent)
                        .map_err(|_| refused())?;
                }
            }
            parent.join("settings.json")
        }
        _ => return Err(refused()),
    };
    let receipt_path = dir.join(RECEIPT);
    let receipt_bytes = read(&receipt_path, 8192)?;
    let receipt: Option<Receipt> = receipt_bytes
        .as_ref()
        .map(|b| serde_json::from_slice(b).map_err(|_| refused()))
        .transpose()?;
    if let Some(r) = &receipt
        && (r.schema != 1 || r.harness != harness || r.home != home || r.worktree != worktree)
    {
        return Err(refused());
    }
    if remove && receipt.is_none() {
        return Ok(());
    }
    let before = read(&path, LIMIT)?;
    let mut doc: Value = match &before {
        Some(bytes) => serde_json::from_slice(bytes).map_err(|_| refused())?,
        None => json!({}),
    };
    let root = doc.as_object_mut().ok_or_else(refused)?;
    let key = worktree.to_str().ok_or_else(refused)?;
    let (already, prior_false, project_added, project_entry_removed) = if harness == "claude-code" {
        let projects = root
            .entry("projects")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(refused)?;
        let project_added = !projects.contains_key(key);
        let project = projects
            .entry(key)
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(refused)?;
        let old = project.get("hasTrustDialogAccepted");
        if old.is_some_and(|v| !v.is_boolean()) {
            return Err(refused());
        }
        let already = old == Some(&Value::Bool(true));
        let prior_false = old == Some(&Value::Bool(false));
        if remove {
            if already {
                if receipt.as_ref().is_some_and(|r| r.prior_false) {
                    project.insert("hasTrustDialogAccepted".into(), Value::Bool(false));
                } else {
                    project.remove("hasTrustDialogAccepted");
                }
            }
        } else {
            project.insert("hasTrustDialogAccepted".into(), Value::Bool(true));
        }
        let project_entry_removed = remove
            && !project_added
            && project.is_empty()
            && receipt.as_ref().is_some_and(|r| r.project_added);
        if project_entry_removed {
            projects.remove(key);
        }
        (already, prior_false, project_added, project_entry_removed)
    } else {
        let entries = root
            .entry("trustedWorkspaces")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(refused)?;
        if entries.iter().any(|v| !v.is_string()) {
            return Err(refused());
        }
        let already = entries.iter().any(|v| v.as_str() == Some(key));
        if remove {
            entries.retain(|v| v.as_str() != Some(key));
        } else if !already {
            entries.push(Value::String(key.into()));
        }
        (already, false, false, false)
    };
    if !remove && already {
        return Ok(());
    }
    let after = serde_json::to_vec_pretty(&doc).map_err(|_| refused())?;
    if after.len() as u64 > LIMIT {
        return Err(refused());
    }
    if !remove && receipt.is_none() {
        // Write-ahead receipt makes a crash on either side of the native write
        // recoverable by retry or task removal. It contains no native history.
        let receipt = Receipt {
            schema: 1,
            harness: harness.into(),
            worktree: worktree.into(),
            home: home.into(),
            prior_false,
            project_added,
        };
        replace(
            &receipt_path,
            None,
            &serde_json::to_vec(&receipt).map_err(|_| refused())?,
            8192,
        )?;
    }
    if !remove || already || project_entry_removed {
        replace(&path, before.as_deref(), &after, LIMIT)?;
    }
    if remove {
        fs::remove_file(receipt_path).map_err(|_| refused())?;
        fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn native_trust_override_diagnostics_name_only_the_key_and_remedy() {
        for (harness, key) in [
            ("claude-code", "CLAUDE_CONFIG_DIR"),
            ("antigravity", "GEMINI_CLI_HOME"),
            ("antigravity", "AGY_CONFIG_DIR"),
        ] {
            let error = default_store(harness, |candidate| candidate == key)
                .unwrap_err()
                .to_string();
            assert!(error.contains(key));
            assert!(error.contains("unset for this invocation"));
            assert!(error.contains("do not delete"));
            default_store(harness, |_| false).unwrap();
        }
    }

    #[test]
    fn native_trust_cleanup_persists_empty_project_removal_after_native_revocation() {
        let f = Fixture::new("claude-code", json!({}));
        f.run("claude-code", false).unwrap();
        let mut native = f.doc();
        native["projects"][f.worktree.to_str().unwrap()]
            .as_object_mut()
            .unwrap()
            .remove("hasTrustDialogAccepted");
        fs::write(&f.path, serde_json::to_vec(&native).unwrap()).unwrap();
        f.run("claude-code", true).unwrap();
        assert!(
            f.doc()["projects"]
                .get(f.worktree.to_str().unwrap())
                .is_none()
        );
        assert!(!f.dir.join(RECEIPT).exists());
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        home: PathBuf,
        dir: PathBuf,
        worktree: PathBuf,
        path: PathBuf,
    }
    impl Fixture {
        fn new(harness: &str, value: Value) -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let base = tmp.path().canonicalize().unwrap();
            let home = base.join("native");
            let dir = base.join("task");
            let worktree = base.join("worktree");
            fs::create_dir(&home).unwrap();
            fs::create_dir(&dir).unwrap();
            let path = if harness == "claude-code" {
                home.join(".claude.json")
            } else {
                let parent = home.join(".gemini/antigravity-cli");
                fs::create_dir_all(&parent).unwrap();
                parent.join("settings.json")
            };
            fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            Self {
                _tmp: tmp,
                home,
                dir,
                worktree,
                path,
            }
        }
        fn run(&self, harness: &str, remove: bool) -> Result<()> {
            update(&self.home, &self.dir, harness, &self.worktree, remove)
        }
        fn doc(&self) -> Value {
            serde_json::from_slice(&fs::read(&self.path).unwrap()).unwrap()
        }
    }

    #[test]
    fn native_trust_shapes_preserve_history_and_cleanup_is_retryable() {
        for harness in ["claude-code", "antigravity"] {
            let f = Fixture::new(
                harness,
                json!({"unrelated":{"history":[1,2]},"projects":{"/other":{"hasTrustDialogAccepted":true}},"trustedWorkspaces":["/other"]}),
            );
            let original = f.doc();
            f.run(harness, false).unwrap();
            assert!(f.dir.join(RECEIPT).exists());
            f.run(harness, false).unwrap();
            let mut native = f.doc();
            if harness == "claude-code" {
                assert_eq!(
                    native["projects"][f.worktree.to_str().unwrap()]["hasTrustDialogAccepted"],
                    true
                );
                native["projects"][f.worktree.to_str().unwrap()]["history"] =
                    json!(["native-added"]);
            } else {
                assert!(
                    native["trustedWorkspaces"]
                        .as_array()
                        .unwrap()
                        .contains(&json!(f.worktree))
                );
            }
            native["newNativeField"] = json!(42);
            fs::write(&f.path, serde_json::to_vec(&native).unwrap()).unwrap();
            f.run(harness, true).unwrap();
            f.run(harness, true).unwrap();
            let after = f.doc();
            assert_eq!(after["newNativeField"], 42);
            assert_eq!(after["unrelated"], original["unrelated"]);
            assert_eq!(after["projects"]["/other"], original["projects"]["/other"]);
            if harness == "claude-code" {
                assert_eq!(
                    after["projects"][f.worktree.to_str().unwrap()],
                    json!({"history":["native-added"]})
                );
            } else {
                assert_eq!(after["trustedWorkspaces"], json!(["/other"]));
            }
            assert!(!f.dir.join(RECEIPT).exists());
        }
    }

    #[test]
    fn native_trust_existing_trust_is_not_owned_or_removed() {
        for harness in ["claude-code", "antigravity"] {
            let f = Fixture::new(harness, json!({}));
            let original = if harness == "claude-code" {
                json!({"projects":{f.worktree.to_str().unwrap():{"hasTrustDialogAccepted":true,"history":[7]}}})
            } else {
                json!({"trustedWorkspaces":[f.worktree]})
            };
            let bytes = serde_json::to_vec(&original).unwrap();
            fs::write(&f.path, &bytes).unwrap();
            f.run(harness, false).unwrap();
            f.run(harness, true).unwrap();
            assert_eq!(fs::read(&f.path).unwrap(), bytes);
            assert!(!f.dir.join(RECEIPT).exists());
        }
    }

    #[test]
    fn native_trust_write_ahead_receipt_recovers_before_and_after_native_write() {
        let f = Fixture::new("claude-code", json!({}));
        let original = json!({"projects":{f.worktree.to_str().unwrap():{"hasTrustDialogAccepted":false,"history":[7]}}});
        fs::write(&f.path, serde_json::to_vec(&original).unwrap()).unwrap();
        let receipt = Receipt {
            schema: 1,
            harness: "claude-code".into(),
            worktree: f.worktree.clone(),
            home: f.home.clone(),
            prior_false: true,
            project_added: false,
        };
        fs::write(f.dir.join(RECEIPT), serde_json::to_vec(&receipt).unwrap()).unwrap();
        f.run("claude-code", false).unwrap();
        f.run("claude-code", false).unwrap();
        fs::write(&f.path, b"malformed-private-sentinel").unwrap();
        let error = f.run("claude-code", true).unwrap_err().to_string();
        assert!(!error.contains("sentinel"));
        assert!(f.dir.join(RECEIPT).exists());
        fs::write(&f.path, serde_json::to_vec(&json!({"projects":{f.worktree.to_str().unwrap():{"hasTrustDialogAccepted":true,"history":[7]}}})).unwrap()).unwrap();
        f.run("claude-code", true).unwrap();
        assert_eq!(f.doc(), original);
    }

    #[test]
    fn native_trust_refuses_bad_shapes_links_and_oversize_without_overwrite() {
        for (harness, invalid) in [
            ("claude-code", json!({"projects":[]})),
            ("antigravity", json!({"trustedWorkspaces":[{}]})),
            ("claude-code", json!([])),
        ] {
            let f = Fixture::new(harness, invalid);
            let before = fs::read(&f.path).unwrap();
            assert!(f.run(harness, false).is_err());
            assert_eq!(fs::read(&f.path).unwrap(), before);
            assert!(!f.dir.join(RECEIPT).exists());
        }
        let f = Fixture::new("claude-code", json!({}));
        let other = f.home.join("other");
        fs::rename(&f.path, &other).unwrap();
        symlink(&other, &f.path).unwrap();
        assert!(f.run("claude-code", false).is_err());
        fs::remove_file(&f.path).unwrap();
        fs::write(&f.path, vec![b' '; LIMIT as usize + 1]).unwrap();
        assert!(f.run("claude-code", false).is_err());
        fs::write(&f.path, b"{}").unwrap();
        fs::set_permissions(&f.path, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(f.run("claude-code", false).is_err());
        let f = Fixture::new("antigravity", json!({}));
        let parent = f.home.join(".gemini");
        let other = f.home.join("elsewhere");
        fs::rename(&parent, &other).unwrap();
        symlink(&other, parent).unwrap();
        assert!(f.run("antigravity", false).is_err());
    }

    #[test]
    fn native_trust_cleanup_does_not_recreate_deleted_native_state() {
        for harness in ["claude-code", "antigravity"] {
            let f = Fixture::new(harness, json!({}));
            f.run(harness, false).unwrap();
            fs::remove_file(&f.path).unwrap();
            if harness == "antigravity" {
                fs::remove_dir_all(f.home.join(".gemini")).unwrap();
            }
            f.run(harness, true).unwrap();
            assert!(!f.path.exists());
            assert!(!f.dir.join(RECEIPT).exists());
        }
    }

    #[test]
    fn native_trust_refuses_arbitrary_checkout_and_worktree_shaped_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        crate::git::run_ok(&root, &["init", "-q"]).unwrap();
        let repo = crate::git::discover(&root).unwrap();
        let task_id = "00000000-0000-7000-8000-000000000001";
        let dir = crate::state::worktree_task_dir(&root, &repo.identity(), task_id);
        assert!(verify_scope(&dir, &root, task_id, &repo.identity()).is_err());
        let shaped = root.join(".worktrees").join(task_id);
        fs::create_dir_all(&shaped).unwrap();
        let dir = crate::state::worktree_task_dir(&shaped, &repo.identity(), task_id);
        assert!(verify_scope(&dir, &shaped, task_id, &repo.identity()).is_err());
        assert!(!dir.exists());
    }

    #[test]
    fn native_trust_opt_in_is_exact_and_changed_write_is_refused() {
        for harness in ["claude-code", "antigravity", "codex", "kiro", "opencode"] {
            for permissions in [
                Permissions::Auto,
                Permissions::Prompt,
                Permissions::AcceptEdits,
            ] {
                assert_eq!(
                    eligible(harness, permissions),
                    permissions == Permissions::Auto
                        && matches!(harness, "claude-code" | "antigravity")
                );
            }
        }
        let f = Fixture::new("claude-code", json!({}));
        assert!(replace(&f.path, Some(b"stale"), b"{}", LIMIT).is_err());
        assert_eq!(fs::read(&f.path).unwrap(), b"{}");
        symlink(&f.path, f.dir.join(RECEIPT)).unwrap();
        assert!(f.run("claude-code", false).is_err());
    }

    #[test]
    fn native_trust_removes_only_new_empty_projects_and_preserves_home_mode() {
        for existing in [false, true] {
            let f = Fixture::new("claude-code", json!({}));
            if existing {
                fs::write(
                    &f.path,
                    serde_json::to_vec(&json!({"projects":{f.worktree.to_str().unwrap():{}}}))
                        .unwrap(),
                )
                .unwrap();
            }
            fs::set_permissions(&f.home, fs::Permissions::from_mode(0o750)).unwrap();
            f.run("claude-code", false).unwrap();
            assert_eq!(fs::metadata(&f.home).unwrap().mode() & 0o777, 0o750);
            f.run("claude-code", true).unwrap();
            assert_eq!(
                f.doc()["projects"]
                    .get(f.worktree.to_str().unwrap())
                    .is_some(),
                existing
            );
            assert_eq!(fs::metadata(&f.home).unwrap().mode() & 0o777, 0o750);
        }
    }

    #[test]
    fn native_trust_concurrent_ahu_tasks_preserve_both_entries() {
        let f = Fixture::new("antigravity", json!({}));
        let dir2 = f.dir.with_file_name("task2");
        fs::create_dir(&dir2).unwrap();
        let worktree2 = f.worktree.with_file_name("worktree2");
        std::thread::scope(|s| {
            s.spawn(|| f.run("antigravity", false).unwrap());
            s.spawn(|| update(&f.home, &dir2, "antigravity", &worktree2, false).unwrap());
        });
        assert_eq!(f.doc()["trustedWorkspaces"].as_array().unwrap().len(), 2);
        f.run("antigravity", true).unwrap();
        assert_eq!(f.doc()["trustedWorkspaces"], json!([worktree2]));
        update(&f.home, &dir2, "antigravity", &worktree2, true).unwrap();
        assert_eq!(f.doc()["trustedWorkspaces"], json!([]));
    }
}

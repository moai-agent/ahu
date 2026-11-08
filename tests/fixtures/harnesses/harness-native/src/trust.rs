//! Exact-worktree trust for explicitly authorized interactive auto tasks.
//! The lock serializes Ahu writers only. Native applications do not participate;
//! rereading before replacement detects some races, not an atomic transaction.
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[cfg(test)]
use crate::Permissions;
use crate::util::{Error, Result};

const LIMIT: u64 = 4 * 1024 * 1024;
const RECEIPT: &str = "native-trust.json";

fn refused() -> Error {
    Error::new(
        "native task trust refused: unsafe, changed, unsupported, or malformed state; native configuration contents are withheld",
    )
}

#[cfg(test)]
fn eligible(harness: &str, permissions: Permissions) -> bool {
    permissions == Permissions::Auto && matches!(harness, "claude-code" | "antigravity" | "codex")
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema: u8,
    harness: String,
    worktree: PathBuf,
    home: PathBuf,
    // Claude's prior false / Codex's prior untrusted value is restored.
    // Only this bounded prior trust state is stored, never native config.
    prior_false: bool,
    project_added: bool,
}

fn checked(path: &Path, directory: bool) -> Result<Option<fs::Metadata>> {
    // SAFETY: geteuid takes no pointers and cannot invalidate filesystem handles.
    let owner = unsafe { libc::geteuid() };
    match fs::symlink_metadata(path) {
        Ok(m)
            if !m.file_type().is_symlink()
                && m.uid() == owner
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

// Lock the existing native-home directory so all repositories serialize Ahu
// writers without creating an ahu-owned file in the user's home directory.
// Native applications do not participate in this advisory lock.
fn lock_native(path: &Path) -> Result<fs::File> {
    use std::os::fd::AsRawFd;
    checked(path, true)?.ok_or_else(refused)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| refused())?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        // SAFETY: file owns this live descriptor; flock changes only its advisory lock.
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

pub(crate) fn update(
    home: &Path,
    dir: &Path,
    harness: &str,
    worktree: &Path,
    remove: bool,
) -> Result<()> {
    // Validate every component before creating or reading state. No native
    // document is copied into task state, diagnostics, or a backup file.
    checked(home, true)?.ok_or_else(refused)?;
    crate::files::check_components(dir)?;
    checked(dir, true)?.ok_or_else(refused)?;
    let _lock = lock_native(home)?;
    let path = match harness {
        "claude-code" => home.join(".claude.json"),
        "antigravity" | "codex" => {
            let mut parent = home.to_path_buf();
            let components: &[&str] = if harness == "codex" {
                &[".codex"]
            } else {
                &[".gemini", "antigravity-cli"]
            };
            for component in components {
                parent.push(component);
                if checked(&parent, true)?.is_none() && !remove {
                    use std::os::unix::fs::DirBuilderExt;
                    fs::DirBuilder::new()
                        .mode(0o700)
                        .create(&parent)
                        .map_err(|_| refused())?;
                }
            }
            parent.join(if harness == "codex" {
                "config.toml"
            } else {
                "settings.json"
            })
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
    let key = worktree.to_str().ok_or_else(refused)?;
    let (after, already, prior_false, project_added, project_entry_removed) = if harness == "codex"
    {
        edit_codex(before.as_deref(), key, remove, receipt.as_ref())?
    } else {
        let mut doc: Value = match &before {
            Some(bytes) => serde_json::from_slice(bytes).map_err(|_| refused())?,
            None => json!({}),
        };
        let root = doc.as_object_mut().ok_or_else(refused)?;
        let (already, prior_false, project_added, project_entry_removed) =
            if harness == "claude-code" {
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
        let after = serde_json::to_vec_pretty(&doc).map_err(|_| refused())?;
        (
            after,
            already,
            prior_false,
            project_added,
            project_entry_removed,
        )
    };
    if !remove && already {
        return Ok(());
    }
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

// Codex 0.163.0's TUI config/read lookup prefers a saved exact cwd entry,
// even for linked worktrees whose fallback trust target is the main checkout.
// Edit the native TOML in place without serializing unrelated settings anew.
fn edit_codex(
    before: Option<&[u8]>,
    key: &str,
    remove: bool,
    receipt: Option<&Receipt>,
) -> Result<(Vec<u8>, bool, bool, bool, bool)> {
    use toml_edit::{DocumentMut, Item, Table, value};
    let source = std::str::from_utf8(before.unwrap_or_default()).map_err(|_| refused())?;
    let mut doc = source.parse::<DocumentMut>().map_err(|_| refused())?;
    let mut new_projects = Table::new();
    new_projects.set_implicit(true);
    let projects = doc
        .entry("projects")
        .or_insert(Item::Table(new_projects))
        .as_table_like_mut()
        .ok_or_else(refused)?;
    let project_added = !projects.contains_key(key);
    let project = projects
        .entry(key)
        .or_insert(Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or_else(refused)?;
    let old = project.get("trust_level");
    let (already, prior_false) = match old {
        None => (false, false),
        Some(item) => match item.as_str() {
            Some("trusted") => (true, false),
            Some("untrusted") => (false, true),
            _ => return Err(refused()),
        },
    };
    if remove {
        if already {
            if receipt.is_some_and(|r| r.prior_false) {
                project.insert("trust_level", value("untrusted"));
            } else {
                project.remove("trust_level");
            }
        }
    } else if !already {
        project.insert("trust_level", value("trusted"));
    }
    let project_entry_removed =
        remove && !project_added && project.is_empty() && receipt.is_some_and(|r| r.project_added);
    if project_entry_removed {
        projects.remove(key);
    }
    Ok((
        doc.to_string().into_bytes(),
        already,
        prior_false,
        project_added,
        project_entry_removed,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn native_lock_serializes_writers_without_creating_home_files() {
        use std::os::fd::AsRawFd;

        let home = tempfile::tempdir().unwrap();
        let lock = lock_native(home.path()).unwrap();
        assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
        let competitor = fs::File::open(home.path()).unwrap();
        // SAFETY: competitor owns a live directory descriptor.
        assert_eq!(
            unsafe { libc::flock(competitor.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().kind(),
            std::io::ErrorKind::WouldBlock
        );
        drop(lock);
        // SAFETY: competitor still owns its live descriptor; the first lock was released.
        assert_eq!(
            unsafe { libc::flock(competitor.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
    }

    #[test]
    fn native_trust_codex_exact_entry_and_reversal() {
        assert!(eligible("codex", Permissions::Auto));
        let f = Fixture::codex(
            "# keep comment\nmodel = \"synthetic\"\n[projects.\"/other\"]\ntrust_level = \"trusted\"\n",
        );
        f.run("codex", false).unwrap();
        let native = fs::read_to_string(&f.path).unwrap();
        let doc: toml::Value = toml::from_str(&native).unwrap();
        assert_eq!(
            doc["projects"][f.worktree.to_str().unwrap()]["trust_level"].as_str(),
            Some("trusted")
        );
        assert_eq!(doc["projects"].as_table().unwrap().len(), 2);
        assert!(native.starts_with("# keep comment\nmodel = \"synthetic\""));
        assert!(
            !fs::read_to_string(f.dir.join(RECEIPT))
                .unwrap()
                .contains("synthetic")
        );
        f.run("codex", false).unwrap();
        f.run("codex", true).unwrap();
        f.run("codex", true).unwrap();
        let doc: toml::Value = toml::from_str(&fs::read_to_string(&f.path).unwrap()).unwrap();
        assert!(doc["projects"].get(f.worktree.to_str().unwrap()).is_none());
        assert_eq!(
            doc["projects"]["/other"]["trust_level"].as_str(),
            Some("trusted")
        );
        assert!(!f.dir.join(RECEIPT).exists());
    }

    #[test]
    fn native_trust_codex_preserves_existing_trust_and_inline_tables() {
        for prior in [None, Some("trusted"), Some("untrusted")] {
            let f = Fixture::codex("");
            let key = serde_json::to_string(f.worktree.to_str().unwrap()).unwrap();
            let trust = prior
                .map(|v| format!(", trust_level = \"{v}\""))
                .unwrap_or_default();
            let original = format!(
                "# private-sentinel\nprojects = {{ {key} = {{ history = [7]{trust} }} }}\n"
            );
            fs::write(&f.path, &original).unwrap();
            f.run("codex", false).unwrap();
            assert_eq!(f.dir.join(RECEIPT).exists(), prior != Some("trusted"));
            if prior == Some("trusted") {
                assert_eq!(fs::read_to_string(&f.path).unwrap(), original);
            }
            f.run("codex", true).unwrap();
            let text = fs::read_to_string(&f.path).unwrap();
            assert!(text.starts_with("# private-sentinel\n"));
            let doc: toml::Value = toml::from_str(&text).unwrap();
            let project = &doc["projects"][f.worktree.to_str().unwrap()];
            assert_eq!(project["history"].as_array().unwrap().len(), 1);
            assert_eq!(
                project.get("trust_level").and_then(toml::Value::as_str),
                prior
            );
        }
    }

    #[test]
    fn native_trust_codex_write_ahead_recovery_and_native_edits() {
        for native_written in [false, true] {
            for retry in [false, true] {
                let f = Fixture::codex("");
                let key = serde_json::to_string(f.worktree.to_str().unwrap()).unwrap();
                let prior = format!("[projects.{key}]\ntrust_level = \"untrusted\"\n");
                fs::write(&f.path, &prior).unwrap();
                f.run("codex", false).unwrap();
                if !native_written {
                    fs::write(&f.path, &prior).unwrap();
                }
                if retry {
                    f.run("codex", false).unwrap();
                }
                let mut native = fs::read_to_string(&f.path).unwrap();
                native.push_str("history = [\"native-added\"]\n");
                fs::write(&f.path, native).unwrap();
                f.run("codex", true).unwrap();
                let text = fs::read_to_string(&f.path).unwrap();
                let doc: toml::Value = toml::from_str(&text).unwrap();
                let project = &doc["projects"][f.worktree.to_str().unwrap()];
                assert_eq!(project["trust_level"].as_str(), Some("untrusted"));
                assert_eq!(project["history"][0].as_str(), Some("native-added"));
                assert!(!f.dir.join(RECEIPT).exists());
            }
        }
        let f = Fixture::codex("");
        f.run("codex", false).unwrap();
        fs::write(&f.path, "malformed private-sentinel").unwrap();
        let error = f.run("codex", true).unwrap_err().to_string();
        assert!(!error.contains("private-sentinel"));
        assert!(f.dir.join(RECEIPT).exists());
        fs::remove_file(&f.path).unwrap();
        fs::remove_dir(f.path.parent().unwrap()).unwrap();
        f.run("codex", true).unwrap();
        assert!(!f.path.exists());
        assert!(!f.dir.join(RECEIPT).exists());
    }

    #[test]
    fn native_trust_codex_unsafe_state_is_not_overwritten() {
        for config in ["projects = []", "broken private-sentinel", "projects = 7"] {
            let f = Fixture::codex(config);
            assert!(f.run("codex", false).is_err());
            assert_eq!(fs::read_to_string(&f.path).unwrap(), config);
            assert!(!f.dir.join(RECEIPT).exists());
        }
        for entry in [
            "[]",
            "{ trust_level = false }",
            "{ trust_level = \"unknown\" }",
        ] {
            let f = Fixture::codex("");
            let key = serde_json::to_string(f.worktree.to_str().unwrap()).unwrap();
            let config = format!("projects = {{ {key} = {entry} }}");
            fs::write(&f.path, &config).unwrap();
            assert!(f.run("codex", false).is_err());
            assert_eq!(fs::read_to_string(&f.path).unwrap(), config);
            assert!(!f.dir.join(RECEIPT).exists());
        }
        for attack in [
            "symlink",
            "hardlink",
            "writable",
            "parent-symlink",
            "parent-writable",
            "oversize",
        ] {
            let f = Fixture::codex("# sentinel\n");
            let other = f.home.join("other");
            match attack {
                "symlink" => {
                    fs::rename(&f.path, &other).unwrap();
                    symlink(&other, &f.path).unwrap();
                }
                "hardlink" => {
                    fs::hard_link(&f.path, &other).unwrap();
                }
                "writable" => {
                    fs::set_permissions(&f.path, fs::Permissions::from_mode(0o666)).unwrap();
                }
                "parent-symlink" => {
                    fs::rename(f.path.parent().unwrap(), &other).unwrap();
                    symlink(&other, f.path.parent().unwrap()).unwrap();
                }
                "parent-writable" => {
                    fs::set_permissions(
                        f.path.parent().unwrap(),
                        fs::Permissions::from_mode(0o777),
                    )
                    .unwrap();
                }
                "oversize" => {
                    fs::write(&f.path, vec![b' '; LIMIT as usize + 1]).unwrap();
                }
                _ => unreachable!(),
            }
            let before = fs::read(&f.path).unwrap();
            assert!(f.run("codex", false).is_err(), "{attack}");
            assert_eq!(fs::read(&f.path).unwrap(), before);
            assert!(!f.dir.join(RECEIPT).exists());
        }
    }

    #[test]
    fn native_trust_codex_literal_keys_and_native_revocation() {
        for inline in [false, true] {
            for keep_history in [false, true] {
                let mut f = Fixture::codex(if inline { "projects = {}\n" } else { "" });
                f.worktree = f
                    .worktree
                    .with_file_name("worktree.with.\"quotes\"\nand-newline");
                f.run("codex", false).unwrap();
                let key = f.worktree.to_str().unwrap();
                let mut doc = fs::read_to_string(&f.path)
                    .unwrap()
                    .parse::<toml_edit::DocumentMut>()
                    .unwrap();
                let project = doc["projects"][key].as_table_like_mut().unwrap();
                project.remove("trust_level");
                if keep_history {
                    project.insert("history", toml_edit::value("native-added"));
                }
                fs::write(&f.path, doc.to_string()).unwrap();
                f.run("codex", true).unwrap();
                let text = fs::read_to_string(&f.path).unwrap();
                let doc: toml::Value = toml::from_str(&text).unwrap();
                assert_eq!(
                    doc.get("projects").and_then(|p| p.get(key)).is_some(),
                    keep_history
                );
                if keep_history {
                    assert_eq!(
                        doc["projects"][key]["history"].as_str(),
                        Some("native-added")
                    );
                }
                assert!(!f.dir.join(RECEIPT).exists());
            }
        }
    }

    #[test]
    fn native_trust_codex_new_store_and_concurrent_tasks() {
        let f = Fixture::codex("");
        fs::remove_file(&f.path).unwrap();
        fs::remove_dir(f.path.parent().unwrap()).unwrap();
        let dir2 = f.dir.with_file_name("task2");
        fs::create_dir(&dir2).unwrap();
        let worktree2 = f.worktree.with_file_name("worktree2");
        std::thread::scope(|s| {
            s.spawn(|| f.run("codex", false).unwrap());
            s.spawn(|| update(&f.home, &dir2, "codex", &worktree2, false).unwrap());
        });
        let text = fs::read_to_string(&f.path).unwrap();
        let doc: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(doc["projects"].as_table().unwrap().len(), 2);
        f.run("codex", true).unwrap();
        let text = fs::read_to_string(&f.path).unwrap();
        let doc: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(doc["projects"].as_table().unwrap().len(), 1);
        assert_eq!(
            doc["projects"][worktree2.to_str().unwrap()]["trust_level"].as_str(),
            Some("trusted")
        );
        update(&f.home, &dir2, "codex", &worktree2, true).unwrap();
        assert!(fs::read_to_string(&f.path).unwrap().trim().is_empty());
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
        fn codex(config: &str) -> Self {
            let mut fixture = Self::new("claude-code", json!({}));
            fs::remove_file(&fixture.path).unwrap();
            let parent = fixture.home.join(".codex");
            fs::create_dir(&parent).unwrap();
            fixture.path = parent.join("config.toml");
            fs::write(&fixture.path, config).unwrap();
            fixture
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
                        && matches!(harness, "claude-code" | "antigravity" | "codex")
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

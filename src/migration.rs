//! Explicit fixed-path migration. A durable journal contains exact source
//! backups and deterministic targets. Recovery only touches matching bytes;
//! concurrent user edits stop rollback instead of being overwritten.
use crate::private_io::{self, Durability};
use crate::util::{Error, Result, digest_bytes, display_safe_block};
use crate::{bail, config, context_lock, git::Repo, okf};
use serde::{Deserialize, Serialize};
use std::path::Path;

const JOURNAL: &str = ".ahu/migrations/project-okf-v1/journal.json";
const GUARD: &str = ".ahu/migrations/project-okf-v1/guard";
const PATHS: [&str; 4] = [
    config::LEGACY_CONFIG_PATH,
    context_lock::LEGACY_LOCK_PATH,
    config::CONFIG_RELATIVE_PATH,
    context_lock::LOCK_PATH,
];
const JOURNAL_LIMIT: u64 = 24 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    schema_version: u32,
    phase: String,
    repository: String,
    base_commit: Option<String>,
    before: [Option<String>; 4],
    after: [Option<String>; 4],
    source_digests: [Option<String>; 4],
    source_modes: [Option<u32>; 4],
}
fn read(root: &Path, relative: &str) -> Result<Option<String>> {
    crate::util::resolve_existing_within(root, relative)?
        .map(|path| {
            String::from_utf8(okf::read(&path)?)
                .map_err(|_| Error::new("migration input is not UTF-8"))
        })
        .transpose()
}
fn file_mode(root: &Path, relative: &str) -> Result<Option<u32>> {
    use std::os::unix::fs::PermissionsExt;
    crate::util::resolve_existing_within(root, relative)?
        .map(|path| {
            std::fs::symlink_metadata(path)
                .map(|metadata| metadata.permissions().mode() & 0o777)
                .map_err(Into::into)
        })
        .transpose()
}
fn read_journal(root: &Path) -> Result<Option<Plan>> {
    let Some(path) = crate::util::resolve_existing_within(root, JOURNAL)? else {
        return Ok(None);
    };
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        bail!("migration journal must be a regular file");
    }
    let bytes = crate::util::read_limited(&mut file, JOURNAL_LIMIT)?;
    let plan: Plan =
        serde_json::from_slice(&bytes).map_err(|_| Error::new("invalid migration journal"))?;
    if plan.schema_version != 1
        || !matches!(plan.phase.as_str(), "prepared" | "complete" | "rolled-back")
    {
        bail!("unsupported migration journal");
    }
    for (value, digest) in plan.before.iter().zip(&plan.source_digests) {
        if value.as_ref().map(|s| digest_bytes(s.as_bytes())) != *digest {
            bail!("migration backup digest mismatch");
        }
    }
    for value in plan.before.iter().chain(&plan.after).flatten() {
        if value.len() as u64 > okf::MAX_BYTES {
            bail!("migration journal document exceeds limit");
        }
    }
    if plan.before[0].is_some() == plan.before[2].is_some()
        || (plan.before[1].is_some() && plan.before[3].is_some())
        || (plan.before[0].is_none() && plan.before[1].is_none())
        || (plan.before[2].is_some() && plan.before[2] != plan.after[2])
        || plan.after[0].is_some()
        || plan.after[1].is_some()
        || plan.after[2].is_none()
        || plan.after[3].is_none()
    {
        bail!("unexpected migration journal destinations");
    }
    for (value, mode) in plan.before.iter().zip(&plan.source_modes) {
        if value.is_some() != mode.is_some() || mode.is_some_and(|m| m & !0o777 != 0) {
            bail!("invalid migration source mode");
        }
    }
    // Historical backups must not pin future readers to the catalog version
    // supported at migration time. Only a pending recovery needs their schema.
    if plan.phase != "prepared" {
        return Ok(Some(plan));
    }
    let old = config::parse(
        plan.before[0]
            .as_ref()
            .or(plan.before[2].as_ref())
            .unwrap()
            .as_bytes(),
        plan.before[0].is_some(),
        Path::new(if plan.before[0].is_some() {
            PATHS[0]
        } else {
            PATHS[2]
        }),
    )?;
    let new = config::parse(
        plan.after[2].as_ref().unwrap().as_bytes(),
        false,
        Path::new(PATHS[2]),
    )?;
    if old != new {
        bail!("migration journal effective policy differs");
    }
    context_lock::LockFile::decode(plan.after[3].as_ref().unwrap().as_bytes(), false)?;
    Ok(Some(plan))
}
/// Every shared reader/writer refuses an unfinished transaction. No automatic
/// recovery: inspection must not write and interrupted policy must not launch.
pub fn ensure_idle(root: &Path) -> Result<()> {
    if read_journal(root)?.is_some_and(|p| p.phase == "prepared") {
        bail!("project migration is incomplete; run `ahu migrate --rollback` before using policy");
    }
    Ok(())
}
fn refuse_task(repo: &Repo) -> Result<()> {
    let root = repo.root.canonicalize()?;
    let managed = repo.primary_root()?.join(crate::state::WORKTREES_DIR);
    if root.starts_with(&managed)
        || crate::git::run_ok(&repo.root, &["branch", "--show-current"])?.starts_with("ahu/")
    {
        bail!("migration refuses managed task worktrees; existing task pins must remain exact");
    }
    Ok(())
}
impl Plan {
    pub fn preview(repo: &Repo) -> Result<Self> {
        let current_repo = crate::git::discover(&repo.root)?;
        let repo = &current_repo;
        ensure_idle(&repo.root)?;
        let previous = read_journal(&repo.root)?;
        let mut before: [Option<String>; 4] = Default::default();
        for (index, path) in PATHS.iter().enumerate() {
            before[index] = read(&repo.root, path)?;
        }
        if let Some(previous) = previous {
            if previous.phase == "complete" {
                bail!("a completed migration journal already exists; its backups are preserved");
            }
            if previous.before != before {
                bail!(
                    "legacy sources differ from the retained rollback backups; preserve and reconcile the existing journal before retrying"
                );
            }
        }
        if (before[0].is_some() && before[2].is_some())
            || (before[1].is_some() && before[3].is_some())
        {
            bail!("migration refuses conflicting formats; no precedence is implied");
        }
        if before[0].is_none() && before[1].is_none() {
            bail!("no legacy project documents to migrate");
        }
        let policy_index = if before[0].is_some() { 0 } else { 2 };
        let policy = before[policy_index]
            .as_ref()
            .ok_or_else(|| Error::new("no project policy for lock migration"))?;
        let config = config::parse(
            policy.as_bytes(),
            policy_index == 0,
            Path::new(PATHS[policy_index]),
        )?;
        // An existing canonical policy is already authoritative: retain every byte.
        let target = if policy_index == 0 {
            config::try_render(&config)?
        } else {
            policy.clone()
        };
        if config != config::parse(target.as_bytes(), false, Path::new(PATHS[2]))? {
            bail!("effective policy parity failed");
        }
        let mut snapshot = crate::snapshot::collect(&repo.root)?;
        let lock_index = if before[1].is_some() { 1 } else { 3 };
        if let Some(old) = &before[lock_index] {
            let lock = context_lock::LockFile::decode(old.as_bytes(), lock_index == 1)?;
            if !lock.matches_snapshot(&snapshot) {
                bail!(
                    "existing lock does not match current shared context; resolve context drift before migration"
                );
            }
        }
        if policy_index == 0 {
            let entry = snapshot
                .entries
                .iter_mut()
                .find(|e| e.path == PATHS[0])
                .ok_or_else(|| Error::new("legacy policy is absent from the context snapshot"))?;
            entry.path = PATHS[2].into();
            entry.bytes = target.len() as u64;
            entry.digest = digest_bytes(target.as_bytes());
            entry.executable = false;
            snapshot.entries.sort_by(|a, b| a.path.cmp(&b.path));
        }
        // Apply creates ignored local journal storage. Predict its disclosed
        // scan gap without making preview write anything.
        if !snapshot.skipped_directories.iter().any(|p| p == ".ahu") {
            snapshot.skipped_directories.push(".ahu".into());
            snapshot.skipped_directories.sort();
        }
        let lock = context_lock::LockFile::from_snapshot(&snapshot).render(before[3].as_deref())?;
        use std::os::unix::fs::PermissionsExt;
        let mut source_modes = [None; 4];
        for (index, path) in PATHS.iter().enumerate() {
            if before[index].is_some() {
                source_modes[index] = Some(
                    std::fs::symlink_metadata(repo.root.join(path))?
                        .permissions()
                        .mode()
                        & 0o777,
                );
            }
        }
        let source_digests = before
            .clone()
            .map(|value| value.map(|s| digest_bytes(s.as_bytes())));
        Ok(Self {
            schema_version: 1,
            phase: "prepared".into(),
            repository: repo.identity(),
            base_commit: repo.head.clone(),
            before,
            after: [None, None, Some(target), Some(lock)],
            source_digests,
            source_modes,
        })
    }
    pub fn display(&self) -> String {
        let mut out = String::from(
            "Effective typed policy parity: identical (including defaults and ranking order).\nLegacy sources are backed up in the private migration journal; task pins and local acceptance stay unchanged.\n",
        );
        for (index, path) in PATHS.iter().enumerate().skip(2) {
            let document = self.after[index].as_ref().unwrap();
            out.push_str(&format!(
                "\nTarget: {}\nSHA-256: {}\n{}",
                path,
                digest_bytes(document.as_bytes()),
                display_safe_block(document)
            ));
        }
        out
    }
}
fn guard(root: &Path) -> Result<std::fs::File> {
    use std::os::unix::{fs::OpenOptionsExt, io::AsRawFd};
    crate::state::ensure_checkout_state(root)?;
    let path = crate::util::resolve_within(root, GUARD, true)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        bail!("migration guard must be a regular file");
    }
    // SAFETY: file owns a live descriptor. The lock is released when it drops.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("another migration is active");
    }
    Ok(file)
}
fn save(root: &Path, plan: &Plan) -> Result<()> {
    let path = crate::util::resolve_within(root, JOURNAL, true)?;
    let bytes =
        serde_json::to_vec(plan).map_err(|_| Error::new("cannot encode migration journal"))?;
    if bytes.len() as u64 > JOURNAL_LIMIT {
        bail!("migration journal exceeds limit");
    }
    private_io::atomic_write(&path, &bytes, Durability::Durable)
}
fn matches(root: &Path, expected: &[Option<String>; 4]) -> Result<()> {
    for (path, value) in PATHS.iter().zip(expected) {
        if read(root, path)? != *value {
            bail!("concurrent edit at {path}; migration stopped");
        }
    }
    Ok(())
}
fn replace(
    root: &Path,
    index: usize,
    expected: &Option<String>,
    target: &Option<String>,
) -> Result<()> {
    if read(root, PATHS[index])? != *expected {
        bail!("concurrent edit; migration stopped");
    }
    if expected == target {
        return Ok(());
    }
    let path = crate::util::resolve_within(root, PATHS[index], true)?;
    if let Some(text) = target {
        if expected.is_some() {
            if index != 3 {
                bail!("migration never overwrites existing policy");
            }
            // Only the generated canonical lock may be refreshed. Its exact
            // bytes are journaled and checked above, including during rollback.
            private_io::atomic_write(&path, text.as_bytes(), Durability::Durable)?;
        } else if !private_io::atomic_create(&path, text.as_bytes(), Durability::Durable)? {
            bail!("migration destination appeared concurrently");
        }
    } else if expected.is_some() {
        std::fs::remove_file(&path)?;
        std::fs::File::open(path.parent().unwrap())?.sync_all()?;
    }
    Ok(())
}
fn check_identity(repo: &Repo, plan: &Plan) -> Result<()> {
    let current = crate::git::discover(&repo.root)?;
    if current.identity() != plan.repository || current.head != plan.base_commit {
        bail!("repository or HEAD changed since migration preview");
    }
    refuse_task(&current)
}
pub fn apply(repo: &Repo, plan: &Plan) -> Result<()> {
    apply_with(repo, plan, |_| Ok(()))
}
fn apply_with(
    repo: &Repo,
    plan: &Plan,
    mut checkpoint: impl FnMut(usize) -> Result<()>,
) -> Result<()> {
    refuse_task(repo)?;
    check_identity(repo, plan)?;
    matches(&repo.root, &plan.before)?;
    let _guard = guard(&repo.root)?;
    ensure_idle(&repo.root)?;
    matches(&repo.root, &plan.before)?;
    // Recompute against current context, not only the two source files.
    let fresh = Plan::preview(repo)?;
    if fresh.after != plan.after
        || fresh.source_modes != plan.source_modes
        || fresh.repository != plan.repository
        || fresh.base_commit != plan.base_commit
    {
        bail!("context changed since migration preview");
    }
    save(&repo.root, &fresh)?;
    let mut expected = plan.before.clone();
    // Publish complete targets first. Readers refuse the pending journal.
    let result = (|| {
        for (step, index) in [2, 3, 0, 1].into_iter().enumerate() {
            checkpoint(step)?;
            matches(&repo.root, &expected)?;
            replace(&repo.root, index, &expected[index], &plan.after[index])?;
            expected[index] = plan.after[index].clone();
        }
        matches(&repo.root, &plan.after)?;
        check_identity(repo, plan)?;
        let snapshot = crate::snapshot::collect(&repo.root)?;
        let target_lock =
            context_lock::LockFile::decode(plan.after[3].as_ref().unwrap().as_bytes(), false)?;
        if !target_lock.matches_snapshot(&snapshot) {
            bail!("context changed during migration");
        }
        let mut complete = fresh.clone();
        complete.phase = "complete".into();
        save(&repo.root, &complete)
    })();
    if let Err(error) = result {
        match rollback_inner(repo) {
            Ok(()) => {
                return Err(Error::new(format!(
                    "migration failed and rolled back: {error}"
                )));
            }
            Err(recovery) => {
                return Err(Error::new(format!(
                    "migration failed: {error}; rollback needs attention: {recovery}; backups remain in {JOURNAL}"
                )));
            }
        }
    }
    Ok(())
}
fn rollback_inner(repo: &Repo) -> Result<()> {
    let mut plan = read_journal(&repo.root)?.ok_or_else(|| Error::new("no migration journal"))?;
    if plan.repository != repo.identity() {
        bail!("migration journal belongs to a different repository");
    }
    if plan.phase != "prepared" {
        bail!("only an interrupted migration can be rolled back");
    }
    // Validate every path before writing anything; do not clobber later edits.
    for (index, path) in PATHS.iter().enumerate() {
        let current = read(&repo.root, path)?;
        if current != plan.before[index] && current != plan.after[index] {
            bail!("concurrent edit at {path}; preserve it and resolve before rollback");
        }
        if plan.before[index] != plan.after[index]
            && let Some(mode) = file_mode(&repo.root, path)?
        {
            // Atomic restoration publishes owner-only bytes before the separate
            // chmod that restores the original mode. Accept that exact
            // interruption state and the original mode; refuse other chmods.
            let original = plan.source_modes[index];
            let expected = if current == plan.before[index] {
                Some(mode) == original || mode == 0o600
            } else {
                mode == 0o600
            };
            if !expected {
                bail!(
                    "concurrent permission change at {path}; preserve it and resolve before rollback"
                );
            }
        }
    }
    for index in [0, 1, 3, 2] {
        let current = read(&repo.root, PATHS[index])?;
        if current != plan.before[index] {
            replace(&repo.root, index, &plan.after[index], &plan.before[index])?;
        }
        if plan.before[index] != plan.after[index]
            && let Some(mode) = plan.source_modes[index]
            && file_mode(&repo.root, PATHS[index])? != Some(mode)
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                repo.root.join(PATHS[index]),
                std::fs::Permissions::from_mode(mode),
            )?;
        }
    }
    plan.phase = "rolled-back".into();
    save(&repo.root, &plan)
}
pub fn rollback(repo: &Repo) -> Result<()> {
    refuse_task(repo)?;
    let _guard = guard(&repo.root)?;
    rollback_inner(repo)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_AUTHOR_NAME", "fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn fixture() -> (tempfile::TempDir, Repo) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q", "-b", "main"]);
        std::fs::create_dir_all(root.join(config::CONFIG_DIR)).unwrap();
        std::fs::write(root.join(PATHS[0]), format!("schema_version = 1\nharness_preferences = ['codex']\nmodel_selection = 'project-ranked'\ncatalog_version = '{}'\n[model_rankings]\ncodex = ['gpt-6-astra']\n", crate::catalog::CATALOG_VERSION)).unwrap();
        let snapshot = crate::snapshot::collect(root).unwrap();
        let lock = context_lock::LockFile::from_snapshot(&snapshot);
        std::fs::write(root.join(PATHS[1]), toml::to_string(&lock).unwrap()).unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-qm", "fixture"]);
        let repo = crate::git::discover(root).unwrap();
        (dir, repo)
    }
    #[test]
    fn stale_head_is_refused() {
        let (_dir, repo) = fixture();
        let plan = Plan::preview(&repo).unwrap();
        git(
            &repo.root,
            &["commit", "--allow-empty", "-qm", "new fixture head"],
        );
        assert!(apply(&repo, &plan).is_err(), "stale HEAD was accepted");
        matches(&repo.root, &plan.before).unwrap();
    }

    #[test]
    fn context_edits_during_apply_are_refused() {
        let (_dir, repo) = fixture();
        let plan = Plan::preview(&repo).unwrap();
        let error = apply_with(&repo, &plan, |step| {
            if step == 3 {
                std::fs::write(repo.root.join("AGENTS.md"), "Concurrent context edit")?;
            }
            Ok(())
        });
        assert!(
            error.is_err(),
            "concurrent context was silently pinned stale"
        );
        matches(&repo.root, &plan.before).unwrap();
        assert_eq!(
            std::fs::read_to_string(repo.root.join("AGENTS.md")).unwrap(),
            "Concurrent context edit"
        );
    }

    #[test]
    fn migration_supports_mixed_policy_and_lock_formats() {
        for canonical_policy in [false, true] {
            let (_dir, repo) = fixture();
            if canonical_policy {
                let settings = config::load(&repo.root).unwrap().unwrap().config;
                std::fs::remove_file(repo.root.join(PATHS[0])).unwrap();
                config::write_new(&repo.root, &settings).unwrap();
                let lock = context_lock::LockFile::from_snapshot(
                    &crate::snapshot::collect(&repo.root).unwrap(),
                );
                std::fs::write(repo.root.join(PATHS[1]), toml::to_string(&lock).unwrap()).unwrap();
            } else {
                std::fs::remove_file(repo.root.join(PATHS[1])).unwrap();
                let lock = context_lock::LockFile::from_snapshot(
                    &crate::snapshot::collect(&repo.root).unwrap(),
                );
                let mut doc =
                    okf::Document::parse(&lock.render(None).unwrap(), "ahu:lock").unwrap();
                doc.fields
                    .insert("notes".into(), serde_json::json!({"nested": ["retained"]}));
                doc.body = "\nRetain this explanation.\n".into();
                std::fs::write(repo.root.join(PATHS[3]), doc.render().unwrap()).unwrap();
            }
            let plan = Plan::preview(&repo).unwrap();
            for failure in 0..4 {
                assert!(
                    apply_with(&repo, &plan, |step| if step == failure {
                        Err(Error::new("synthetic failure"))
                    } else {
                        Ok(())
                    })
                    .is_err()
                );
                matches(&repo.root, &plan.before).unwrap();
            }
            // A process can disappear without executing the error handler.
            for stop in 0..=4 {
                let guard_file = guard(&repo.root).unwrap();
                save(&repo.root, &plan).unwrap();
                for index in [2, 3, 0, 1].into_iter().take(stop) {
                    replace(&repo.root, index, &plan.before[index], &plan.after[index]).unwrap();
                }
                assert!(config::load(&repo.root).is_err());
                drop(guard_file);
                rollback(&repo).unwrap();
                matches(&repo.root, &plan.before).unwrap();
            }
            apply(&repo, &plan).unwrap();
            matches(&repo.root, &plan.after).unwrap();
            if canonical_policy {
                assert_eq!(plan.before[2], plan.after[2]);
            } else {
                let doc =
                    okf::Document::parse(plan.after[3].as_ref().unwrap(), "ahu:lock").unwrap();
                assert_eq!(
                    doc.fields["notes"],
                    serde_json::json!({"nested": ["retained"]})
                );
                assert_eq!(doc.body, "\nRetain this explanation.\n");
            }
        }
    }

    #[test]
    fn migration_display_escapes_preserved_terminal_controls() {
        let hostile_body =
            "\nretained \u{1b}[2J\u{1b}[H\u{1b}]52;c;marker\u{7}\u{009b}31m\u{202e} end\n";
        for canonical_policy in [false, true] {
            let (_dir, repo) = fixture();
            if canonical_policy {
                let settings = config::load(&repo.root).unwrap().unwrap().config;
                std::fs::remove_file(repo.root.join(PATHS[0])).unwrap();
                config::write_new(&repo.root, &settings).unwrap();
                let policy_path = repo.root.join(PATHS[2]);
                let mut policy = okf::Document::parse(
                    &std::fs::read_to_string(&policy_path).unwrap(),
                    "ahu:project",
                )
                .unwrap();
                policy.body = hostile_body.to_owned();
                std::fs::write(&policy_path, policy.render().unwrap()).unwrap();
                let snapshot = crate::snapshot::collect(&repo.root).unwrap();
                let lock = context_lock::LockFile::from_snapshot(&snapshot);
                std::fs::write(repo.root.join(PATHS[1]), toml::to_string(&lock).unwrap()).unwrap();
            } else {
                std::fs::remove_file(repo.root.join(PATHS[1])).unwrap();
                let snapshot = crate::snapshot::collect(&repo.root).unwrap();
                let lock = context_lock::LockFile::from_snapshot(&snapshot);
                let mut doc =
                    okf::Document::parse(&lock.render(None).unwrap(), "ahu:lock").unwrap();
                doc.body = hostile_body.to_owned();
                std::fs::write(repo.root.join(PATHS[3]), doc.render().unwrap()).unwrap();
            }

            let plan = Plan::preview(&repo).unwrap();
            let rendered = plan.display();
            assert!(!rendered.contains('\u{1b}'));
            assert!(!rendered.contains('\u{7}'));
            assert!(!rendered.contains('\u{009b}'));
            assert!(!rendered.contains('\u{202e}'));
            assert!(rendered.contains(r"\x1b[2J"));
            assert!(rendered.contains(r"\x1b]52;c;marker\x07\x9b31m\u{202e}"));

            apply(&repo, &plan).unwrap();
            let preserved_path = if canonical_policy { PATHS[2] } else { PATHS[3] };
            let preserved = okf::Document::parse(
                &std::fs::read_to_string(repo.root.join(preserved_path)).unwrap(),
                if canonical_policy {
                    "ahu:project"
                } else {
                    "ahu:lock"
                },
            )
            .unwrap();
            assert_eq!(preserved.body, hostile_body);
        }
    }

    #[test]
    fn exclusive_guard_and_lock_edits_leave_user_bytes_intact() {
        let (_dir, repo) = fixture();
        let plan = Plan::preview(&repo).unwrap();
        let guard_file = guard(&repo.root).unwrap();
        assert!(
            apply(&repo, &plan)
                .unwrap_err()
                .to_string()
                .contains("another migration")
        );
        matches(&repo.root, &plan.before).unwrap();
        drop(guard_file);
        std::fs::remove_file(repo.root.join(PATHS[1])).unwrap();
        let snapshot = crate::snapshot::collect(&repo.root).unwrap();
        context_lock::refresh(&repo, &snapshot).unwrap();
        let plan = Plan::preview(&repo).unwrap();
        let error = apply_with(&repo, &plan, |step| {
            if step == 1 {
                std::fs::write(repo.root.join(PATHS[3]), "Concurrent lock annotation")?;
            }
            Ok(())
        })
        .unwrap_err();
        assert!(error.to_string().contains("rollback needs attention"));
        assert_eq!(
            read(&repo.root, PATHS[3]).unwrap().unwrap(),
            "Concurrent lock annotation"
        );
        std::fs::write(repo.root.join(PATHS[3]), plan.before[3].as_ref().unwrap()).unwrap();
        rollback(&repo).unwrap();
        matches(&repo.root, &plan.before).unwrap();
    }

    #[test]
    fn preview_is_read_only_apply_preserves_policy_and_lock_is_deterministic() {
        let (_dir, repo) = fixture();
        let before = config::load(&repo.root).unwrap().unwrap();
        assert!(
            context_lock::check(&repo, &crate::snapshot::collect(&repo.root).unwrap())
                .unwrap()
                .current
        );
        let plan = Plan::preview(&repo).unwrap();
        assert_eq!(plan.display(), Plan::preview(&repo).unwrap().display());
        assert!(!repo.root.join(".ahu").exists());
        assert!(
            crate::git::run_ok(&repo.root, &["status", "--porcelain"])
                .unwrap()
                .is_empty()
        );
        apply(&repo, &plan).unwrap();
        let after = config::load(&repo.root).unwrap().unwrap();
        assert_eq!(before.config, after.config);
        assert_ne!(before.digest, after.digest);
        assert!(!repo.root.join(PATHS[0]).exists());
        assert!(!repo.root.join(PATHS[1]).exists());
        let snapshot = crate::snapshot::collect(&repo.root).unwrap();
        assert!(!snapshot.entries.iter().any(|e| e.path == PATHS[3]));
        let locked = std::fs::read(repo.root.join(PATHS[3])).unwrap();
        context_lock::refresh(&repo, &snapshot).unwrap();
        assert_eq!(locked, std::fs::read(repo.root.join(PATHS[3])).unwrap());
        assert!(!context_lock::check(&repo, &snapshot).unwrap().current);
        git(&repo.root, &["add", "-A"]);
        git(&repo.root, &["commit", "-qm", "migrate fixture"]);
        assert!(context_lock::check(&repo, &snapshot).unwrap().current);
        assert_eq!(
            read_journal(&repo.root).unwrap().unwrap().before,
            plan.before
        );
        assert!(
            !crate::git::run_ok(&repo.root, &["ls-files", JOURNAL])
                .unwrap()
                .contains("journal")
        );
    }
    #[test]
    fn failures_at_each_boundary_restore_sources_and_keep_backup() {
        for fail in 0..4 {
            let (_dir, repo) = fixture();
            let plan = Plan::preview(&repo).unwrap();
            assert!(
                apply_with(&repo, &plan, |step| if step == fail {
                    Err(Error::new("synthetic interruption"))
                } else {
                    Ok(())
                })
                .is_err()
            );
            matches(&repo.root, &plan.before).unwrap();
            assert_eq!(
                read_journal(&repo.root).unwrap().unwrap().phase,
                "rolled-back"
            );
            assert_eq!(
                config::load(&repo.root).unwrap().unwrap().digest,
                plan.source_digests[0].as_ref().unwrap().as_str()
            );
        }
    }
    #[test]
    fn recovery_refuses_concurrent_edits_then_recovers_interrupted_transaction() {
        let (_dir, repo) = fixture();
        let plan = Plan::preview(&repo).unwrap();
        let _guard = guard(&repo.root).unwrap();
        save(&repo.root, &plan).unwrap();
        replace(&repo.root, 2, &None, &plan.after[2]).unwrap();
        assert!(config::load(&repo.root).is_err());
        std::fs::write(repo.root.join(PATHS[2]), "concurrent edit").unwrap();
        assert!(rollback_inner(&repo).is_err());
        assert_eq!(
            read(&repo.root, PATHS[2]).unwrap().unwrap(),
            "concurrent edit"
        );
        std::fs::write(repo.root.join(PATHS[2]), plan.after[2].as_ref().unwrap()).unwrap();
        rollback_inner(&repo).unwrap();
        matches(&repo.root, &plan.before).unwrap();
    }
    #[test]
    fn edits_during_apply_stop_without_overwriting_and_modes_survive_recovery() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, repo) = fixture();
        std::fs::set_permissions(
            repo.root.join(PATHS[1]),
            std::fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        let plan = Plan::preview(&repo).unwrap();
        let error = apply_with(&repo, &plan, |step| {
            if step == 1 {
                std::fs::write(repo.root.join(PATHS[0]), "synthetic concurrent edit")?;
            }
            Ok(())
        })
        .unwrap_err();
        assert!(error.to_string().contains("rollback needs attention"));
        assert_eq!(
            read(&repo.root, PATHS[0]).unwrap().unwrap(),
            "synthetic concurrent edit"
        );
        assert_eq!(read_journal(&repo.root).unwrap().unwrap().phase, "prepared");
        std::fs::write(repo.root.join(PATHS[0]), plan.before[0].as_ref().unwrap()).unwrap();
        rollback(&repo).unwrap();
        matches(&repo.root, &plan.before).unwrap();
        // Simulate power loss after removing both old sources.
        save(&repo.root, &plan).unwrap();
        for index in [2, 3, 0, 1] {
            replace(&repo.root, index, &plan.before[index], &plan.after[index]).unwrap();
        }
        rollback(&repo).unwrap();
        matches(&repo.root, &plan.before).unwrap();
        assert_eq!(
            std::fs::metadata(repo.root.join(PATHS[1]))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
    }

    #[test]
    fn interrupted_permission_restore_is_repaired_but_concurrent_modes_are_preserved() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, repo) = fixture();
        let source = repo.root.join(PATHS[0]);
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o755)).unwrap();
        let snapshot = crate::snapshot::collect(&repo.root).unwrap();
        std::fs::write(
            repo.root.join(PATHS[3]),
            context_lock::LockFile::from_snapshot(&snapshot)
                .render(None)
                .unwrap(),
        )
        .unwrap();
        std::fs::remove_file(repo.root.join(PATHS[1])).unwrap();
        git(&repo.root, &["add", "-A", "-f"]);
        git(
            &repo.root,
            &["commit", "--amend", "--allow-empty", "-qm", "fixture mode"],
        );
        let plan = Plan::preview(&repo).unwrap();
        save(&repo.root, &plan).unwrap();

        // Simulate a process stopping after atomic byte restoration (0600) but
        // before rollback applies the journaled source mode (0755).
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
        rollback(&repo).unwrap();
        assert_eq!(read(&repo.root, PATHS[0]).unwrap(), plan.before[0]);
        assert_eq!(file_mode(&repo.root, PATHS[0]).unwrap(), Some(0o755));

        let (_dir, repo) = fixture();
        let source = repo.root.join(PATHS[0]);
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o755)).unwrap();
        let snapshot = crate::snapshot::collect(&repo.root).unwrap();
        std::fs::write(
            repo.root.join(PATHS[3]),
            context_lock::LockFile::from_snapshot(&snapshot)
                .render(None)
                .unwrap(),
        )
        .unwrap();
        std::fs::remove_file(repo.root.join(PATHS[1])).unwrap();
        git(&repo.root, &["add", "-A", "-f"]);
        git(
            &repo.root,
            &["commit", "--amend", "--allow-empty", "-qm", "fixture mode"],
        );
        let plan = Plan::preview(&repo).unwrap();
        save(&repo.root, &plan).unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o640)).unwrap();
        assert!(
            rollback(&repo)
                .unwrap_err()
                .to_string()
                .contains("concurrent permission change")
        );
        assert_eq!(read(&repo.root, PATHS[0]).unwrap(), plan.before[0]);
        assert_eq!(file_mode(&repo.root, PATHS[0]).unwrap(), Some(0o640));
    }
    #[test]
    fn journal_symlink_and_changed_context_are_refused() {
        let (_dir, repo) = fixture();
        let plan = Plan::preview(&repo).unwrap();
        std::fs::write(repo.root.join("AGENTS.md"), "changed shared input").unwrap();
        assert!(apply(&repo, &plan).is_err());
        matches(&repo.root, &plan.before).unwrap();
        std::fs::remove_file(repo.root.join("AGENTS.md")).unwrap();
        let target = tempfile::NamedTempFile::new().unwrap();
        let journal = crate::util::resolve_within(&repo.root, JOURNAL, true).unwrap();
        std::os::unix::fs::symlink(target.path(), journal).unwrap();
        assert!(Plan::preview(&repo).is_err());
        assert!(rollback(&repo).is_err());
        assert!(std::fs::read(target.path()).unwrap().is_empty());
    }

    #[test]
    fn stale_preview_conflicts_symlinks_and_managed_checkouts_are_refused() {
        let (_dir, repo) = fixture();
        let plan = Plan::preview(&repo).unwrap();
        std::fs::write(repo.root.join(PATHS[0]), "concurrent source edit").unwrap();
        assert!(apply(&repo, &plan).is_err());
        std::fs::write(repo.root.join(PATHS[0]), plan.before[0].as_ref().unwrap()).unwrap();
        std::fs::write(repo.root.join(PATHS[2]), plan.after[2].as_ref().unwrap()).unwrap();
        assert!(Plan::preview(&repo).is_err());
        assert!(config::load(&repo.root).is_err());
        std::fs::remove_file(repo.root.join(PATHS[2])).unwrap();
        std::os::unix::fs::symlink("config.toml", repo.root.join(PATHS[2])).unwrap();
        assert!(Plan::preview(&repo).is_err());
        std::fs::remove_file(repo.root.join(PATHS[2])).unwrap();
        git(&repo.root, &["checkout", "-qb", "ahu/fixture"]);
        assert!(
            apply(&repo, &plan)
                .unwrap_err()
                .to_string()
                .contains("managed task")
        );
    }
}

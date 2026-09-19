//! Task identity and the local task record.
//!
//! A task record freezes what a launch actually used. Editing an agent
//! definition later changes subsequent launches; a task that has already
//! started keeps the snapshot recorded here.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bail;
use crate::harness::{EnforcementReport, LaunchCommand};
use crate::hooks::HookInventory;
use crate::snapshot::{ConfigSnapshot, MaterializeReport};
use crate::state;
use crate::util::{Error, Result};

/// How a launch chose its harness and model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LaunchMode {
    /// `@name` — the agent's manifest is authoritative.
    Named,
    /// No selector — the project policy snapshot is authoritative.
    Automatic,
}

/// Life cycle of a task session, as far as ahu can actually observe it.
///
/// `Exited` means the harness process ended. It is not a claim that the task
/// was completed successfully. `Cancelled` means ahu terminated the harness
/// process tree on request; like `Exited`, it is terminal and is not a claim
/// of success.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TaskState {
    Starting,
    Running,
    Exited,
    Failed,
    Cancelled,
}

impl TaskState {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Starting => "starting",
            TaskState::Running => "running",
            TaskState::Exited => "exited",
            TaskState::Failed => "failed",
            TaskState::Cancelled => "cancelled",
        }
    }

    /// A state whose session may still be doing work. Only these states can
    /// meaningfully be cancelled; the rest describe sessions that already
    /// stopped for some reason.
    pub fn is_live(self) -> bool {
        matches!(self, TaskState::Starting | TaskState::Running)
    }
}

/// The frozen identity of one launch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LaunchIdentity {
    pub mode: LaunchMode,
    /// Agent name, or `auto` for an automatic launch.
    pub agent: String,
    /// Semantic version of the named agent, absent for automatic launches.
    pub agent_version: Option<String>,
    /// Approval widening this launch was configured with.
    #[serde(default)]
    pub permissions: crate::agent::Permissions,
    pub harness: String,
    pub model: String,
    /// Where the agent's instructions came from, repository-relative. ahu
    /// delivers them in the prompt; no harness flag selects them.
    pub instructions_source: Option<String>,
    /// Digest of the complete file at `instructions_source`, frontmatter
    /// included — the file as a reviewer would find it in the repository.
    pub source_digest: Option<String>,
    /// Digest of exactly the instruction text ahu delivered, which for a format
    /// with frontmatter is the file with that frontmatter stripped.
    ///
    /// This schema binds the digest to delivered text, not the whole source
    /// file. `load` must refuse incompatible schema versions rather than
    /// reinterpret a digest with a different byte scope.
    pub instructions_digest: Option<String>,
    /// Digest binding manifest fields and both file digests together.
    pub identity_digest: Option<String>,
    /// Why this pair, for automatic launches.
    pub selection_basis: Option<String>,
}

/// Everything ahu recorded about a task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskRecord {
    pub schema_version: u32,
    pub task_id: String,
    pub title: String,
    /// Plain sidebar description; absent in older records.
    #[serde(default)]
    pub summary: String,
    pub created_at: String,
    pub repo_identity: String,
    pub repo_root: PathBuf,
    pub branch: String,
    pub worktree: PathBuf,
    pub base_commit: Option<String>,
    pub identity: LaunchIdentity,
    pub policy_digest: String,
    pub catalog_version: String,
    pub config_snapshot: ConfigSnapshot,
    pub config_snapshot_digest: String,
    /// Hooks that were in effect at launch, at every scope ahu could read.
    #[serde(default)]
    pub hooks: HookInventory,
    /// Digest over those hooks, so a change at any scope shows up as drift.
    #[serde(default)]
    pub hooks_digest: String,
    pub materialize: MaterializeReport,
    /// The launch command with the prompt redacted. The prompt itself lives only
    /// in `prompt.txt`, which is written owner-only.
    pub launch_command: LaunchCommand,
    /// What ahu put in the harness's prompt slot.
    ///
    /// The redacted command cannot cover any of it -- the contract, the agent's
    /// instructions and the prompt are all inside the one argv element redaction
    /// replaces -- so this carries its own digest and `run_task` checks it.
    pub delivery: crate::orchestration::Delivery,
    /// Digest of the submitted prompt, so the prompt file can be verified
    /// without a second copy of its contents existing.
    ///
    /// Deliberately not `#[serde(default)]`: an absent integrity value must be a
    /// load failure with a clear message, not a record that loads with its
    /// verification quietly disabled.
    pub prompt_digest: String,
    /// Absolute path of the harness binary as resolved at submission.
    ///
    /// Evidence, not a path that gets executed: the preview shows it, and
    /// `run_task` compares it with what it resolves itself. The workspace
    /// resolves the same harness *name* on its own `PATH` again, because cmux
    /// installs a per-surface wrapper whose path differs from the submitting
    /// shell's.
    #[serde(default)]
    pub harness_executable: PathBuf,
    pub enforcement: EnforcementReport,
    pub reliability_warning: Option<String>,
    pub cmux_group_id: Option<String>,
    pub cmux_workspace_id: Option<String>,
    pub cmux_window_id: Option<String>,
    pub state: TaskState,
}

impl TaskRecord {
    /// `chris@1.2.0` or `auto`.
    pub fn agent_label(&self) -> String {
        match &self.identity.agent_version {
            Some(version) => format!("{}@{version}", self.identity.agent),
            None => self.identity.agent.clone(),
        }
    }
}

const TASK_FILE: &str = "task.json";
const PROMPT_FILE: &str = "prompt.txt";
/// Schema 2 splits `LaunchIdentity`'s single instruction digest into
/// `source_digest` (the whole file) and `instructions_digest` (the delivered
/// text). A schema-1 record carries the whole-file digest under the *name*
/// `instructions_digest`, so reading one as schema 2 would attribute file bytes
/// to delivered bytes. `load` refuses it by version instead.
pub const TASK_SCHEMA_VERSION: u32 = 2;

/// A time-ordered, collision-resistant task identifier.
///
/// Repeated and concurrent launches of the same agent must produce distinct
/// tasks, so the identifier mixes a timestamp with process and counter entropy.
pub fn new_task_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "{:010x}{:04x}{:04x}",
        now.as_secs(),
        (now.subsec_nanos() >> 8) & 0xffff,
        (std::process::id() ^ counter) & 0xffff
    )
}

/// RFC 3339 UTC timestamp, computed without a date library.
pub fn now_rfc3339() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    format_rfc3339(secs)
}

pub fn format_rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Write a task record and its prompt.
///
/// Both files are created owner-only through the state helpers, so the record,
/// the prompt, and every directory leading to them are refused rather than
/// followed if anything on the way has been replaced by a symlink.
///
/// The prompt is written as its own file and is never placed on a command line.
/// `ahu run-task` reads it back and passes it to the harness as a single
/// argument vector element.
pub fn save(dir: &Path, record: &TaskRecord, prompt: &str) -> Result<()> {
    state::create_private_dir_all(dir)?;
    // The record names the agent, the repository, and the task title. It is not
    // as sensitive as the prompt, but it has no reason to be world-readable.
    state::write_json(&dir.join(TASK_FILE), record)?;
    state::write_private_file(&dir.join(PROMPT_FILE), prompt.as_bytes())
}

pub fn load(dir: &Path) -> Result<TaskRecord> {
    let path = dir.join(TASK_FILE);
    let bytes = state::read_private_file(&path)
        .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;

    // The schema version is read on its own, before the record is deserialized
    // into this build's struct.
    //
    // Check compatibility before deserialization: incompatible records may
    // omit required fields, but the useful diagnostic is the schema mismatch.
    let version = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|value| value.get("schema_version")?.as_u64());
    if let Some(version) = version
        && version != u64::from(TASK_SCHEMA_VERSION)
    {
        bail!(
            "{} was written by a different ahu schema version ({version}); this ahu build reads \
             {TASK_SCHEMA_VERSION}.\n\
             ahu will not reinterpret it: schema 1 recorded the whole file's digest under the \
             name instructions_digest, which now means the delivered instruction text, so the \
             same field would be read as covering bytes it does not cover.",
            path.display()
        );
    }

    let record: TaskRecord = serde_json::from_slice(&bytes).map_err(|e| {
        Error::new(format!(
            "{} is not a valid ahu task record: {e}",
            path.display()
        ))
    })?;
    // A record whose `schema_version` could not be read as a number at all --
    // absent, or not an integer -- still must not be accepted on the strength of
    // the struct happening to deserialize.
    if record.schema_version != TASK_SCHEMA_VERSION {
        bail!(
            "{} declares ahu schema version {}; this ahu build reads {TASK_SCHEMA_VERSION}.",
            path.display(),
            record.schema_version
        );
    }
    Ok(record)
}

pub fn load_prompt(dir: &Path) -> Result<String> {
    let path = dir.join(PROMPT_FILE);
    let bytes = state::read_private_file(&path)
        .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;
    String::from_utf8(bytes)
        .map_err(|_| Error::new(format!("{} is not valid UTF-8", path.display())))
}

/// Update the recorded state of a task.
pub fn set_state(dir: &Path, new_state: TaskState) -> Result<()> {
    let mut record = load(dir)?;
    record.state = new_state;
    state::write_json(&dir.join(TASK_FILE), &record)
}

/// A task directory whose record ahu could not read.
///
/// Everything here comes from the directory itself, never from the file ahu
/// refused: reading fields out of a record whose schema ahu does not understand
/// is the exact reinterpretation [`TASK_SCHEMA_VERSION`] exists to prevent. The
/// directory name is the task id ahu chose when it created the task, so it is
/// safe to use and enough to find the leftovers on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadableTask {
    pub dir: PathBuf,
    /// The directory name, which is the task id.
    pub task_id: String,
    /// Why [`load`] refused it, in full.
    pub reason: String,
}

/// Every task directory ahu found for a repository: the ones it could read, and
/// the ones it could not.
///
/// Unreadable records remain visible so inspection can report their task
/// directories and recover worktree locations without trusting their contents.
#[derive(Debug, Clone, Default)]
pub struct TaskListing {
    /// Readable records, newest first.
    pub records: Vec<(PathBuf, TaskRecord)>,
    /// Directories ahu could not read, by task id.
    pub unreadable: Vec<UnreadableTask>,
    /// Things found while listing that are not tasks and must not be presented
    /// as any task's, such as a record copied into another task's store.
    ///
    /// Deliberately not an [`UnreadableTask`]: a row carrying a task id makes
    /// that id ambiguous to an exact lookup, which is precisely what a
    /// transplanted record would exploit.
    pub notes: Vec<String>,
}

impl TaskListing {
    /// Whether a task id already has a row, readable or not.
    fn has(&self, task_id: &str) -> bool {
        self.records.iter().any(|(_, r)| r.task_id == task_id)
            || self.unreadable.iter().any(|u| u.task_id == task_id)
    }

    /// Whether this repository has no task directories at all.
    ///
    /// Not "no readable records": a caller asking this is usually about to tell
    /// the user that nothing was ever launched here, and that must only be said
    /// when ahu actually found nothing.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty() && self.unreadable.is_empty()
    }

    /// Every task directory, readable or not.
    pub fn dirs(&self) -> Vec<PathBuf> {
        self.records
            .iter()
            .map(|(dir, _)| dir.clone())
            .chain(self.unreadable.iter().map(|u| u.dir.clone()))
            .collect()
    }
}

/// Every task recorded for a repository: readable records newest first, plus
/// every directory whose record could not be read.
///
/// Two places are searched, and both are reached from any checkout of the
/// repository, so `ahu tasks` shows the same list from the primary checkout and
/// from a sibling task worktree without anything being set in the environment:
///
/// * each task worktree under `.worktrees/`, which is where a task's own record
///   lives and where it goes away when the worktree is removed, and
/// * the invoking checkout's own store, which holds records written before task
///   state moved into worktrees, and records written under an explicit
///   `AHU_STATE_DIR`.
///
/// A task found in both is reported from its worktree: that copy is the live
/// one, and the other is a leftover of the older layout.
pub fn list(repo: &crate::git::Repo) -> Result<TaskListing> {
    let identity = repo.identity();
    let primary_root = repo.primary_root()?;
    let mut listing = TaskListing::default();
    let mut incomplete = Vec::new();
    scan_worktrees(
        &state::worktrees_root_at(&primary_root),
        &identity,
        &mut listing,
        &mut incomplete,
    )?;

    for store in checkout_stores(repo, &primary_root, &identity)? {
        // Listing is a read of the store, so the path to it is confined the
        // same way a record read is: a link below the state root is refused,
        // not walked.
        state::confine_existing_dir(&store)?;
        let mut found = TaskListing::default();
        scan_tasks_dir(&store, &identity, Store::Checkout, &mut found)?;
        listing.notes.extend(found.notes);
        for (dir, record) in found.records {
            if !listing.has(&record.task_id) {
                listing.records.push((dir, record));
            }
        }
        for refused in found.unreadable {
            if !listing.has(&refused.task_id) {
                listing.unreadable.push(refused);
            }
        }
    }

    for dir in crate::headless::discover(repo)? {
        let record = load(&dir)?;
        if record.repo_identity != identity
            || record.task_id != dir.file_name().unwrap_or_default().to_string_lossy()
        {
            bail!("external task record has inconsistent repository/task identity");
        }
        if listing
            .records
            .iter()
            .any(|(_, old)| old.task_id == record.task_id)
        {
            bail!(
                "conflicting internal and external task records for {}",
                record.task_id
            );
        }
        listing
            .unreadable
            .retain(|row| row.task_id != record.task_id);
        listing.records.push((dir, record));
    }

    // Only now, when every store has been read: a worktree from the older
    // layout has no state of its own and its record has just been found in a
    // checkout store, so reporting it as recordless would be wrong.
    for row in incomplete {
        if !listing.has(&row.task_id) {
            listing.unreadable.push(row);
        }
    }

    listing
        .records
        .sort_by(|a, b| b.1.created_at.cmp(&a.1.created_at));
    listing.unreadable.sort_by(|a, b| b.task_id.cmp(&a.task_id));
    Ok(listing)
}

/// The per-checkout stores that can hold a task record for this repository.
///
/// The invoking checkout's store comes first, then the primary checkout's, so a
/// record written before task state moved into worktrees is still found when
/// `ahu tasks` runs from a sibling worktree rather than from the checkout that
/// launched it. Both are derived from the repository passed in, not from the
/// process working directory, so a caller holding a valid repository gets the
/// same answer wherever it is standing.
///
/// A managed task worktree's store is deliberately absent from this list.
/// [`scan_worktrees`] has already read it as that task's own store, under the
/// rule that a task worktree holds only its own state. Reading it again here --
/// as a checkout store, which may legitimately hold many tasks -- would take
/// back exactly the records that rule refused, and would do so only when
/// listing from inside that worktree.
///
/// An explicit `AHU_STATE_DIR` that is not simply one of this repository's own
/// checkout stores replaces these default stores, and is then the only one read
/// here. It changes nothing else: `list` scans task worktrees separately and
/// always, so a task launched by an ordinary ahu is still found by a tool that
/// sets one -- and `ahu tasks` may then update that task's recorded state
/// inside its own worktree. An override is not a promise of isolation from live
/// tasks, and nothing here should be read as one.
fn checkout_stores(
    repo: &crate::git::Repo,
    primary_root: &Path,
    identity: &str,
) -> Result<Vec<PathBuf>> {
    let tasks_under = |root: &Path| root.join("repos").join(identity).join("tasks");
    if let Some(explicit) = state::isolated_store(repo)? {
        return Ok(vec![tasks_under(&explicit)]);
    }
    let mut stores = Vec::new();
    if !is_managed_worktree(&repo.root, primary_root) {
        stores.push(tasks_under(&state::checkout_root(&repo.root)?));
    }
    let primary = tasks_under(&state::checkout_root(primary_root)?);
    if !stores.contains(&primary) {
        stores.push(primary);
    }
    Ok(stores)
}

/// Whether `root` is one of the task worktrees ahu creates.
///
/// Resolved before comparing: the same directory is reached one way through
/// Git's answer and another through a path ahu built.
fn is_managed_worktree(root: &Path, primary_root: &Path) -> bool {
    let worktrees = state::worktrees_root_at(primary_root);
    root.canonicalize()
        .ok()
        .zip(worktrees.canonicalize().ok())
        .is_some_and(|(root, worktrees)| root.parent() == Some(worktrees.as_path()))
}

/// Which store a scan is reading, and therefore what it may contain.
///
/// A task worktree holds exactly one task's state: its own. A checkout store
/// holds whatever was launched from that checkout, so it may hold many.
#[derive(Debug, Clone, Copy)]
enum Store<'a> {
    Worktree { path: &'a Path, task_id: &'a str },
    Checkout,
}

/// Read every task directory directly inside `dir`, if it is there at all.
///
/// A directory that does not exist is not an error: a repository with no tasks
/// in a given store simply has none.
///
/// A record is only accepted as the task the directory names. Copying a valid
/// record for task B into task A's store would otherwise put B in the listing
/// twice -- once as itself and once as something found in A -- and make an
/// exact lookup of B ambiguous, so a record that does not match where it was
/// found is carried as unreadable instead.
fn scan_tasks_dir(
    dir: &Path,
    repo_identity: &str,
    store: Store<'_>,
    listing: &mut TaskListing,
) -> Result<usize> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => bail!("cannot read {}: {e}", dir.display()),
    };
    // Counts only what accounts for the task this store belongs to. A stray
    // that is rejected below must not stand in for the owner's own record, or a
    // worktree with nothing of its own would stop being reported as such.
    let mut accounted = 0;
    for entry in entries {
        let path = entry?.path();
        let task_id = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        // In a worktree store, only the owner's own task id can be a task.
        // Anything else gets a note rather than a row, because a row carries an
        // id and would make that id ambiguous to an exact lookup.
        if let Store::Worktree {
            path: owner,
            task_id: owner_id,
        } = store
            && task_id != owner_id
        {
            listing.notes.push(format!(
                "{} holds {task_id}, which is not its own task state. It is not listed as a \
                 task. Nothing was changed or removed.",
                owner.display()
            ));
            continue;
        }
        match std::fs::symlink_metadata(&path) {
            // A task directory ahu wrote is a real directory.
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            // A link here would read a record from wherever it points. It is
            // not followed -- and not passed over in silence either, because a
            // listing that skipped it would report fewer tasks than the store
            // has entries for.
            Ok(meta) if meta.file_type().is_symlink() => {
                accounted += 1;
                listing.unreadable.push(UnreadableTask {
                    dir: path,
                    task_id,
                    reason: "this task directory is a symlink; ahu will not read a record \
                             through one."
                        .to_string(),
                });
                continue;
            }
            _ => continue,
        }
        match load(&path) {
            Ok(record) => match misplaced(&record, &task_id, repo_identity, store) {
                None => {
                    accounted += 1;
                    listing.records.push((path, record));
                }
                // A record that does not belong where it was found is named,
                // not listed, and does not account for the task whose place it
                // is occupying.
                Some(reason) => match store {
                    Store::Worktree { path: owner, .. } => listing.notes.push(format!(
                        "{} holds a record that does not belong to it: {reason} It is not \
                         listed as a task. Nothing was changed or removed.",
                        owner.display()
                    )),
                    Store::Checkout => listing.unreadable.push(UnreadableTask {
                        dir: path,
                        task_id,
                        reason,
                    }),
                },
            },
            // Carried, not dropped. It must still not break the listing of the
            // others. Keep the refused directory visible for inspection.
            Err(e) => {
                accounted += 1;
                listing.unreadable.push(UnreadableTask {
                    dir: path,
                    task_id,
                    reason: e.to_string(),
                });
            }
        }
    }
    Ok(accounted)
}

/// Why a record does not belong where it was found, if it does not.
///
/// Nothing here trusts the record to describe itself correctly; each field is
/// compared against something ahu knows independently -- the directory name it
/// chose, the repository it is looking in, and the worktree it is standing in.
fn misplaced(
    record: &TaskRecord,
    task_id: &str,
    repo_identity: &str,
    store: Store<'_>,
) -> Option<String> {
    if record.task_id != task_id {
        return Some(format!(
            "this directory is named for task {task_id}, but the record in it is for task {}. \
             ahu will not present a record as a task it does not name.",
            record.task_id
        ));
    }
    if record.repo_identity != repo_identity {
        return Some(
            "this record was written for a different repository than the one being listed."
                .to_string(),
        );
    }
    let Store::Worktree { path, .. } = store else {
        return None;
    };
    // Compared resolved, because the worktree path is reached one way here and
    // recorded another way at submission.
    let same = path
        .canonicalize()
        .ok()
        .zip(record.worktree.canonicalize().ok())
        .is_some_and(|(found, recorded)| found == recorded);
    if !same {
        return Some(format!(
            "this record names the worktree {}, but it was found in {}.",
            record.worktree.display(),
            path.display()
        ));
    }
    None
}

/// Look in every task worktree of the repository for the record it owns.
///
/// The worktree directory name is the task id ahu chose, so a worktree that is
/// gone takes its task out of the listing by simply not being there. A worktree
/// whose state directory ahu refuses stays *in* the listing as unreadable:
/// hiding it would claim work does not exist when its checkout and branch do.
fn scan_worktrees(
    root: &Path,
    repo_identity: &str,
    listing: &mut TaskListing,
    incomplete: &mut Vec<UnreadableTask>,
) -> Result<()> {
    match std::fs::symlink_metadata(root) {
        Ok(meta) if meta.file_type().is_symlink() => bail!(
            "refusing to enumerate task worktrees through {}: it is a symlink.",
            root.display()
        ),
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => bail!("cannot read {}: {e}", root.display()),
    }
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => bail!("cannot read {}: {e}", root.display()),
    };
    for entry in entries {
        let worktree = entry?.path();
        let Some(task_id) = worktree
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
        else {
            continue;
        };
        // `.gitignore` is the file that makes `.worktrees/` ignore itself.
        if task_id.starts_with('.') {
            continue;
        }
        match std::fs::symlink_metadata(&worktree) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(meta) if meta.file_type().is_symlink() => {
                listing.unreadable.push(UnreadableTask {
                    dir: worktree,
                    task_id,
                    reason: "this task worktree is a symlink; ahu will not follow one out of \
                             the repository."
                        .to_string(),
                });
                continue;
            }
            _ => continue,
        }
        let state_root = match state::checkout_root(&worktree) {
            Ok(root) => root,
            Err(e) => {
                listing.unreadable.push(UnreadableTask {
                    dir: worktree.join(".ahu").join("state"),
                    task_id,
                    reason: e.to_string(),
                });
                continue;
            }
        };
        let tasks = state_root.join("repos").join(repo_identity).join("tasks");
        if let Err(e) = state::confine_existing_dir(&tasks) {
            listing.unreadable.push(UnreadableTask {
                dir: tasks,
                task_id,
                reason: e.to_string(),
            });
            continue;
        }
        let found = scan_tasks_dir(
            &tasks,
            repo_identity,
            Store::Worktree {
                path: &worktree,
                task_id: &task_id,
            },
            listing,
        )?;
        if found == 0 {
            // A checkout ahu created with nothing recorded in it. Leaving it out
            // would make `ahu tasks` claim the repository has no such task while
            // its checkout and branch are sitting there -- which happens exactly
            // when a launch failed after creating the worktree and Git then
            // refused to remove it because it already held work.
            //
            // Held back rather than added here: a task from the older layout
            // legitimately has a worktree with no state in it and its record in
            // a checkout store. These rows are only used for the worktrees that
            // still have nothing after every store has been read.
            incomplete.push(UnreadableTask {
                dir: tasks,
                task_id,
                reason: format!(
                    "this task worktree has no record anywhere ahu looked. A launch may still \
                     be preparing it, or one failed after creating the checkout {} and could \
                     not remove it. The checkout and its branch are still here.",
                    worktree.display()
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod state_tests {
    use super::*;

    #[test]
    fn cancelled_state_strings_and_serde() {
        assert_eq!(TaskState::Cancelled.as_str(), "cancelled");
        let value = serde_json::to_value(TaskState::Cancelled).unwrap();
        assert_eq!(value, serde_json::json!("cancelled"));
        let round: TaskState = serde_json::from_value(value).unwrap();
        assert_eq!(round, TaskState::Cancelled);
    }

    #[test]
    fn live_states_are_starting_and_running_only() {
        assert!(TaskState::Starting.is_live());
        assert!(TaskState::Running.is_live());
        assert!(!TaskState::Exited.is_live());
        assert!(!TaskState::Failed.is_live());
        assert!(!TaskState::Cancelled.is_live());
    }
}

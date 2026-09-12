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
/// was completed successfully.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TaskState {
    Starting,
    Running,
    Exited,
    Failed,
}

impl TaskState {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Starting => "starting",
            TaskState::Running => "running",
            TaskState::Exited => "exited",
            TaskState::Failed => "failed",
        }
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
    /// Schema 1 had this field holding the *whole-file* digest, so its name and
    /// its value disagreed once ahu began delivering the stripped body. That is
    /// why [`TASK_SCHEMA_VERSION`] is 2: an old record's `instructions_digest`
    /// cannot be reinterpreted as this one, and `load` refuses it by version
    /// rather than silently reading the wrong bytes under the right name.
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
    /// Recorded so the exec does not consult `PATH` a second time, and so the
    /// preview can show exactly which binary will run.
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

/// Create a directory that only its owner can read.
fn create_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn set_owner_only(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Write a task record and its prompt.
///
/// The prompt is written as its own file with owner-only permissions and is
/// never placed on a command line. `ahu run-task` reads it back and passes it to
/// the harness as a single argument vector element.
pub fn save(dir: &Path, record: &TaskRecord, prompt: &str) -> Result<()> {
    create_private_dir(dir)?;
    state::write_json(&dir.join(TASK_FILE), record)?;
    // The record names the agent, the repository, and the task title. It is not
    // as sensitive as the prompt, but it has no reason to be world-readable.
    set_owner_only(&dir.join(TASK_FILE))?;
    let prompt_path = dir.join(PROMPT_FILE);
    std::fs::write(&prompt_path, prompt.as_bytes())
        .map_err(|e| Error::new(format!("cannot write {}: {e}", prompt_path.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&prompt_path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn load(dir: &Path) -> Result<TaskRecord> {
    let path = dir.join(TASK_FILE);
    let bytes = std::fs::read(&path)
        .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;

    // The schema version is read on its own, before the record is deserialized
    // into this build's struct.
    //
    // Checking it afterwards made the careful message below unreachable for the
    // case it was written for: schema 1 has no `delivery` field, so serde failed
    // on `missing field \`delivery\`` and that is what the user saw. A version
    // mismatch is the *reason* the fields do not line up, and reporting a
    // symptom of it instead tells someone with five old tasks to go looking for
    // a corrupt file.
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
    let bytes = std::fs::read(&path)
        .map_err(|e| Error::new(format!("cannot read {}: {e}", path.display())))?;
    String::from_utf8(bytes)
        .map_err(|_| Error::new(format!("{} is not valid UTF-8", path.display())))
}

/// Update the recorded state of a task.
pub fn set_state(dir: &Path, new_state: TaskState) -> Result<()> {
    let mut record = load(dir)?;
    record.state = new_state;
    state::write_json(&dir.join(TASK_FILE), &record)?;
    set_owner_only(&dir.join(TASK_FILE))
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
/// Both halves, deliberately. `list` used to return only the readable records
/// and drop the rest, despite claiming to report them.
/// Unreadable records were initially the exception.
///
/// The schema-2 bump made them the rule: every record written by an earlier ahu
/// is refused, so a repository with five tasks, five worktrees, five branches
/// and three live cmux sessions had `ahu tasks` print "No ahu tasks have been
/// launched from this repository." An informational nit became a positive claim
/// of absence that was false, and the only surface that could have told the user
/// where their leftover worktrees were is the one that denied they existed.
#[derive(Debug, Clone, Default)]
pub struct TaskListing {
    /// Readable records, newest first.
    pub records: Vec<(PathBuf, TaskRecord)>,
    /// Directories ahu could not read, by task id.
    pub unreadable: Vec<UnreadableTask>,
}

impl TaskListing {
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
pub fn list(repo_identity: &str) -> Result<TaskListing> {
    let dir = state::tasks_dir(repo_identity)?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(TaskListing::default()),
        Err(e) => bail!("cannot read {}: {e}", dir.display()),
    };
    let mut listing = TaskListing::default();
    for entry in entries {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        match load(&path) {
            Ok(record) => listing.records.push((path, record)),
            // Carried, not dropped. It must still not break the listing of the
            // others, which is what the `continue` was for; the mistake was
            // throwing the evidence away on the way past.
            Err(e) => {
                let task_id = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string_lossy().to_string());
                listing.unreadable.push(UnreadableTask {
                    dir: path,
                    task_id,
                    reason: e.to_string(),
                });
            }
        }
    }
    listing
        .records
        .sort_by(|a, b| b.1.created_at.cmp(&a.1.created_at));
    listing.unreadable.sort_by(|a, b| b.task_id.cmp(&a.task_id));
    Ok(listing)
}

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
    pub harness: String,
    pub model: String,
    /// Where the system prompt came from, repository-relative.
    pub instructions_source: Option<String>,
    pub instructions_digest: Option<String>,
    /// Digest binding manifest fields and native definition together.
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
    pub launch_command: LaunchCommand,
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
pub const TASK_SCHEMA_VERSION: u32 = 1;

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
/// The prompt is written as its own file with owner-only permissions and is
/// never placed on a command line. `ahu run-task` reads it back and passes it to
/// the harness as a single argument vector element.
pub fn save(dir: &Path, record: &TaskRecord, prompt: &str) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    state::write_json(&dir.join(TASK_FILE), record)?;
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
    let record: TaskRecord = serde_json::from_slice(&bytes).map_err(|e| {
        Error::new(format!(
            "{} is not a valid ahu task record: {e}",
            path.display()
        ))
    })?;
    if record.schema_version != TASK_SCHEMA_VERSION {
        bail!(
            "{} was written by a different ahu schema version ({}).",
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
    state::write_json(&dir.join(TASK_FILE), &record)
}

/// Every task recorded for a repository, newest first.
pub fn list(repo_identity: &str) -> Result<Vec<(PathBuf, TaskRecord)>> {
    let dir = state::tasks_dir(repo_identity)?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => bail!("cannot read {}: {e}", dir.display()),
    };
    let mut found = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        match load(&path) {
            Ok(record) => found.push((path, record)),
            // A record ahu cannot read is reported by `ahu tasks`, not silently
            // dropped, but it must not break the listing of the others.
            Err(_) => continue,
        }
    }
    found.sort_by(|a, b| b.1.created_at.cmp(&a.1.created_at));
    Ok(found)
}

//! Version 1. Incompatible changes require a new module and storage version.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const VERSION: u32 = 1;
pub const MAX_INPUT_BYTES: usize = 64 * 1024;
pub const MAX_OPERATIONS: usize = 256;
pub const REPLAY_WINDOW: usize = 128;
pub const MAX_PAGE_SIZE: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Principal {
    Operator(String),
    Worker(String),
    Plugin(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    List,
    Read,
    Replay,
    Submit,
    Cancel,
    Input,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub version: u32,
    pub operations: BTreeSet<Operation>,
    pub max_input_bytes: usize,
    pub replay_window: usize,
}

/// Frozen launch identity; provider is an execution adapter identity, not a
/// terminal host. No paths, native auth, raw events, or execution argv cross V1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub repository: String,
    pub task: String,
    pub provider: String,
    pub agent: crate::task::LaunchIdentity,
    pub policy_digest: String,
    pub config_digest: String,
    pub prompt_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Prepared,
    Queued,
    /// A durable claim exists. Startup may or may not have happened.
    Starting,
    Running,
    WaitingForApproval,
    /// Provider acknowledgement was lost, or core lost its execution owner.
    OutcomeUnknown,
    Exited,
    Failed,
    Cancelled,
}
impl Lifecycle {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Exited | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub version: u32,
    pub identity: Identity,
    pub owner: Principal,
    pub revision: u64,
    pub lifecycle: Lifecycle,
    /// Intent only, never proof of provider termination.
    pub cancellation_requested: bool,
    pub supports_input: bool,
}

/// Snapshots in ascending task-ID order, restricted to the session's exact
/// grants. Each snapshot is coherent; a page is not a cross-task transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskPage {
    pub tasks: Vec<Snapshot>,
    /// Exclusive task-ID cursor, present only when more granted IDs remain.
    /// Reauthorize scope on reconnect; this cursor never confers authority.
    pub next_after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "text", rename_all = "snake_case")]
pub enum Action {
    Submit,
    Cancel,
    Input(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mutation {
    pub version: u32,
    pub task: String,
    /// Unique within this task and principal. Never reused for different bytes.
    pub operation_id: String,
    pub expected_revision: u64,
    pub action: Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Admitted,
    Submitted,
    CancellationRequested,
    InputAccepted,
    EffectClaimed,
    EffectObserved,
    Reconciled,
}

/// Bounded coordination metadata, not a provider execution trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub revision: u64,
    pub kind: ChangeKind,
    pub lifecycle: Lifecycle,
    pub cancellation_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Replay {
    pub snapshot: Snapshot,
    pub changes: Vec<Change>,
}

/// Claimed means an effect may be in flight or its owner may have died. Neither
/// Claimed nor Unknown permits automatic retry, even after reconnection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Pending,
    Claimed,
    Confirmed,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputStatus {
    pub operation_id: String,
    pub delivery: DeliveryState,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effects {
    pub revision: u64,
    pub start: DeliveryState,
    pub cancellation: Option<DeliveryState>,
    pub inputs: Vec<InputStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    UnsupportedVersion,
    Denied,
    NotFound,
    InvalidRequest,
    Conflict,
    StaleRevision {
        current: u64,
    },
    InvalidLifecycle,
    UnsupportedOperation,
    ReplayExpired {
        oldest: u64,
    },
    Capacity,
    Busy,
    /// A claim exists: it is unsafe to automatically invoke the effect again.
    OutcomeUnknown,
    Storage(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

/// Caller identity/grants are bound by core when creating the session, never
/// supplied in a request. IDs locate tasks; they do not confer ownership.
pub trait TaskService: Send + Sync {
    fn capabilities(&self, version: u32) -> Result<Capabilities>;
    /// Enumerate only core-granted task IDs, with an explicit List grant.
    /// Limit must be 1..=MAX_PAGE_SIZE; after must be an exact granted ID.
    /// Missing or unreadable records fail the page, never a partial success.
    /// Implementations predating listing may omit it and must not advertise List.
    fn list(&self, version: u32, after: Option<&str>, limit: usize) -> Result<TaskPage> {
        let _ = (after, limit);
        if version != VERSION {
            return Err(Error::UnsupportedVersion);
        }
        Err(Error::UnsupportedOperation)
    }
    fn get(&self, version: u32, task: &str) -> Result<Snapshot>;
    fn effects(&self, version: u32, task: &str) -> Result<Effects>;
    fn replay(&self, version: u32, task: &str, after: u64) -> Result<Replay>;
    /// Retry the exact request after losing an acknowledgement. The original
    /// receipt wins over a now-stale expected revision; authorization is fresh.
    fn mutate(&self, request: Mutation) -> Result<Snapshot>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartOutcome {
    Running,
    NotStarted,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectOutcome {
    Confirmed,
    Unknown,
}

/// Called only by core after persisting a one-shot claim. Implementations must
/// preserve the frozen identity and existing native approval/admission checks.
/// Panics, lost replies and process death leave a claim, never permission to retry.
/// An adapter gets no storage handle and cannot admit work through this trait.
pub trait ExecutionAdapter {
    fn provider_id(&self) -> &str;
    fn start(&mut self, identity: &Identity) -> StartOutcome;
    fn cancel(&mut self, identity: &Identity) -> EffectOutcome;
    fn input(&mut self, identity: &Identity, operation_id: &str, text: &str) -> EffectOutcome;
}

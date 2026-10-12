//! Read-only TaskService observations carried as input to generic protocol V1.
//! Core owns grants and collection; the executable receives data, not a service.
use super::{Code, Failure, HostGrant, MAX_BYTES, Result};
use crate::agent_task::{Controller, v1};
use crate::git::Repo;
use ahu_host_api::protocol::from_slice;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    io::Write,
    sync::atomic::{AtomicBool, Ordering},
};

pub const VERSION: u32 = 1;
pub const MAX_TASKS: usize = 128;
pub const MAX_OBSERVATIONS: usize = 128;

/// Trusted core factory only. Never construct from plugin-supplied identity or
/// scope. No serialization, controller access, storage location or mutation API.
pub struct TaskReadGrant {
    host: HostGrant,
    service: Box<dyn v1::TaskService>,
}
impl TaskReadGrant {
    pub fn new(host: HostGrant, core: &Controller, exact_tasks: BTreeSet<String>) -> Result<Self> {
        if core.repository_identity() != host.repository {
            return Err(Failure::before(Code::Denied));
        }
        if exact_tasks.len() > MAX_TASKS
            || exact_tasks
                .iter()
                .any(|id| !crate::task::is_canonical_task_uuid(id))
        {
            return Err(Failure::before(Code::InvalidInput));
        }
        let service = core
            .plugin_read_session(&host.plugin, exact_tasks)
            .map_err(service_error)?;
        Ok(Self {
            host,
            service: Box::new(service),
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    observations: Vec<Observation>,
}
#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Observation {
    List { after: Option<String>, limit: usize },
    Read { task: String },
    Replay { task: String, after: u64 },
}

// Explicit wire projection: never serialize the service's full identity/owner,
// LaunchIdentity (which includes a source path), effects or task records.
#[derive(Serialize)]
struct Snapshot {
    version: u32,
    task: String,
    revision: u64,
    lifecycle: v1::Lifecycle,
    cancellation_requested: bool,
}
impl From<v1::Snapshot> for Snapshot {
    fn from(value: v1::Snapshot) -> Self {
        Self {
            version: VERSION,
            task: value.identity.task,
            revision: value.revision,
            lifecycle: value.lifecycle,
            cancellation_requested: value.cancellation_requested,
        }
    }
}
#[derive(Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
enum Observed {
    List {
        tasks: Vec<Snapshot>,
        next_after: Option<String>,
    },
    Read {
        snapshot: Snapshot,
    },
    Replay {
        snapshot: Snapshot,
        changes: Vec<v1::Change>,
    },
}
#[derive(Serialize)]
struct Payload {
    kind: &'static str,
    version: u32,
    capabilities: v1::Capabilities,
    observations: Vec<Observed>,
}

/// Invoke an enabled executable with core-collected observations of exact grants.
/// `request_json` selects only list/read/replay; it cannot select scope or identity.
/// Capabilities are always included. Collection failure prevents plugin startup.
/// Neither collection nor returned JSON dispatches any task execution or mutation.
pub fn invoke(
    repo: &Repo,
    grant: &TaskReadGrant,
    request_json: &[u8],
    cancelled: &AtomicBool,
) -> Result<Value> {
    let check_cancel = || {
        if cancelled.load(Ordering::Relaxed) {
            Err(Failure::before(Code::Cancelled))
        } else {
            Ok(())
        }
    };
    check_cancel()?;
    // Gate service access on the same current committed authority as execution.
    super::authorized_binding(repo, &grant.host)?;
    let request = parse_request(request_json)?;
    let service = &grant.service;
    let mut payload = Payload {
        kind: "ahu.task-observations",
        version: VERSION,
        capabilities: service.capabilities(v1::VERSION).map_err(service_error)?,
        observations: Vec::new(),
    };
    for observation in request.observations {
        check_cancel()?;
        let observed = match observation {
            Observation::List { after, limit } => {
                let page = service
                    .list(v1::VERSION, after.as_deref(), limit)
                    .map_err(service_error)?;
                Observed::List {
                    tasks: page.tasks.into_iter().map(Into::into).collect(),
                    next_after: page.next_after,
                }
            }
            Observation::Read { task } => Observed::Read {
                snapshot: service
                    .get(v1::VERSION, &task)
                    .map_err(service_error)?
                    .into(),
            },
            Observation::Replay { task, after } => {
                let replay = service
                    .replay(v1::VERSION, &task, after)
                    .map_err(service_error)?;
                Observed::Replay {
                    snapshot: replay.snapshot.into(),
                    changes: replay.changes,
                }
            }
        };
        payload.observations.push(observed);
        // Fail as soon as collection exceeds the byte budget; never truncate or
        // collect all remaining observations after overflow.
        bounded_json(&payload)?;
    }
    let input = bounded_json(&payload)?;
    // The unchanged generic path bounds the entire envelope (not just input),
    // rechecks committed authority, verifies compatibility/package and spawns.
    super::invoke(repo, &grant.host, &input, cancelled)
}

fn service_error(error: v1::Error) -> Failure {
    Failure::before(match error {
        v1::Error::Denied => Code::Denied,
        v1::Error::UnsupportedVersion
        | v1::Error::InvalidRequest
        | v1::Error::UnsupportedOperation
        | v1::Error::StaleRevision { .. }
        | v1::Error::ReplayExpired { .. } => Code::InvalidInput,
        _ => Code::Unavailable,
    })
}

fn bounded_json(value: &impl Serialize) -> Result<Vec<u8>> {
    struct Bounded(Vec<u8>);
    impl Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_BYTES - self.0.len() {
                return Err(std::io::Error::other("observation limit"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Bounded(Vec::new());
    serde_json::to_writer(&mut output, value).map_err(|_| Failure::before(Code::InvalidInput))?;
    Ok(output.0)
}

fn parse_request(bytes: &[u8]) -> Result<Request> {
    if bytes.len() > MAX_BYTES {
        return Err(Failure::before(Code::InvalidInput));
    }
    let request: Request = from_slice(bytes).map_err(|_| Failure::before(Code::InvalidInput))?;
    if request.version != VERSION || request.observations.len() > MAX_OBSERVATIONS {
        return Err(Failure::before(Code::InvalidInput));
    }
    Ok(request)
}

/// Operator-only CLI bridge. Authority is separate from the observation JSON;
/// only immutable, exact indexed observations are admitted, never a local store.
pub(crate) fn invoke_inspected(
    repo: &Repo,
    host: HostGrant,
    tasks: BTreeSet<String>,
    bytes: &[u8],
    cancelled: &AtomicBool,
) -> Result<Value> {
    super::authorized_binding(repo, &host)?;
    if tasks.len() > MAX_TASKS
        || tasks
            .iter()
            .any(|id| !crate::task::is_canonical_task_uuid(id))
    {
        return Err(Failure::before(Code::InvalidInput));
    }
    let request = parse_request(bytes)?;
    let mut demanded = BTreeSet::new();
    // Preflight every operation and cursor before any task lookup.
    for observation in &request.observations {
        match observation {
            Observation::List { after, limit } => {
                demanded.extend(
                    crate::agent_task::read_page_ids(&tasks, after.as_deref(), *limit)
                        .map_err(service_error)?
                        .0,
                );
            }
            Observation::Read { task } | Observation::Replay { task, .. } => {
                if !tasks.contains(task) {
                    return Err(Failure::before(Code::Denied));
                }
                demanded.insert(task.clone());
            }
        }
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(Failure::before(Code::Cancelled));
    }
    let inspected =
        crate::agent_task::inspection::Inspection::open_exact(repo, &demanded, cancelled);
    if cancelled.load(Ordering::Relaxed) {
        return Err(Failure::before(Code::Cancelled));
    }
    let inspected = inspected.map_err(|_| Failure::before(Code::Unavailable))?;
    if inspected.repository_identity() != host.repository {
        return Err(Failure::before(Code::Denied));
    }
    let service = inspected
        .plugin_read_session(&host.plugin, tasks)
        .map_err(service_error)?;
    let grant = TaskReadGrant {
        host,
        service: Box::new(service),
    };
    invoke(repo, &grant, bytes, cancelled)
}

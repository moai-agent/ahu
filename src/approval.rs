//! Durable, task-scoped human approval checkpoints.
//!
//! Approval requests are explicit MCP calls. They do not intercept arbitrary
//! harness shell commands; they let an agent pause before a named operation
//! and require an operator decision tied to the current task.

use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::util::{Error, Result};

const REQUEST_FILE: &str = "approval-request.json";
const RESPONSE_FILE: &str = "approval-response.json";
const MAX_TEXT: usize = 2048;
const MAX_WAIT: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    schema_version: u32,
    pub request_id: String,
    pub task_id: String,
    pub operation: String,
    pub summary: String,
    pub target: Option<String>,
    pub requested_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    schema_version: u32,
    request_id: String,
    decision: String,
    decided_at: String,
}

/// Frozen when a durable MCP job is submitted, never inferred from its worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Context {
    task_id: String,
    task_dir: std::path::PathBuf,
    attempt: Option<u32>,
}

impl Context {
    pub(crate) fn current() -> Result<Self> {
        let task_id = std::env::var("AHU_TASK_ID")
            .map_err(|_| Error::new("approval requests require an Ahu-managed task context"))?;
        let task_dir = std::env::var_os("AHU_TASK_DIR")
            .map(std::path::PathBuf::from)
            .ok_or_else(|| Error::new("approval requests require an Ahu-managed task context"))?;
        let attempt = current_attempt(&task_dir)?;
        Ok(Self {
            task_id,
            task_dir,
            attempt,
        })
    }
}

fn current_attempt(dir: &Path) -> Result<Option<u32>> {
    let path = dir.join("headless.json");
    if crate::state::confine_file(&path)?.is_none() {
        return Ok(None);
    }
    let spec: crate::headless::Spec =
        serde_json::from_slice(&crate::state::read_private_file(&path)?)?;
    Ok(Some(spec.attempt))
}

struct RequestInput<'a> {
    operation: &'a str,
    summary: &'a str,
    target: Option<&'a str>,
}

fn valid_text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= MAX_TEXT && !value.chars().any(char::is_control)
}

fn paths(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    (dir.join(REQUEST_FILE), dir.join(RESPONSE_FILE))
}

pub fn request(
    repo: &crate::git::Repo,
    operation: &str,
    summary: &str,
    target: Option<&str>,
    cancellation: Option<&Path>,
) -> Result<serde_json::Value> {
    request_with_context(repo, operation, summary, target, cancellation, None)
}

pub(crate) fn request_with_context(
    repo: &crate::git::Repo,
    operation: &str,
    summary: &str,
    target: Option<&str>,
    cancellation: Option<&Path>,
    context: Option<&Context>,
) -> Result<serde_json::Value> {
    if !matches!(
        operation,
        "external-write" | "network" | "destructive" | "other"
    ) || !valid_text(summary)
        || target.is_some_and(|value| !valid_text(value))
    {
        return Err(Error::new("invalid approval request"));
    }
    let captured;
    let context = match context {
        Some(context) => context,
        None => {
            captured = Context::current()?;
            &captured
        }
    };
    request_for_task(
        repo,
        context,
        RequestInput {
            operation,
            summary,
            target,
        },
        cancellation,
        MAX_WAIT,
    )
}

fn request_for_task(
    repo: &crate::git::Repo,
    context: &Context,
    input: RequestInput<'_>,
    cancellation: Option<&Path>,
    max_wait: Duration,
) -> Result<serde_json::Value> {
    let task_id = context.task_id.as_str();
    let task_dir = context.task_dir.as_path();
    crate::state::confine_file(&task_dir.join("task.json"))?;
    // Keep the execution lease until this waiter consumes its response. A new
    // request must never steal an approved response from an existing waiter.
    let _lease = crate::task::StateLock::try_acquire(&task_dir.join("approval.worker"))?
        .ok_or_else(|| Error::new("another approval request is already pending"))?;
    let state_lock = crate::task::lock_state(task_dir)?;
    let record = crate::task::load(task_dir)?;
    let attempt = context.attempt;
    if current_attempt(task_dir)? != attempt {
        return Err(Error::new("approval belongs to a different task attempt"));
    }
    if record.task_id != task_id
        || record.repo_identity != repo.identity()
        || record.worktree.canonicalize()? != repo.root.canonicalize()?
        || !matches!(record.state, crate::task::TaskState::Running)
        || crate::state::confine_file(&task_dir.join("cancel.json"))?.is_some()
    {
        return Err(Error::new(
            "approval request does not belong to the current live task",
        ));
    }
    let (request_path, response_path) = paths(task_dir);
    // Orphaned checkpoints remain unresolved; never silently replay an operation
    // after a server crash or recycle a prior attempt's approval.
    if crate::state::confine_file(&response_path)?.is_some()
        || crate::state::confine_file(&request_path)?.is_some()
    {
        return Err(Error::new("another approval request is already pending"));
    }
    let request = Request {
        schema_version: 1,
        request_id: crate::orchestration::new_nonce()?,
        task_id: task_id.to_string(),
        operation: input.operation.into(),
        summary: input.summary.into(),
        target: input.target.map(str::to_owned),
        requested_at: crate::task::now_rfc3339(),
    };
    crate::state::write_json(&request_path, &request)?;
    if let Err(error) =
        crate::task::set_state_locked(task_dir, crate::task::TaskState::WaitingForApproval)
    {
        let _ = std::fs::remove_file(&request_path);
        return Err(error);
    }
    drop(state_lock);
    notify_cmux(repo, &record, "Waiting for approval");
    let deadline = Instant::now() + max_wait;
    let outcome = (|| {
        loop {
            let state_lock = crate::task::lock_state(task_dir)?;
            if current_attempt(task_dir)? != attempt
                || !crate::task::load(task_dir)?.state.is_live()
            {
                return Err(Error::new("task stopped while approval was pending"));
            }
            if crate::state::confine_file(&task_dir.join("cancel.json"))?.is_some() {
                return Err(Error::new("task was cancelled while approval was pending"));
            }
            if cancellation.is_some_and(|path| path.exists()) {
                crate::task::set_state_locked(task_dir, crate::task::TaskState::Running)?;
                drop(state_lock);
                notify_cmux(repo, &record, "Running");
                return Err(Error::new(
                    "approval request was cancelled by the MCP client",
                ));
            }
            if crate::state::confine_file(&response_path)?.is_some() {
                let response: Response = crate::state::read_json(&response_path)?;
                if response.schema_version != 1
                    || response.request_id != request.request_id
                    || !matches!(response.decision.as_str(), "approve" | "reject")
                {
                    return Err(Error::new("approval response does not match this request"));
                }
                if response.decision == "approve" {
                    crate::task::set_state_locked(task_dir, crate::task::TaskState::Running)?;
                    drop(state_lock);
                    notify_cmux(repo, &record, "Running");
                    return Ok(json!({"approved":true,"request_id":request.request_id}));
                }
                return Err(Error::new("operator rejected the requested operation"));
            }
            if Instant::now() >= deadline {
                decide_locked(repo, task_dir, &request.task_id, false)?;
                drop(state_lock);
                notify_cmux(repo, &record, "Approval rejected");
                return Err(Error::new("approval request expired after 30 minutes"));
            }
            drop(state_lock);
            std::thread::sleep(Duration::from_millis(100));
        }
    })();
    // Retire this checkpoint on every completed waiter path, including task
    // cancellation and terminal/attempt changes. Keep the execution lease until
    // retirement finishes, and never change task state during cleanup.
    let _state = crate::task::lock_state(task_dir)?;
    retire_checkpoint(task_dir, &request)?;
    outcome
}

/// Caller holds the state lock and the approval execution lease. Validate all
/// surviving files before deleting either; another checkpoint is never ours.
fn retire_checkpoint(dir: &Path, expected: &Request) -> Result<()> {
    let (request_path, response_path) = paths(dir);
    let request_exists = crate::state::confine_file(&request_path)?.is_some();
    if request_exists {
        let current = read_request(&request_path)?;
        if current.task_id != expected.task_id || current.request_id != expected.request_id {
            return Err(Error::new("cannot retire a different approval checkpoint"));
        }
    }
    let response_exists = crate::state::confine_file(&response_path)?.is_some();
    if response_exists {
        let response: Response = crate::state::read_json(&response_path)?;
        if response.schema_version != 1
            || response.request_id != expected.request_id
            || !matches!(response.decision.as_str(), "approve" | "reject")
        {
            return Err(Error::new("cannot retire a mismatched approval response"));
        }
        // Removing the response first leaves an attributable request if the
        // process exits between removals; explicit resume can retire it later.
        std::fs::remove_file(&response_path)?;
    }
    if request_exists {
        std::fs::remove_file(&request_path)?;
    }
    Ok(())
}

/// Explicit resume calls this after validating a terminal result and acquiring
/// supervisor ownership. Retire abandoned checkpoints without replaying their
/// decisions, and hold the returned lease until the next attempt is prepared.
pub(crate) fn retire_for_resume(
    dir: &Path,
    task_id: &str,
    attempt: u32,
) -> Result<crate::task::StateLock> {
    let lease =
        crate::task::StateLock::try_acquire(&dir.join("approval.worker"))?.ok_or_else(|| {
            Error::new("approval worker is still active; retry resume after it stops")
        })?;
    let _state = crate::task::lock_state(dir)?;
    let record = crate::task::load(dir)?;
    if record.task_id != task_id || record.state.is_live() || current_attempt(dir)? != Some(attempt)
    {
        return Err(Error::new(
            "approval cleanup requires the same terminal task attempt",
        ));
    }
    if let Some(request) = pending(dir, task_id)? {
        retire_checkpoint(dir, &request)?;
    } else if crate::state::confine_file(&dir.join(RESPONSE_FILE))?.is_some() {
        return Err(Error::new("approval response has no attributable request"));
    }
    Ok(lease)
}

fn read_request(path: &Path) -> Result<Request> {
    let request: Request = crate::state::read_json(path)?;
    if request.schema_version != 1
        || request.request_id.len() != 16
        || !request
            .request_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || !matches!(
            request.operation.as_str(),
            "external-write" | "network" | "destructive" | "other"
        )
        || !valid_text(&request.summary)
        || request
            .target
            .as_deref()
            .is_some_and(|target| !valid_text(target))
    {
        return Err(Error::new("pending approval request is malformed"));
    }
    Ok(request)
}

pub fn pending(dir: &Path, expected_task_id: &str) -> Result<Option<Request>> {
    let (request_path, _) = paths(dir);
    if crate::state::confine_file(&request_path)?.is_none() {
        return Ok(None);
    }
    let request = read_request(&request_path)?;
    if request.task_id != expected_task_id {
        return Err(Error::new("pending approval belongs to another task"));
    }
    Ok(Some(request))
}

pub fn decide(
    repo: &crate::git::Repo,
    dir: &Path,
    expected_task_id: &str,
    approve: bool,
) -> Result<Request> {
    let lock = crate::task::lock_state(dir)?;
    let request = decide_locked(repo, dir, expected_task_id, approve)?;
    let record = crate::task::load(dir)?;
    drop(lock);
    notify_cmux(
        repo,
        &record,
        if approve {
            "Running"
        } else {
            "Approval rejected"
        },
    );
    Ok(request)
}

fn decide_locked(
    repo: &crate::git::Repo,
    dir: &Path,
    expected_task_id: &str,
    approve: bool,
) -> Result<Request> {
    let request_path = dir.join(REQUEST_FILE);
    let response_path = dir.join(RESPONSE_FILE);
    let request = read_request(&request_path)?;
    let record = crate::task::load(dir)?;
    if request.task_id != expected_task_id
        || record.task_id != expected_task_id
        || !matches!(record.state, crate::task::TaskState::WaitingForApproval)
        || record.repo_identity != repo.identity()
        || crate::state::confine_file(&dir.join("cancel.json"))?.is_some()
        || crate::state::confine_file(&response_path)?.is_some()
    {
        return Err(Error::new("approval is stale or already resolved"));
    }
    let response = Response {
        schema_version: 1,
        request_id: request.request_id.clone(),
        decision: if approve { "approve" } else { "reject" }.into(),
        decided_at: crate::task::now_rfc3339(),
    };
    crate::state::write_json(&response_path, &response)?;
    if approve {
        crate::task::set_state_locked(dir, crate::task::TaskState::Running)?;
    } else {
        crate::state::write_private_file(
            &dir.join("cancel.json"),
            serde_json::to_string(&json!({
                "requested_at": response.decided_at,
                "reason": "approval_rejected",
            }))?
            .as_bytes(),
        )?;
    }
    Ok(request)
}

fn notify_cmux(repo: &crate::git::Repo, record: &crate::task::TaskRecord, status: &str) {
    if record.cmux_workspace_id.is_none() {
        return;
    }
    if let Ok(client) = crate::cmux::Cmux::discover()
        && let manager = crate::cmux::repository::RepositoryManager::new(&client, repo)
    {
        let _ = manager.set_task_status(record, status);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (
        tempfile::TempDir,
        crate::git::Repo,
        String,
        std::path::PathBuf,
    ) {
        let temp = tempfile::tempdir().unwrap();
        crate::git::run_ok(temp.path(), &["init", "-q"]).unwrap();
        let repo = crate::git::discover(temp.path()).unwrap();
        let task_id = crate::task::new_task_id().unwrap();
        let dir = crate::state::worktree_task_dir(&repo.root, &repo.identity(), &task_id);
        let adapter = crate::harness::adapter_for("codex").unwrap();
        let delivery = crate::orchestration::deliver(None, "approval fixture")
            .unwrap()
            .1;
        let record = crate::task::TaskRecord {
            schema_version: crate::task::TASK_SCHEMA_VERSION,
            task_id: task_id.clone(),
            title: "approval fixture".into(),
            summary: String::new(),
            created_at: crate::task::now_rfc3339(),
            repo_identity: repo.identity(),
            repo_root: repo.root.clone(),
            branch: format!("ahu/test/{task_id}"),
            worktree: repo.root.clone(),
            base_commit: None,
            identity: crate::task::LaunchIdentity {
                mode: crate::task::LaunchMode::Automatic,
                agent: "auto".into(),
                agent_version: None,
                permissions: Default::default(),
                harness: "codex".into(),
                model: "gpt-6".into(),
                instructions_source: None,
                source_digest: None,
                instructions_digest: None,
                identity_digest: None,
                selection_basis: None,
            },
            policy_digest: "0".repeat(64),
            catalog_version: crate::catalog::CATALOG_VERSION.into(),
            config_snapshot: Default::default(),
            config_snapshot_digest: "0".repeat(64),
            hooks: Default::default(),
            hooks_digest: String::new(),
            materialize: Default::default(),
            launch_command: adapter
                .launch_command(&crate::harness::LaunchRequest {
                    model: "gpt-6",
                    prompt: "approval fixture",
                    cwd: &repo.root,
                    permissions: Default::default(),
                })
                .unwrap(),
            delivery,
            prompt_digest: crate::util::digest_bytes(b"approval fixture"),
            harness_executable: "/synthetic/codex".into(),
            enforcement: adapter.enforcement("gpt-6", Default::default()).unwrap(),
            reliability_warning: None,
            cmux_group_id: None,
            cmux_workspace_id: None,
            cmux_window_id: None,
            state: crate::task::TaskState::WaitingForApproval,
        };
        crate::task::save(&dir, &record, "approval fixture").unwrap();
        let request = Request {
            schema_version: 1,
            request_id: "0123456789abcdef".into(),
            task_id: task_id.clone(),
            operation: "network".into(),
            summary: "Fetch the requested issue metadata".into(),
            target: Some("https://example.invalid".into()),
            requested_at: crate::task::now_rfc3339(),
        };
        crate::state::write_json(&dir.join(REQUEST_FILE), &request).unwrap();
        (temp, repo, task_id, dir)
    }

    fn start_request_with_context(
        repo: &crate::git::Repo,
        task_id: &str,
        dir: &Path,
        cancellation: Option<std::path::PathBuf>,
    ) -> std::thread::JoinHandle<Result<serde_json::Value>> {
        std::fs::remove_file(dir.join(REQUEST_FILE)).unwrap();
        crate::task::set_state(dir, crate::task::TaskState::Running).unwrap();
        spawn_checkpoint(repo, task_id, dir, cancellation)
    }

    fn spawn_checkpoint(
        repo: &crate::git::Repo,
        task_id: &str,
        dir: &Path,
        cancellation: Option<std::path::PathBuf>,
    ) -> std::thread::JoinHandle<Result<serde_json::Value>> {
        let repo = repo.clone();
        // Exercise durable context serialization without the live environment.
        let context = Context {
            task_id: task_id.into(),
            task_dir: dir.into(),
            attempt: current_attempt(dir).unwrap(),
        };
        let context: Context =
            serde_json::from_slice(&serde_json::to_vec(&context).unwrap()).unwrap();
        let worker = std::thread::spawn(move || {
            request_with_context(
                &repo,
                "network",
                "inspect synthetic metadata",
                None,
                cancellation.as_deref(),
                Some(&context),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while crate::task::load(dir).unwrap().state != crate::task::TaskState::WaitingForApproval {
            assert!(
                Instant::now() < deadline,
                "waiter did not publish its checkpoint"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        worker
    }

    #[test]
    fn simultaneous_operator_decisions_have_exactly_one_winner() {
        let (_temp, repo, task_id, dir) = fixture();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let workers: Vec<_> = [true, false]
            .into_iter()
            .map(|approve| {
                let (repo, task_id, dir, barrier) =
                    (repo.clone(), task_id.clone(), dir.clone(), barrier.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    decide(&repo, &dir, &task_id, approve).is_ok()
                })
            })
            .collect();
        barrier.wait();
        let wins = workers
            .into_iter()
            .filter_map(|worker| worker.join().ok())
            .filter(|won| *won)
            .count();
        assert_eq!(wins, 1);
        let response: Response = crate::state::read_json(&dir.join(RESPONSE_FILE)).unwrap();
        assert_eq!(
            dir.join("cancel.json").exists(),
            response.decision == "reject"
        );
    }

    #[test]
    fn duplicate_request_cannot_steal_an_unconsumed_approval() {
        let (_temp, repo, task_id, dir) = fixture();
        let _lease = crate::task::StateLock::try_acquire(&dir.join("approval.worker"))
            .unwrap()
            .unwrap();
        decide(&repo, &dir, &task_id, true).unwrap();
        let before = std::fs::read(dir.join(RESPONSE_FILE)).unwrap();
        let result = request_for_task(
            &repo,
            &Context {
                task_id: task_id.clone(),
                task_dir: dir.clone(),
                attempt: None,
            },
            RequestInput {
                operation: "network",
                summary: "second request",
                target: None,
            },
            None,
            Duration::ZERO,
        );
        assert!(result.unwrap_err().to_string().contains("already pending"));
        assert_eq!(std::fs::read(dir.join(RESPONSE_FILE)).unwrap(), before);
        assert!(dir.join(REQUEST_FILE).exists());
    }

    #[test]
    fn cancellation_and_late_approval_preserve_terminal_state() {
        for terminal in [
            crate::task::TaskState::Exited,
            crate::task::TaskState::Failed,
            crate::task::TaskState::Cancelled,
        ] {
            let (_temp, repo, task_id, dir) = fixture();
            let cancellation = dir.join("mcp.cancel");
            let worker =
                start_request_with_context(&repo, &task_id, &dir, Some(cancellation.clone()));
            // Publish terminal state and client cancellation under the same
            // lock as the waiter, so it must see both on its next poll.
            let lock = crate::task::lock_state(&dir).unwrap();
            crate::task::set_state_locked(&dir, terminal).unwrap();
            std::fs::write(cancellation, b"cancelled").unwrap();
            drop(lock);
            assert!(decide(&repo, &dir, &task_id, true).is_err());
            assert!(worker.join().unwrap().is_err());
            assert_eq!(crate::task::load(&dir).unwrap().state, terminal);
            assert!(pending(&dir, &task_id).unwrap().is_none());
            assert!(!dir.join(RESPONSE_FILE).exists());
        }
    }

    #[test]
    fn rejected_request_never_returns_approval_and_prevents_reentry() {
        let (_temp, repo, task_id, dir) = fixture();
        let worker = start_request_with_context(&repo, &task_id, &dir, None);
        decide(&repo, &dir, &task_id, false).unwrap();
        assert!(worker.join().unwrap().is_err());
        assert!(dir.join("cancel.json").exists());
        assert!(pending(&dir, &task_id).unwrap().is_none());
        assert!(!dir.join(RESPONSE_FILE).exists());
        assert!(
            request_for_task(
                &repo,
                &Context {
                    task_id: task_id.clone(),
                    task_dir: dir.clone(),
                    attempt: None
                },
                RequestInput {
                    operation: "network",
                    summary: "retry",
                    target: None
                },
                None,
                Duration::ZERO
            )
            .is_err()
        );
    }

    fn write_attempt(dir: &Path, attempt: u32) {
        crate::state::write_json(
            &dir.join("headless.json"),
            &json!({
                "schema_version":2, "options":crate::headless::Options::default(),
                "harness_version":"synthetic", "executable_digest":"synthetic",
                "parent_task":null, "depth":0, "attempt":attempt, "session":null,
                "native_controls":[], "gaps":[]
            }),
        )
        .unwrap();
    }

    fn approve_after_resume(repo: &crate::git::Repo, id: &str, dir: &Path) {
        crate::task::set_state(dir, crate::task::TaskState::Failed).unwrap();
        let lease = retire_for_resume(dir, id, 1).unwrap();
        write_attempt(dir, 2);
        if dir.join("cancel.json").exists() {
            std::fs::remove_file(dir.join("cancel.json")).unwrap();
        }
        crate::task::set_state(dir, crate::task::TaskState::Running).unwrap();
        drop(lease);
        let worker = spawn_checkpoint(repo, id, dir, None);
        decide(repo, dir, id, true).unwrap();
        assert_eq!(worker.join().unwrap().unwrap()["approved"], true);
        assert!(pending(dir, id).unwrap().is_none());
        assert!(!dir.join(RESPONSE_FILE).exists());
    }

    #[test]
    fn rejected_and_cancelled_checkpoints_retire_before_resume() {
        for rejected in [true, false] {
            let (_temp, repo, id, dir) = fixture();
            write_attempt(&dir, 1);
            let worker = start_request_with_context(&repo, &id, &dir, None);
            if rejected {
                decide(&repo, &dir, &id, false).unwrap();
            } else {
                crate::state::write_json(&dir.join("cancel.json"), &json!({"reason":"synthetic"}))
                    .unwrap();
            }
            assert!(worker.join().unwrap().is_err());
            assert!(pending(&dir, &id).unwrap().is_none());
            assert!(!dir.join(RESPONSE_FILE).exists());
            assert!(dir.join("cancel.json").exists());
            assert_eq!(
                crate::task::load(&dir).unwrap().state,
                crate::task::TaskState::WaitingForApproval
            );
            approve_after_resume(&repo, &id, &dir);
        }
    }

    #[test]
    fn timed_out_checkpoint_retires_before_resume() {
        let (_temp, repo, id, dir) = fixture();
        write_attempt(&dir, 1);
        std::fs::remove_file(dir.join(REQUEST_FILE)).unwrap();
        crate::task::set_state(&dir, crate::task::TaskState::Running).unwrap();
        let result = request_for_task(
            &repo,
            &Context {
                task_id: id.clone(),
                task_dir: dir.clone(),
                attempt: Some(1),
            },
            RequestInput {
                operation: "network",
                summary: "timeout",
                target: None,
            },
            None,
            Duration::ZERO,
        );
        assert!(result.unwrap_err().to_string().contains("expired"));
        assert!(pending(&dir, &id).unwrap().is_none());
        assert!(!dir.join(RESPONSE_FILE).exists());
        assert!(dir.join("cancel.json").exists());
        approve_after_resume(&repo, &id, &dir);
    }

    #[test]
    fn attempt_change_retires_old_checkpoint_without_restoring_running() {
        let (_temp, repo, id, dir) = fixture();
        write_attempt(&dir, 1);
        let worker = start_request_with_context(&repo, &id, &dir, None);
        let lock = crate::task::lock_state(&dir).unwrap();
        write_attempt(&dir, 2);
        crate::task::set_state_locked(&dir, crate::task::TaskState::Starting).unwrap();
        drop(lock);
        assert!(worker.join().unwrap().is_err());
        assert!(pending(&dir, &id).unwrap().is_none());
        assert!(!dir.join(RESPONSE_FILE).exists());
        assert_eq!(
            crate::task::load(&dir).unwrap().state,
            crate::task::TaskState::Starting
        );
    }

    #[test]
    fn explicit_resume_retires_abandoned_decisions_without_replaying_them() {
        for decision in [None, Some(false), Some(true)] {
            let (_temp, repo, id, dir) = fixture();
            write_attempt(&dir, 1);
            if let Some(approve) = decision {
                decide(&repo, &dir, &id, approve).unwrap();
            }
            crate::task::set_state(&dir, crate::task::TaskState::Failed).unwrap();
            let before = crate::task::load(&dir).unwrap().state;
            let lease = retire_for_resume(&dir, &id, 1).unwrap();
            assert_eq!(crate::task::load(&dir).unwrap().state, before);
            assert!(pending(&dir, &id).unwrap().is_none());
            assert!(!dir.join(RESPONSE_FILE).exists());
            drop(lease);
            approve_after_resume(&repo, &id, &dir);
        }
    }

    #[test]
    fn resume_cleanup_refuses_active_workers_foreign_attempts_and_mismatched_responses() {
        let (_temp, _repo, id, dir) = fixture();
        write_attempt(&dir, 1);
        assert!(retire_for_resume(&dir, &id, 1).is_err());
        crate::task::set_state(&dir, crate::task::TaskState::Failed).unwrap();
        assert!(retire_for_resume(&dir, "other-task", 1).is_err());
        assert!(retire_for_resume(&dir, &id, 2).is_err());
        let lease = crate::task::StateLock::try_acquire(&dir.join("approval.worker"))
            .unwrap()
            .unwrap();
        assert!(retire_for_resume(&dir, &id, 1).is_err());
        drop(lease);
        crate::state::write_json(
            &dir.join(RESPONSE_FILE),
            &Response {
                schema_version: 1,
                request_id: "fedcba9876543210".into(),
                decision: "approve".into(),
                decided_at: crate::task::now_rfc3339(),
            },
        )
        .unwrap();
        assert!(retire_for_resume(&dir, &id, 1).is_err());
        assert!(pending(&dir, &id).unwrap().is_some());
        assert!(dir.join(RESPONSE_FILE).exists());
    }

    #[test]
    fn dangling_checkpoint_links_are_refused() {
        let (_temp, repo, task_id, dir) = fixture();
        std::fs::remove_file(dir.join(REQUEST_FILE)).unwrap();
        std::os::unix::fs::symlink(dir.join("missing"), dir.join(REQUEST_FILE)).unwrap();
        assert!(pending(&dir, &task_id).is_err());
        assert!(decide(&repo, &dir, &task_id, true).is_err());
    }

    #[test]
    fn durable_context_refuses_another_task_or_attempt() {
        let (_temp, repo, _task_id, dir) = fixture();
        let context = Context {
            task_id: "another-task".into(),
            task_dir: dir.clone(),
            attempt: None,
        };
        assert!(
            request_with_context(
                &repo,
                "network",
                "synthetic request",
                None,
                None,
                Some(&context)
            )
            .is_err()
        );
        let context = Context {
            attempt: Some(2),
            ..context
        };
        assert!(
            request_with_context(
                &repo,
                "network",
                "synthetic request",
                None,
                None,
                Some(&context)
            )
            .unwrap_err()
            .to_string()
            .contains("different task attempt")
        );
    }

    #[test]
    fn pending_approval_round_trips_and_is_scoped_to_its_task() {
        let (_temp, _repo, task_id, dir) = fixture();
        let request = pending(&dir, &task_id).unwrap().unwrap();
        assert_eq!(request.task_id, task_id);
        assert_eq!(request.operation, "network");
        assert!(pending(&dir, "another-task").is_err());
    }

    #[test]
    fn request_enters_waiting_state_and_resumes_after_operator_approval() {
        let (_temp, repo, task_id, dir) = fixture();
        std::fs::remove_file(dir.join(REQUEST_FILE)).unwrap();
        crate::task::set_state(&dir, crate::task::TaskState::Running).unwrap();
        let worker_repo = repo.clone();
        let worker_task_id = task_id.clone();
        let worker_dir = dir.clone();
        let worker = std::thread::spawn(move || {
            request_for_task(
                &worker_repo,
                &Context {
                    task_id: worker_task_id.clone(),
                    task_dir: worker_dir.clone(),
                    attempt: None,
                },
                RequestInput {
                    operation: "external-write",
                    summary: "publish the reviewed release",
                    target: Some("release registry"),
                },
                None,
                MAX_WAIT,
            )
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !matches!(
            crate::task::load(&dir).unwrap().state,
            crate::task::TaskState::WaitingForApproval
        ) && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            crate::task::load(&dir).unwrap().state,
            crate::task::TaskState::WaitingForApproval
        );
        assert_eq!(
            pending(&dir, &task_id).unwrap().unwrap().operation,
            "external-write"
        );
        decide(&repo, &dir, &task_id, true).unwrap();
        let result = worker.join().unwrap().unwrap();
        assert_eq!(result["approved"], true);
        assert_eq!(
            crate::task::load(&dir).unwrap().state,
            crate::task::TaskState::Running
        );
    }

    #[test]
    fn timeout_cancels_the_checkpoint_and_mcp_cancellation_releases_the_parent() {
        let (_temp, repo, task_id, dir) = fixture();
        std::fs::remove_file(dir.join(REQUEST_FILE)).unwrap();
        crate::task::set_state(&dir, crate::task::TaskState::Running).unwrap();
        let worker_repo = repo.clone();
        let worker_task_id = task_id.clone();
        let worker_dir = dir.clone();
        let timed_out = request_for_task(
            &worker_repo,
            &Context {
                task_id: worker_task_id.clone(),
                task_dir: worker_dir.clone(),
                attempt: None,
            },
            RequestInput {
                operation: "network",
                summary: "fetch remote context",
                target: None,
            },
            None,
            Duration::from_millis(20),
        );
        assert!(timed_out.unwrap_err().to_string().contains("expired"));
        assert!(dir.join("cancel.json").exists());
        assert!(pending(&dir, &task_id).unwrap().is_none());
        assert!(!dir.join(RESPONSE_FILE).exists());

        let (_temp2, repo, task_id, dir) = fixture();
        std::fs::remove_file(dir.join(REQUEST_FILE)).unwrap();
        crate::task::set_state(&dir, crate::task::TaskState::Running).unwrap();
        let cancellation = dir.join("mcp.cancel");
        let worker_repo = repo.clone();
        let worker_task_id = task_id.clone();
        let worker_dir = dir.clone();
        let worker_cancel = cancellation.clone();
        let worker = std::thread::spawn(move || {
            request_for_task(
                &worker_repo,
                &Context {
                    task_id: worker_task_id.clone(),
                    task_dir: worker_dir.clone(),
                    attempt: None,
                },
                RequestInput {
                    operation: "network",
                    summary: "fetch remote context",
                    target: None,
                },
                Some(&worker_cancel),
                MAX_WAIT,
            )
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !matches!(
            crate::task::load(&dir).unwrap().state,
            crate::task::TaskState::WaitingForApproval
        ) && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::fs::write(&cancellation, b"cancelled").unwrap();
        assert!(
            worker
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
        assert_eq!(
            crate::task::load(&dir).unwrap().state,
            crate::task::TaskState::Running
        );
        assert!(pending(&dir, &task_id).unwrap().is_none());
        assert!(!dir.join(RESPONSE_FILE).exists());
        let next = spawn_checkpoint(&repo, &task_id, &dir, None);
        decide(&repo, &dir, &task_id, true).unwrap();
        assert_eq!(next.join().unwrap().unwrap()["approved"], true);
    }

    #[test]
    fn approve_resolves_only_the_current_request_and_restores_running_state() {
        let (_temp, repo, task_id, dir) = fixture();
        let request = decide(&repo, &dir, &task_id, true).unwrap();
        assert_eq!(request.operation, "network");
        assert_eq!(
            crate::task::load(&dir).unwrap().state,
            crate::task::TaskState::Running
        );
        let response: Response = crate::state::read_json(&dir.join(RESPONSE_FILE)).unwrap();
        assert_eq!(response.request_id, request.request_id);
        assert_eq!(response.decision, "approve");
        assert!(decide(&repo, &dir, &task_id, true).is_err());
    }

    #[test]
    fn reject_keeps_its_decision_available_until_the_waiter_retires_it() {
        let (_temp, repo, task_id, dir) = fixture();
        let request = decide(&repo, &dir, &task_id, false).unwrap();
        assert_eq!(request.task_id, task_id);
        assert!(dir.join("cancel.json").exists());
        assert_eq!(
            crate::task::load(&dir).unwrap().state,
            crate::task::TaskState::WaitingForApproval
        );
        assert!(pending(&dir, &task_id).unwrap().is_some());
    }
}

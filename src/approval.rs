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
    if !matches!(
        operation,
        "external-write" | "network" | "destructive" | "other"
    ) || !valid_text(summary)
        || target.is_some_and(|value| !valid_text(value))
    {
        return Err(Error::new("invalid approval request"));
    }
    let task_id = std::env::var("AHU_TASK_ID")
        .map_err(|_| Error::new("approval requests require an Ahu-managed task context"))?;
    let task_dir = std::env::var_os("AHU_TASK_DIR")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| Error::new("approval requests require an Ahu-managed task context"))?;
    request_for_task(
        repo,
        &task_id,
        &task_dir,
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
    task_id: &str,
    task_dir: &Path,
    input: RequestInput<'_>,
    cancellation: Option<&Path>,
    max_wait: Duration,
) -> Result<serde_json::Value> {
    crate::state::confine_file(&task_dir.join("task.json"))?;
    let record = crate::task::load(task_dir)?;
    if record.task_id != task_id
        || record.repo_identity != repo.identity()
        || record.worktree.canonicalize().ok() != repo.root.canonicalize().ok()
        || !record.state.is_live()
    {
        return Err(Error::new(
            "approval request does not belong to the current live task",
        ));
    }
    let (request_path, response_path) = paths(task_dir);
    if response_path.exists() {
        let response: Response = crate::state::read_json(&response_path)?;
        if response.request_id != read_request(&request_path)?.request_id {
            return Err(Error::new("a stale approval response is present"));
        }
        std::fs::remove_file(&request_path)?;
        std::fs::remove_file(&response_path)?;
    } else if request_path.exists() {
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
    if let Err(error) = set_task_waiting(repo, task_dir, &record) {
        let _ = std::fs::remove_file(&request_path);
        return Err(error);
    }
    let deadline = Instant::now() + max_wait;
    loop {
        if cancellation.is_some_and(|path| path.exists()) {
            let _ = std::fs::remove_file(&request_path);
            crate::task::set_state(task_dir, crate::task::TaskState::Running)?;
            notify_cmux(repo, &record, "Running");
            return Err(Error::new(
                "approval request was cancelled by the MCP client",
            ));
        }
        if task_dir.join("cancel.json").exists() {
            return Err(Error::new("task was cancelled while approval was pending"));
        }
        if response_path.exists() {
            let response: Response = crate::state::read_json(&response_path)?;
            if response.schema_version != 1 || response.request_id != request.request_id {
                return Err(Error::new("approval response does not match this request"));
            }
            let _ = std::fs::remove_file(&request_path);
            let _ = std::fs::remove_file(&response_path);
            if response.decision == "approve" {
                crate::task::set_state(task_dir, crate::task::TaskState::Running)?;
                notify_cmux(repo, &record, "Running");
                return Ok(json!({"approved":true,"request_id":request.request_id}));
            }
            return Err(Error::new("operator rejected the requested operation"));
        }
        if Instant::now() >= deadline {
            let _ = decide(repo, task_dir, &request.task_id, false);
            return Err(Error::new("approval request expired after 30 minutes"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn read_request(path: &Path) -> Result<Request> {
    let request: Request = crate::state::read_json(path)?;
    if request.schema_version != 1
        || request.request_id.len() != 16
        || !request
            .request_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
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
    if !request_path.exists() {
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
    let request_path = dir.join(REQUEST_FILE);
    let response_path = dir.join(RESPONSE_FILE);
    let request = read_request(&request_path)?;
    let record = crate::task::load(dir)?;
    if request.task_id != expected_task_id
        || record.task_id != expected_task_id
        || !matches!(record.state, crate::task::TaskState::WaitingForApproval)
        || record.repo_identity != repo.identity()
        || dir.join("cancel.json").exists()
        || response_path.exists()
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
        crate::task::set_state(dir, crate::task::TaskState::Running)?;
        notify_cmux(repo, &record, "Running");
    } else {
        crate::state::write_private_file(
            &dir.join("cancel.json"),
            serde_json::to_string(&json!({
                "requested_at": response.decided_at,
                "reason": "approval_rejected",
            }))?
            .as_bytes(),
        )?;
        notify_cmux(repo, &record, "Approval rejected");
    }
    Ok(request)
}

fn set_task_waiting(
    repo: &crate::git::Repo,
    dir: &Path,
    record: &crate::task::TaskRecord,
) -> Result<()> {
    crate::task::set_state(dir, crate::task::TaskState::WaitingForApproval)?;
    notify_cmux(repo, record, "Waiting for approval");
    Ok(())
}

fn notify_cmux(repo: &crate::git::Repo, record: &crate::task::TaskRecord, status: &str) {
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
                &worker_task_id,
                &worker_dir,
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
            &worker_task_id,
            &worker_dir,
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

        std::fs::remove_file(dir.join("cancel.json")).unwrap();
        crate::task::set_state(&dir, crate::task::TaskState::Running).unwrap();
        let cancellation = dir.join("mcp.cancel");
        let worker_repo = repo.clone();
        let worker_task_id = task_id.clone();
        let worker_dir = dir.clone();
        let worker_cancel = cancellation.clone();
        let worker = std::thread::spawn(move || {
            request_for_task(
                &worker_repo,
                &worker_task_id,
                &worker_dir,
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
    fn reject_records_cancellation_without_clearing_the_pending_evidence() {
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

//! Durable Tasks extension for the local stdio transport. Only replay-safe
//! inspection operations are queued. Protocol completion never accepts work.
use super::{TASKS_EXTENSION, call_response, response, rpc_error};
use crate::{
    git::Repo,
    state,
    util::{Error, Result},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf};

const MAX_UPDATE: usize = 8192;
const INPUT_KEY: &str = "task-selection";

fn capabilities(params: &Value) -> &Value {
    &params["_meta"]["io.modelcontextprotocol/clientCapabilities"]
}
fn capable(params: &Value) -> bool {
    capabilities(params)["extensions"][TASKS_EXTENSION].is_object()
}
fn elicitation(params: &Value) -> bool {
    capabilities(params)["elicitation"]["form"].is_object()
}
fn missing(id: &Value) -> Value {
    let mut error = rpc_error(id, -32021, "MCP Tasks extension required");
    error["error"]["data"] = json!({"requiredCapabilities":{"extensions":{TASKS_EXTENSION:{}}}});
    error
}

pub(super) struct Session {
    owner: String,
    legacy: bool,
    modern: bool,
    enabled: bool,
    worker_started: bool,
    inspection_adapter: bool,
    subscriptions: BTreeMap<String, Value>,
    subscription_id: Option<Value>,
    acknowledged: bool,
}
impl Session {
    pub(super) fn new() -> Result<Self> {
        // The stdio host is the authentication boundary. Never take caller
        // identity from JSON-RPC parameters, metadata, or clientInfo.
        let owner = std::env::var("AHU_MCP_CALLER").unwrap_or_else(|_| {
            // SAFETY: geteuid has no preconditions.
            format!("local-uid:{}", unsafe { libc::geteuid() })
        });
        if owner.is_empty() || owner.len() > 256 || owner.chars().any(char::is_control) {
            return Err(Error::new("invalid AHU_MCP_CALLER"));
        }
        Ok(Self {
            owner,
            legacy: false,
            modern: false,
            enabled: false,
            worker_started: false,
            inspection_adapter: std::env::var("AHU_MCP_TASKS_ADAPTER").as_deref()
                == Ok("inspection-v1"),
            subscriptions: BTreeMap::new(),
            subscription_id: None,
            acknowledged: false,
        })
    }

    pub(super) fn legacy(&self) -> bool {
        self.legacy
    }

    pub(super) fn modern(&self) -> bool {
        self.modern
    }

    pub(super) fn mark_modern(&mut self) {
        self.modern = true;
    }

    pub(super) fn handle(
        &mut self,
        repo: &Repo,
        id: &Value,
        method: &str,
        params: &Value,
    ) -> Option<Value> {
        if method == "initialize" {
            self.legacy = true;
            self.subscriptions.clear();
            return None;
        }
        let task_method = matches!(
            method,
            "tasks/get" | "tasks/update" | "tasks/cancel" | "subscriptions/listen"
        );
        if task_method && (self.legacy || !capable(params)) {
            return Some(missing(id));
        }
        if self.legacy {
            return None;
        }
        if method == "tools/list" && self.inspection_adapter && capable(params) {
            let mut tools = super::tools();
            tools.push(json!({"name":"ahu_task_inspect", "description":"Inspect a repository task, requesting a task selector when omitted (experimental inspection adapter).", "inputSchema":{"type":"object","properties":{"task":{"type":"string","maxLength":256}},"additionalProperties":false}}));
            return Some(response(id, json!({"tools":tools})));
        }
        if method == "tools/call" && capable(params) {
            self.enabled = true;
            return Some(match self.create(repo, params) {
                Ok(task) => match task.save(repo) {
                    Ok(()) => response(id, task.view("task")),
                    Err(error) => rpc_error(id, -32603, error.to_string()),
                },
                Err(error) => rpc_error(id, -32602, error.to_string()),
            });
        }
        if !task_method {
            return None;
        }
        self.enabled = true;
        if method == "subscriptions/listen" {
            return Some(self.subscribe(repo, id, params));
        }
        let task_id = params["taskId"].as_str().unwrap_or("");
        let path = match task_path(repo, task_id) {
            Ok(path) => path,
            Err(_) => return Some(rpc_error(id, -32602, "invalid task id")),
        };
        // Authorize before touching lock state, then recheck under the lock.
        if load(repo, task_id, &self.owner).is_err() {
            return Some(rpc_error(id, -32602, "unknown task id"));
        }
        // Reads and mutations share the same process-safe lock; work itself is
        // outside this lock, so cancellation can win while inspection runs.
        let _lock = match Lock::acquire(&path.with_extension("lock")) {
            Ok(lock) => lock,
            Err(error) => return Some(rpc_error(id, -32603, error.to_string())),
        };
        let mut task = match load(repo, task_id, &self.owner) {
            Ok(task) => task,
            Err(_) => return Some(rpc_error(id, -32602, "unknown task id")),
        };
        let result = match method {
            "tasks/get" => return Some(response(id, task.view("complete"))),
            "tasks/update" => task.update(params),
            "tasks/cancel" => {
                if task.active() {
                    task.cancel_requested = true;
                    // This adapter has no external side effects. At this
                    // checkpoint it can cancel immediately; a racing worker
                    // rechecks persisted cancellation before publishing output.
                    task.status = "cancelled".into();
                    task.input_requests = None;
                    task.changed();
                }
                Ok(())
            }
            _ => unreachable!(),
        };
        if let Err(error) = result {
            return Some(rpc_error(id, -32602, error.to_string()));
        }
        Some(match task.save(repo) {
            Ok(()) => response(id, json!({"resultType":"complete"})),
            Err(error) => rpc_error(id, -32603, error.to_string()),
        })
    }

    fn create(&self, repo: &Repo, params: &Value) -> Result<StoredTask> {
        let name = params["name"]
            .as_str()
            .ok_or_else(|| Error::new("missing tool name"))?;
        let adapter = name == "ahu_task_inspect" && self.inspection_adapter;
        if !adapter && !matches!(name, "ahu_agents_list" | "ahu_tasks_list" | "ahu_task_get") {
            return Err(Error::new("unknown ahu MCP tool"));
        }
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let object = arguments
            .as_object()
            .ok_or_else(|| Error::new("arguments must be an object"))?;
        if serde_json::to_vec(&arguments)?.len() > MAX_UPDATE || object.keys().any(|k| k != "task")
        {
            return Err(Error::new("invalid inspection arguments"));
        }
        if let Some(selector) = object.get("task") {
            if !selector
                .as_str()
                .is_some_and(|s| !s.is_empty() && s.len() <= 256)
            {
                return Err(Error::new("invalid task selector"));
            }
        } else if adapter && !elicitation(params) {
            return Err(Error::new(
                "inspection adapter requires elicitation.form capability",
            ));
        }
        let timestamp = crate::task::now_rfc3339();
        let task = StoredTask {
            task_id: crate::task::new_task_id()?,
            owner: self.owner.clone(),
            repository: repo.identity(),
            checkout: repo.root.clone(),
            status: "working".into(),
            created_at: timestamp.clone(),
            last_updated_at: timestamp,
            ttl_ms: None,
            revision: 0,
            cancel_requested: false,
            name: if adapter {
                "ahu_task_get".into()
            } else {
                name.into()
            },
            request_input: adapter && object.get("task").is_none(),
            arguments,
            input_requests: None,
            result: None,
            error: None,
        };
        Ok(task)
    }

    fn subscribe(&mut self, repo: &Repo, id: &Value, params: &Value) -> Value {
        let Some(ids) = params["notifications"]["taskIds"].as_array() else {
            return rpc_error(id, -32602, "notifications.taskIds must be an array");
        };
        if ids.len() > 64 {
            return rpc_error(id, -32602, "too many task subscriptions");
        }
        let mut subscriptions = BTreeMap::new();
        for id_value in ids {
            let Some(task_id) = id_value.as_str() else {
                return rpc_error(id, -32602, "invalid task id");
            };
            if load(repo, task_id, &self.owner).is_err() {
                return rpc_error(id, -32602, "unknown task id");
            }
            subscriptions.insert(task_id.into(), Value::Null);
        }
        self.subscriptions = subscriptions;
        self.subscription_id = Some(id.clone());
        self.acknowledged = true;
        response(id, json!({"resultType":"complete"}))
    }

    pub(super) fn notifications(&mut self, repo: &Repo) -> Result<Vec<Value>> {
        let mut messages = Vec::new();
        if self.acknowledged {
            let subscription_id = self.subscription_id.clone().unwrap_or(Value::Null);
            messages.push(json!({"jsonrpc":"2.0","method":"notifications/subscriptions/acknowledged","params":{"_meta":{"io.modelcontextprotocol/subscriptionId":subscription_id},"notifications":{"taskIds":self.subscriptions.keys().collect::<Vec<_>>()}}}));
            self.acknowledged = false;
        }
        for (id, previous) in &mut self.subscriptions {
            // Reauthorize every emission, including transitions from other
            // server processes. Never publish a task read under another caller.
            let Ok(task) = load(repo, id, &self.owner) else {
                continue;
            };
            let view = task.view("complete");
            if view != *previous {
                let mut params = view;
                params["_meta"]["io.modelcontextprotocol/subscriptionId"] =
                    self.subscription_id.clone().unwrap_or(Value::Null);
                messages
                    .push(json!({"jsonrpc":"2.0","method":"notifications/tasks","params":params}));
                *previous = task.view("complete");
            }
        }
        Ok(messages)
    }

    pub(super) fn start_worker(&mut self, repo: &Repo) {
        if !self.enabled || self.worker_started || self.legacy {
            return;
        }
        self.worker_started = true;
        let repo = repo.clone();
        let owner = self.owner.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if let Err(error) = work(&repo, &owner) {
                    eprintln!("MCP task worker: {error}");
                }
            }
        });
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredTask {
    task_id: String,
    owner: String,
    repository: String,
    checkout: PathBuf,
    status: String,
    created_at: String,
    last_updated_at: String,
    ttl_ms: Option<u64>,
    revision: u64,
    cancel_requested: bool,
    name: String,
    arguments: Value,
    request_input: bool,
    input_requests: Option<Value>,
    result: Option<Value>,
    error: Option<Value>,
}
impl StoredTask {
    fn active(&self) -> bool {
        matches!(self.status.as_str(), "working" | "input_required")
    }
    fn changed(&mut self) {
        self.revision += 1;
        self.last_updated_at = crate::task::now_rfc3339();
    }
    fn view(&self, result_type: &str) -> Value {
        let mut value = json!({"resultType":result_type,"taskId":self.task_id,"status":self.status,
            "createdAt":self.created_at,"lastUpdatedAt":self.last_updated_at,"ttlMs":self.ttl_ms,"pollIntervalMs":100,
            "_meta":{super::SERVER_INFO_META:{"name":super::SERVER_NAME,"version":super::SERVER_VERSION}}});
        if result_type != "task" {
            for (key, data) in [
                ("inputRequests", &self.input_requests),
                ("result", &self.result),
                ("error", &self.error),
            ] {
                if let Some(data) = data {
                    value[key] = data.clone();
                }
            }
        }
        value
    }
    fn save(&self, repo: &Repo) -> Result<()> {
        let path = task_path(repo, &self.task_id)?;
        state::confine_file(&path)?;
        crate::private_io::atomic_write(
            &path,
            &serde_json::to_vec(self)?,
            crate::private_io::Durability::Durable,
        )
    }
    fn update(&mut self, params: &Value) -> Result<()> {
        let object = params
            .as_object()
            .ok_or_else(|| Error::new("invalid update"))?;
        if serde_json::to_vec(params)?.len() > MAX_UPDATE
            || object
                .keys()
                .any(|k| !matches!(k.as_str(), "taskId" | "inputResponses" | "_meta"))
        {
            return Err(Error::new("update exceeds allowed payload"));
        }
        let inputs = params["inputResponses"]
            .as_object()
            .ok_or_else(|| Error::new("inputResponses must be an object"))?;
        if self.status != "input_required" {
            return Ok(());
        }
        let Some(input) = inputs.get(INPUT_KEY) else {
            return Ok(());
        };
        if !elicitation(params) {
            return Err(Error::new("elicitation.form capability required"));
        }
        let answer = input
            .as_object()
            .ok_or_else(|| Error::new("invalid input response"))?;
        if answer
            .keys()
            .any(|k| !matches!(k.as_str(), "action" | "content"))
        {
            return Err(Error::new("unexpected input response field"));
        }
        match input["action"].as_str() {
            Some("accept") => {
                let content = input["content"]
                    .as_object()
                    .ok_or_else(|| Error::new("missing content"))?;
                let selector = content
                    .get("task")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty() && s.len() <= 256)
                    .ok_or_else(|| Error::new("invalid task selector"))?;
                if content.len() != 1 {
                    return Err(Error::new("unexpected content field"));
                }
                self.arguments = json!({"task":selector});
                self.status = "working".into();
            }
            Some("decline" | "cancel") => {
                self.cancel_requested = true;
                self.status = "cancelled".into();
            }
            _ => return Err(Error::new("invalid elicitation action")),
        }
        self.input_requests = None;
        self.request_input = false;
        self.changed();
        Ok(())
    }
}
fn store(repo: &Repo) -> Result<PathBuf> {
    let dir = state::coordination_dir(repo)?.join("mcp/tasks");
    state::create_private_dir_all(&dir)?;
    Ok(dir)
}
fn task_path(repo: &Repo, id: &str) -> Result<PathBuf> {
    if !crate::task::is_canonical_task_uuid(id) {
        return Err(Error::new("invalid task id"));
    }
    Ok(store(repo)?.join(format!("{id}.json")))
}
fn load(repo: &Repo, id: &str, owner: &str) -> Result<StoredTask> {
    let task: StoredTask =
        serde_json::from_slice(&state::read_private_file(&task_path(repo, id)?)?)?;
    if task.task_id != id
        || task.owner != owner
        || task.repository != repo.identity()
        || task.checkout != repo.root
    {
        return Err(Error::new("unknown task id"));
    }
    Ok(task)
}
fn work(repo: &Repo, owner: &str) -> Result<()> {
    for entry in std::fs::read_dir(store(repo)?)? {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(task) = load(repo, id, owner) else {
            continue;
        };
        if task.status != "working" {
            continue;
        }
        // A separate execution lease prevents duplicate execution across
        // reconnects. The kernel releases it if the worker process dies.
        let Some(_execution) = Lock::try_acquire(&path.with_extension("worker"))? else {
            continue;
        };
        let snapshot = {
            let _lock = Lock::acquire(&path.with_extension("lock"))?;
            let mut task = load(repo, id, owner)?;
            if task.status != "working" || task.cancel_requested {
                continue;
            }
            if task.request_input {
                task.status = "input_required".into();
                task.input_requests = Some(
                    json!({INPUT_KEY:{"method":"elicitation/create","params":{
                        "mode":"form","message":"Which repository task should be inspected?",
                        "requestedSchema":{"type":"object","properties":{"task":{"type":"string","maxLength":256}},"required":["task"],"additionalProperties":false}
                    }}}),
                );
                task.changed();
                task.save(repo)?;
                continue;
            }
            task
        };
        let output = call_response(
            repo,
            &Value::Null,
            &json!({"name":snapshot.name,"arguments":snapshot.arguments}),
        );
        let _lock = Lock::acquire(&path.with_extension("lock"))?;
        let mut task = load(repo, id, owner)?;
        if task.status != "working" || task.cancel_requested || task.revision != snapshot.revision {
            continue;
        }
        if let Some(error) = output.get("error") {
            task.status = "failed".into();
            task.error = Some(error.clone());
        } else {
            task.status = "completed".into();
            task.result = Some(output["result"].clone());
        }
        task.changed();
        task.save(repo)?;
    }
    Ok(())
}

// Persistent lock files must never be unlinked: all processes must lock the
// same inode. Kernel ownership, rather than a timestamp, survives crashes safely.
struct Lock(std::fs::File);
impl Lock {
    fn acquire(path: &std::path::Path) -> Result<Self> {
        for _ in 0..50 {
            if let Some(lock) = Self::try_acquire(path)? {
                return Ok(lock);
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        Err(Error::new("MCP task busy; retry request"))
    }
    fn try_acquire(path: &std::path::Path) -> Result<Option<Self>> {
        use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
        state::confine_file(path)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)?;
        crate::storage::validate_owned_metadata(&file.metadata()?, true)?;
        // SAFETY: the file owns a valid descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(None);
            }
            return Err(error.into());
        }
        Ok(Some(Self(file)))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        // SAFETY: the file is still open.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

//! A persistent, bounded executable transport for Host V1.
use super::{plugins::PluginStatus, v1::*};
use crate::util::{Error, ErrorKind, Result};
use ahu_host_api::protocol::{self, ErrorCode, Hello, Request, Response};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::{
    fs::{OpenOptionsExt, PermissionsExt},
    process::CommandExt,
};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Executable {
    pub path: String,
    pub protocol_version: u32,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}
fn default_timeout() -> u64 {
    5000
}
impl Executable {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.protocol_version != protocol::VERSION || !(100..=30_000).contains(&self.timeout_ms)
        {
            return Err(Error::new(
                "executable requires protocol_version = 1 and timeout_ms between 100 and 30000",
            )
            .with_kind(ErrorKind::Usage));
        }
        Ok(())
    }
}

struct RpcError {
    error: Error,
    dispatched: bool,
    not_created: bool,
}
impl From<RpcError> for Error {
    fn from(error: RpcError) -> Self {
        error.error
    }
}

struct Process {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
    _snapshot: tempfile::TempDir,
    timeout: Duration,
    next_id: u64,
    broken: bool,
    cancelled: Arc<AtomicBool>,
}

// Ahu controls only this process group, not a backend's independent sessions.
// A hostile same-user program may escape the group; this is not a sandbox.
impl Drop for Process {
    fn drop(&mut self) {
        // Do not reap before signalling: an unreaped child's PID cannot be reused.
        // SAFETY: child was spawned as leader of its own process group.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let until = Instant::now() + Duration::from_millis(250);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                _ if Instant::now() >= until => break,
                _ => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }
}

fn nonblocking(fd: RawFd) -> Result<()> {
    // SAFETY: fd is a live pipe owned by Process, and fcntl does not own it.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(Error::new("cannot configure external host pipes"));
    }
    Ok(())
}
fn ready(fd: RawFd, events: i16, deadline: Instant, cancelled: &AtomicBool) -> Result<()> {
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::new(
                "external host request cancelled; outcome may be unknown",
            ));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(Error::new(
                "external host request timed out; outcome may be unknown",
            ));
        }
        let mut poll = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        // SAFETY: poll points to one valid descriptor entry.
        let result = unsafe { libc::poll(&mut poll, 1, remaining.as_millis().clamp(1, 50) as i32) };
        if result > 0 {
            return Ok(());
        }
        if result < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return Err(Error::new("external host pipe polling failed"));
        }
    }
}
fn read_exact_until(
    pipe: &mut ChildStdout,
    mut bytes: &mut [u8],
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<()> {
    while !bytes.is_empty() {
        ready(pipe.as_raw_fd(), libc::POLLIN, deadline, cancelled)?;
        match pipe.read(bytes) {
            Ok(0) => return Err(Error::new("external host closed its response pipe")),
            Ok(n) => bytes = &mut bytes[n..],
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(_) => return Err(Error::new("external host response read failed")),
        }
    }
    Ok(())
}
impl Process {
    fn abort(&mut self) {
        self.broken = true;
        // SAFETY: this child is unreaped and owns its process group.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
    }
    fn rpc(&mut self, method: &str, params: Value) -> std::result::Result<Value, RpcError> {
        let mut dispatched = false;
        let result = (|| -> Result<Response> {
            if self.broken {
                return Err(Error::new(
                    "external host connection is unusable; no request was sent",
                ));
            }
            // Reject unsolicited or leftover bytes before starting another request.
            let mut extra = [0];
            match self.output.read(&mut extra) {
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                _ => {
                    return Err(Error::new(
                        "external host exited or sent unsolicited output",
                    ));
                }
            }
            let id = self.next_id;
            let bytes = serde_json::to_vec(&Request {
                protocol_version: protocol::VERSION,
                id,
                method: method.into(),
                params,
            })?;
            if bytes.len() > protocol::MAX_FRAME_BYTES {
                return Err(Error::new(
                    "external host request exceeds 1 MiB; no request was sent",
                ));
            }
            self.next_id = id
                .checked_add(1)
                .ok_or_else(|| Error::new("external host request id exhausted"))?;
            let deadline = Instant::now() + self.timeout;
            let mut frame = (bytes.len() as u32).to_be_bytes().to_vec();
            frame.extend(bytes);
            let mut remaining = frame.as_slice();
            while !remaining.is_empty() {
                ready(
                    self.input.as_raw_fd(),
                    libc::POLLOUT,
                    deadline,
                    &self.cancelled,
                )?;
                match self.input.write(remaining) {
                    Ok(0) => return Err(Error::new("external host request pipe closed")),
                    Ok(n) => {
                        dispatched = true;
                        remaining = &remaining[n..];
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                        ) => {}
                    Err(_) => return Err(Error::new("external host request write failed")),
                }
            }
            let mut length = [0; 4];
            read_exact_until(&mut self.output, &mut length, deadline, &self.cancelled)?;
            let length = u32::from_be_bytes(length) as usize;
            if length == 0 || length > protocol::MAX_FRAME_BYTES {
                return Err(Error::new(
                    "external host response frame must be 1 byte to 1 MiB",
                ));
            }
            let mut response = vec![0; length];
            read_exact_until(&mut self.output, &mut response, deadline, &self.cancelled)?;
            let response: Response = protocol::from_slice(&response)
                .map_err(|_| Error::new("external host sent an invalid response envelope"))?;
            let (version, response_id) = match &response {
                Response::Success(r) => (r.protocol_version, r.id),
                Response::Failure(r) => (r.protocol_version, r.id),
            };
            if version != protocol::VERSION || response_id != id {
                return Err(Error::new(
                    "external host response version or request id mismatch",
                ));
            }
            Ok(response)
        })();
        match result {
            Ok(Response::Success(r)) => Ok(r.result),
            Ok(Response::Failure(r)) => Err(RpcError {
                error: Error::new(format!(
                    "external host error: {}",
                    crate::util::display_safe(
                        &r.error.message.chars().take(512).collect::<String>()
                    )
                )),
                dispatched,
                not_created: r.error.code == ErrorCode::NotCreated,
            }),
            Err(error) => {
                self.abort();
                Err(RpcError {
                    error,
                    dispatched,
                    not_created: false,
                })
            }
        }
    }
}

pub struct ExternalHost {
    process: RefCell<Process>,
    hello: Hello,
    persistence_key: String,
}
impl ExternalHost {
    /// Another thread may set this token to cancel the current/next request.
    /// Cancellation permanently poisons the connection; there is no retry.
    pub fn cancellation_token(&self) -> Arc<AtomicBool> {
        self.process.borrow().cancelled.clone()
    }
    pub fn cancel(&self) {
        let mut process = self.process.borrow_mut();
        process.cancelled.store(true, Ordering::Relaxed);
        process.abort();
    }
    pub fn connect(status: &PluginStatus) -> Result<Self> {
        let repo = crate::git::discover(&std::env::current_dir()?)?;
        let (verified, frozen) = super::binding::select(&repo, &status.id)?;
        if verified.manifest != status.manifest || verified.executable != status.executable {
            return Err(Error::new("selected session host changed since discovery"));
        }
        Self::connect_bound(&verified, &frozen)
    }
    pub(crate) fn connect_bound(
        status: &PluginStatus,
        frozen: &super::binding::FrozenHost,
    ) -> Result<Self> {
        let spec = status.executable.as_ref().ok_or_else(|| {
            Error::new(
                status
                    .activation_blocker
                    .unwrap_or("package has no executable"),
            )
            .with_kind(ErrorKind::Prerequisite)
        })?;
        spec.validate()?;
        let mut source = super::plugins::local::executable_file(status)?;
        let base = std::env::temp_dir().canonicalize()?;
        validate_snapshot_base(&base)?;
        let private_dir = std::fs::Permissions::from_mode(0o700);
        let snapshot = tempfile::Builder::new()
            .prefix("ahu-host-")
            .permissions(private_dir)
            .tempdir_in(base)?;
        let executable = snapshot.path().join("host");
        let bytes = crate::util::read_limited(&mut source, 64 * 1024 * 1024)?;
        if crate::util::digest_bytes(&bytes) != frozen.executable_sha256 {
            return Err(Error::new(
                "session host executable changed before activation",
            ));
        }
        let mut file = File::options()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&executable)?;
        file.write_all(&bytes)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o500))?;
        drop(file);
        let mut child = Command::new(&executable)
            .arg("--ahu-host-protocol=1")
            .current_dir(snapshot.path())
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|_| {
                Error::new("cannot start the selected external host executable")
                    .with_kind(ErrorKind::Prerequisite)
            })?;
        let input = child.stdin.take().expect("piped stdin");
        let output = child.stdout.take().expect("piped stdout");
        let mut process = Process {
            child,
            input,
            output,
            _snapshot: snapshot,
            timeout: Duration::from_millis(spec.timeout_ms),
            next_id: 1,
            broken: false,
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        nonblocking(process.input.as_raw_fd())?;
        nonblocking(process.output.as_raw_fd())?;
        let value = process.rpc("hello", json!({"protocol_versions":[1], "host_api":1, "plugin_id":status.id, "required_capabilities":protocol::METHODS}))?;
        let hello: Hello =
            serde_json::from_value(value).map_err(|_| Error::new("invalid external host hello"))?;
        let capabilities: BTreeSet<_> = hello.capabilities.iter().map(String::as_str).collect();
        if hello.protocol_version != protocol::VERSION
            || hello.host_api != 1
            || hello.plugin_id != status.id
            || capabilities.len() != hello.capabilities.len()
            || !protocol::METHODS.iter().all(|m| capabilities.contains(m))
            || hello
                .inherited_workspace
                .as_deref()
                .is_some_and(|id| !valid_id(id))
        {
            return Err(Error::new(
                "external host protocol, identity, or required capabilities mismatch",
            )
            .with_kind(ErrorKind::Prerequisite));
        }
        Ok(Self {
            process: RefCell::new(process),
            hello,
            persistence_key: frozen.key()?,
        })
    }
    fn call<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T> {
        let value = self.process.borrow_mut().rpc(method, params)?;
        serde_json::from_value(value).map_err(|_| {
            self.process.borrow_mut().abort();
            Error::new("external host returned an invalid method result; outcome may be unknown")
        })
    }
}

fn validate_snapshot_base(base: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    for path in base.ancestors() {
        match std::fs::symlink_metadata(path.join(".git")) {
            Ok(_) => {
                return Err(Error::new(
                    "external host executable snapshots must be outside repositories",
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Error::new("cannot verify external host snapshot storage")),
        }
        let metadata = std::fs::metadata(path)?;
        // SAFETY: geteuid has no preconditions.
        if !metadata.is_dir()
            || (metadata.uid() != 0 && metadata.uid() != unsafe { libc::geteuid() })
            || (metadata.mode() & 0o022 != 0 && metadata.mode() & 0o1000 == 0)
        {
            return Err(Error::new("unsafe external host snapshot storage"));
        }
    }
    Ok(())
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceInventory {
    complete: bool,
    workspaces: BTreeMap<String, WorkspaceInfo>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupInventory {
    complete: bool,
    groups: Vec<Group>,
}
fn check_group(group: &Group) -> Result<()> {
    let unique: BTreeSet<_> = group.member_workspace_ids.iter().collect();
    if !valid_id(&group.id)
        || !valid_id(&group.anchor_workspace_id)
        || group.member_workspace_ids.iter().any(|id| !valid_id(id))
        || unique.len() != group.member_workspace_ids.len()
        || !group
            .member_workspace_ids
            .contains(&group.anchor_workspace_id)
    {
        return Err(Error::new("external host returned invalid group ownership"));
    }
    Ok(())
}
fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .filter(|_| path.is_absolute())
        .ok_or_else(|| Error::new("external host paths must be absolute UTF-8 paths"))
}
impl Host for ExternalHost {
    fn persistence_key(&self) -> Option<String> {
        Some(self.persistence_key.clone())
    }
    fn inherited_workspace(&self) -> Option<String> {
        self.hello.inherited_workspace.clone()
    }
    fn check_capabilities(&self) -> Result<Vec<String>> {
        if self.process.borrow().broken {
            return Err(Error::new("external host connection is unusable"));
        }
        Ok(self.hello.capabilities.clone())
    }
    fn list_groups(&self, window_id: Option<&str>) -> Result<Vec<Group>> {
        let result: GroupInventory = self.call("list_groups", json!({"window_id":window_id}))?;
        if !result.complete {
            return Err(Error::new("external host group inventory is incomplete"));
        }
        let mut unique = BTreeSet::new();
        for group in &result.groups {
            check_group(group)?;
            if !unique.insert(&group.id) {
                return Err(Error::new("external host returned duplicate groups"));
            }
        }
        Ok(result.groups)
    }
    fn find_group(&self, group_id: &str, window_id: Option<&str>) -> Result<Option<Group>> {
        let result: Option<Group> = self.call(
            "find_group",
            json!({"group_id":group_id,"window_id":window_id}),
        )?;
        if let Some(group) = &result {
            check_group(group)?;
            if group.id != group_id {
                return Err(Error::new("external host returned a different group"));
            }
        }
        Ok(result)
    }
    fn current_window(&self) -> Result<Option<String>> {
        self.call("current_window", json!({}))
    }
    fn window_for_workspace(&self, workspace_id: &str) -> Result<String> {
        self.call("window_for_workspace", json!({"workspace_id":workspace_id}))
    }
    fn add_workspace_to_group(
        &self,
        group_id: &str,
        workspace_id: &str,
        window_id: &str,
    ) -> Result<()> {
        self.call(
            "add_workspace_to_group",
            json!({"group_id":group_id,"workspace_id":workspace_id,"window_id":window_id}),
        )
    }
    fn current_workspace(&self) -> Result<Option<String>> {
        self.call("current_workspace", json!({}))
    }
    fn create_anchor_workspace(&self, group_id: &str, title: &str, cwd: &Path) -> Result<String> {
        self.call(
            "create_anchor_workspace",
            json!({"group_id":group_id,"title":title,"cwd":path_text(cwd)?}),
        )
    }
    fn set_anchor(&self, group_id: &str, workspace_id: &str) -> Result<()> {
        self.call(
            "set_anchor",
            json!({"group_id":group_id,"workspace_id":workspace_id}),
        )
    }
    fn create_group(&self, name: &str, cwd: &Path) -> Result<Group> {
        let group = self.call("create_group", json!({"name":name,"cwd":path_text(cwd)?}))?;
        check_group(&group)?;
        Ok(group)
    }
    fn expand_group(&self, group_id: &str) -> Result<()> {
        self.call("expand_group", json!({"group_id":group_id}))
    }
    fn create_task_workspace(
        &self,
        request: &WorkspaceRequest<'_>,
    ) -> WorkspaceCreationResult<CreatedWorkspace> {
        let cwd = path_text(request.cwd).map_err(WorkspaceCreationError::NotCreated)?;
        let result = self.process.borrow_mut().rpc("create_task_workspace", json!({
            "group_id":request.group_id,"window_id":request.window_id,"title":request.title,
            "summary":request.summary,"cwd":cwd,"startup_command":request.startup_command,"focus":request.focus
        })).map_err(|error| {
            if !error.dispatched || error.not_created { WorkspaceCreationError::NotCreated(error.error) }
            else { WorkspaceCreationError::OutcomeUnknown(error.error) }
        })?;
        // A success envelope is not enough: validate ownership and exact cwd
        // through a complete inventory before accepting the created resource.
        let verify = || -> Result<CreatedWorkspace> {
            let created: CreatedWorkspace = serde_json::from_value(result)
                .map_err(|_| Error::new("external host returned an invalid creation result"))?;
            if !valid_id(&created.workspace_id)
                || !valid_id(&created.window_id)
                || created.group_id.as_deref() != Some(request.group_id)
                || request.window_id.is_some_and(|w| w != created.window_id)
            {
                return Err(Error::new(
                    "external host returned unexpected workspace ownership",
                ));
            }
            let inventory = self.workspaces_in_window(Some(&created.window_id))?;
            if inventory
                .get(&created.workspace_id)
                .is_none_or(|w| w.directory != cwd)
            {
                return Err(Error::new(
                    "external host creation has no matching workspace directory",
                ));
            }
            if self.window_for_workspace(&created.workspace_id)? != created.window_id
                || !self
                    .find_group(request.group_id, Some(&created.window_id))?
                    .is_some_and(|g| g.member_workspace_ids.contains(&created.workspace_id))
            {
                return Err(Error::new(
                    "external host creation ownership could not be confirmed",
                ));
            }
            Ok(created)
        };
        verify().map_err(WorkspaceCreationError::OutcomeUnknown)
    }
    fn open_markdown(&self, path: &Path, focus: bool) -> Result<String> {
        self.call(
            "open_markdown",
            json!({"path":path_text(path)?,"focus":focus}),
        )
    }
    fn select_workspace(&self, workspace_id: &str) -> Result<()> {
        self.call("select_workspace", json!({"workspace_id":workspace_id}))
    }
    fn close_workspace(&self, workspace_id: &str) -> Result<()> {
        self.call("close_workspace", json!({"workspace_id":workspace_id}))
    }
    fn set_status(&self, workspace_id: &str, value: &str) -> Result<()> {
        self.call(
            "set_status",
            json!({"workspace_id":workspace_id,"value":value}),
        )
    }
    fn set_agent_metadata(
        &self,
        workspace_id: &str,
        agent: &str,
        harness: &str,
        model: &str,
    ) -> Result<()> {
        self.call(
            "set_agent_metadata",
            json!({"workspace_id":workspace_id,"agent":agent,"harness":harness,"model":model}),
        )
    }
    fn workspaces(&self) -> Result<BTreeMap<String, WorkspaceInfo>> {
        self.inventory("workspaces", json!({}))
    }
    fn workspaces_in_window(
        &self,
        window: Option<&str>,
    ) -> Result<BTreeMap<String, WorkspaceInfo>> {
        self.inventory("workspaces_in_window", json!({"window_id":window}))
    }
}
impl ExternalHost {
    fn inventory(&self, method: &str, params: Value) -> Result<BTreeMap<String, WorkspaceInfo>> {
        let result: WorkspaceInventory = self.call(method, params)?;
        if !result.complete
            || result
                .workspaces
                .iter()
                .any(|(id, w)| !valid_id(id) || !Path::new(&w.directory).is_absolute())
        {
            return Err(Error::new(
                "external host workspace inventory is incomplete or invalid",
            ));
        }
        Ok(result.workspaces)
    }
}

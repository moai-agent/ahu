//! Host-authorized, one-shot executable plugins. Inventory/install never call this API.
//! This transport bounds communication and lifetime; it is not an OS sandbox.
use crate::{git::Repo, interactive::plugins::local, plugin_binding::Grant};
use ahu_host_api::protocol::from_slice;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::{fs::PermissionsExt, process::CommandExt};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub mod cli;
pub mod task_service;

pub const VERSION: u32 = 1;
pub const MAX_BYTES: usize = 64 * 1024;
pub const MAX_CONTEXT_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entrypoint {
    pub path: String,
    pub protocol_version: u32,
    pub timeout_ms: u64,
}
impl Entrypoint {
    pub(crate) fn validate(&self) -> crate::util::Result<()> {
        if self.protocol_version != VERSION || !(100..=30_000).contains(&self.timeout_ms) {
            return Err(crate::util::Error::new(
                "entrypoint requires protocol_version = 1 and timeout_ms between 100 and 30000",
            ));
        }
        Ok(())
    }
}

/// Construct only in trusted host code, never from plugin requests. The grant
/// binds one repository and plugin. Context admits only an opaque invocation ID,
/// never credential/path fields or arbitrary metadata. The trusted host must not
/// encode secrets into that ID or the separately supplied input payload.
/// No TaskService handle or ambient context is added.
pub struct HostGrant {
    repository: String,
    plugin: String,
    context: InvocationContext,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InvocationContext {
    invocation_id: String,
}

impl HostGrant {
    pub fn new(repo: &Repo, plugin: &str, context_json: &[u8]) -> Result<Self> {
        if plugin.is_empty()
            || plugin.len() > 64
            || !plugin
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || context_json.len() > MAX_CONTEXT_BYTES
        {
            return Err(Failure::before(Code::InvalidInput));
        }
        let context: InvocationContext =
            from_slice(context_json).map_err(|_| Failure::before(Code::InvalidInput))?;
        if context.invocation_id.is_empty()
            || context.invocation_id.len() > 64
            || !context
                .invocation_id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
        {
            return Err(Failure::before(Code::InvalidInput));
        }
        Ok(Self {
            repository: repo.identity(),
            plugin: plugin.into(),
            context,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    Denied,
    InvalidInput,
    Unavailable,
    Cancelled,
    TimedOut,
    OutputLimit,
    Protocol,
    Failed,
    Io,
}
/// Fixed diagnostics never include child output, manifest text, paths or OS errors.
/// After spawn, any failure leaves effects unknown and must not be auto-retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure {
    pub code: Code,
    pub outcome_may_be_unknown: bool,
}
impl Failure {
    fn before(code: Code) -> Self {
        Self {
            code,
            outcome_may_be_unknown: false,
        }
    }
    fn after(code: Code) -> Self {
        Self {
            code,
            outcome_may_be_unknown: true,
        }
    }
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "plugin invocation: {:?}; outcome may be unknown: {}",
            self.code, self.outcome_may_be_unknown
        )
    }
}
impl std::error::Error for Failure {}
pub type Result<T> = std::result::Result<T, Failure>;

#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum Response {
    Ok {
        protocol_version: u32,
        result: Value,
    },
    Error {
        protocol_version: u32,
        code: RemoteError,
    },
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RemoteError {
    Failed,
    InvalidInput,
    Unsupported,
}

/// Invoke an installed package once, using current committed enablement and an
/// `execute` grant as well as a trusted host grant. No discovery-path override,
/// automatic install, handshake, auth lookup, network client or task callback.
pub fn invoke(
    repo: &Repo,
    grant: &HostGrant,
    input_json: &[u8],
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
    if repo.identity() != grant.repository {
        return Err(Failure::before(Code::Denied));
    }
    if input_json.len() > MAX_BYTES {
        return Err(Failure::before(Code::InvalidInput));
    }
    let input: Value = from_slice(input_json).map_err(|_| Failure::before(Code::InvalidInput))?;
    let request = serde_json::to_vec(&json!({
        "protocol_version": VERSION, "plugin_id": grant.plugin,
        "grant": {"operation": "invoke"}, "context": grant.context, "input": input,
    }))
    .map_err(|_| Failure::before(Code::InvalidInput))?;
    if request.len() > MAX_BYTES {
        return Err(Failure::before(Code::InvalidInput));
    }
    // Authorize before touching the package, executable or any provider state.
    let binding = authorized_binding(repo, grant)?;
    let prepared = (|| -> crate::util::Result<_> {
        let store = crate::state::checkout_root(&repo.primary_root()?)?.join("plugins");
        let installation = crate::plugin_install::installed(&store, &binding)?;
        crate::plugin_install::verify_installed(&installation, &binding)?;
        let status = local::read_package(&installation.join("package"))?;
        let spec = status
            .entrypoint
            .as_ref()
            .ok_or_else(|| crate::util::Error::new("no entrypoint"))?;
        let mut source = local::entrypoint_file(&status)?;
        let bytes = crate::util::read_limited(&mut source, 64 * 1024 * 1024)?;
        let base = std::env::temp_dir().canonicalize()?;
        crate::interactive::external::validate_snapshot_base(&base)?;
        let snapshot = tempfile::Builder::new()
            .prefix("ahu-plugin-")
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir_in(base)?;
        let executable = snapshot.path().join("entrypoint");
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::File::options()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&executable)?;
        file.write_all(&bytes)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o500))?;
        drop(file);
        Ok((snapshot, executable, Duration::from_millis(spec.timeout_ms)))
    })()
    .map_err(|_| Failure::before(Code::Unavailable))?;
    let (snapshot, executable, timeout) = prepared;
    check_cancel()?;
    let deadline = Instant::now() + timeout;
    let child = Command::new(executable)
        .arg("--ahu-plugin-protocol=1")
        .current_dir(snapshot.path())
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|_| Failure::before(Code::Unavailable))?;
    let mut process = Process { child: Some(child) };
    let output = exchange(&mut process, &request, deadline, cancelled)?;
    let response: Response = from_slice(&output).map_err(|_| Failure::after(Code::Protocol))?;
    match response {
        Response::Ok {
            protocol_version: VERSION,
            result,
        } => Ok(result),
        Response::Error {
            protocol_version: VERSION,
            code,
        } => {
            let _ = code; // Remote errors cannot dictate diagnostics or retry policy.
            Err(Failure::after(Code::Failed))
        }
        _ => Err(Failure::after(Code::Protocol)),
    }
}

// Shared committed policy gate. Task observations are collected only after this
// check; invoke checks it again immediately before preparing the executable.
fn authorized_binding(repo: &Repo, grant: &HostGrant) -> Result<crate::plugin_binding::Binding> {
    if repo.identity() != grant.repository {
        return Err(Failure::before(Code::Denied));
    }
    let binding = crate::interactive::plugins::project::inventory(&repo.root)
        .map_err(|_| Failure::before(Code::Unavailable))?
        .bindings
        .into_iter()
        .find(|s| s.binding.id == grant.plugin)
        .ok_or_else(|| Failure::before(Code::Denied))?
        .binding;
    if !binding.enabled
        || !binding.grants.contains(&Grant::Execute)
        || binding.compatibility.ahu != env!("CARGO_PKG_VERSION")
    {
        return Err(Failure::before(Code::Denied));
    }
    Ok(binding)
}

// Cleanup before reaping prevents group signalling after PID reuse. No records
// contain process authority. Escaped descendants remain outside this guarantee.
struct Process {
    child: Option<Child>,
}
impl Process {
    fn finish(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let child = self.child.as_mut().expect("owned child");
        crate::headless::signal_group(child.id(), libc::SIGKILL);
        let status = child.wait()?;
        self.child = None;
        Ok(status)
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            crate::headless::signal_group(child.id(), libc::SIGKILL);
            let _ = child.kill();
            // Bounded cleanup, including a child stuck in an uninterruptible syscall.
            let until = Instant::now() + Duration::from_millis(250);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) | Err(_) => break,
                    _ if Instant::now() >= until => break,
                    _ => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        }
    }
}
fn nonblocking(fd: i32) -> Result<()> {
    // SAFETY: this is a live pipe owned by this invocation.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(Failure::after(Code::Io));
    }
    Ok(())
}
fn retry(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
    )
}
fn exchange(
    process: &mut Process,
    request: &[u8],
    deadline: Instant,
    cancelled: &AtomicBool,
) -> Result<Vec<u8>> {
    let child = process.child.as_mut().expect("owned child");
    let pid = child.id();
    let mut input = child.stdin.take();
    let mut output = child.stdout.take().expect("piped stdout");
    nonblocking(input.as_ref().expect("piped stdin").as_raw_fd())?;
    nonblocking(output.as_raw_fd())?;
    let mut sent = 0;
    let mut bytes = Vec::new();
    let mut eof = false;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Failure::after(Code::Cancelled));
        }
        if Instant::now() >= deadline {
            return Err(Failure::after(Code::TimedOut));
        }
        if let Some(pipe) = &mut input {
            match pipe.write(&request[sent..]) {
                Ok(0) => return Err(Failure::after(Code::Io)),
                Ok(n) => sent += n,
                Err(e) if retry(&e) => {}
                Err(_) => return Err(Failure::after(Code::Io)),
            }
            if sent == request.len() {
                input = None;
            }
        }
        // One bounded read per iteration: even endless output checks cancellation.
        if !eof {
            let mut chunk = [0; 4096];
            match output.read(&mut chunk) {
                Ok(0) => eof = true,
                Ok(n) => {
                    if bytes.len() + n > MAX_BYTES {
                        return Err(Failure::after(Code::OutputLimit));
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                }
                Err(e) if retry(&e) => {}
                Err(_) => return Err(Failure::after(Code::Io)),
            }
        }
        if eof && crate::headless::child_exited(pid).map_err(|_| Failure::after(Code::Io))? {
            let status = process.finish().map_err(|_| Failure::after(Code::Io))?;
            if !status.success() || sent != request.len() {
                return Err(Failure::after(Code::Failed));
            }
            return Ok(bytes);
        }
        let mut polls = [
            libc::pollfd {
                fd: if eof { -1 } else { output.as_raw_fd() },
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: input.as_ref().map_or(-1, AsRawFd::as_raw_fd),
                events: libc::POLLOUT,
                revents: 0,
            },
        ];
        let millis = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .clamp(1, 20) as i32;
        // SAFETY: polls is a live array of two valid descriptors (or ignored -1).
        let ready = unsafe { libc::poll(polls.as_mut_ptr(), 2, millis) };
        if ready < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return Err(Failure::after(Code::Io));
        }
    }
}

//! Foreground operator transport. All runtime failures cross a fixed JSON boundary.
use super::{Code, Failure, HostGrant, MAX_BYTES, Result};
use std::{
    collections::BTreeSet,
    fs::File,
    io::IsTerminal,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const IO_TIMEOUT: Duration = Duration::from_secs(30);
static CANCELLED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub id: String,
    pub input_file: PathBuf,
    pub mode: Mode,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Json,
    TaskObservations(BTreeSet<String>),
}

pub fn parse(args: &[String]) -> crate::util::Result<Options> {
    let invalid = || {
        crate::util::Error::new(
        "expected ahu plugins invoke ID --input-file PATH|- [--task-observations [--allow-task UUID ...]]",
    ).with_kind(crate::util::ErrorKind::Usage)
    };
    let id = args
        .first()
        .filter(|id| {
            !id.is_empty()
                && id.len() <= 64
                && id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .ok_or_else(invalid)?;
    let mut input_file = None;
    let mut task_mode = false;
    let mut tasks = BTreeSet::new();
    let mut flags = args[1..].iter();
    while let Some(flag) = flags.next() {
        match flag.as_str() {
            "--input-file" if input_file.is_none() => {
                input_file = Some(PathBuf::from(
                    flags.next().filter(|v| !v.is_empty()).ok_or_else(invalid)?,
                ));
            }
            "--task-observations" if !task_mode => task_mode = true,
            "--allow-task" => {
                let id = flags.next().ok_or_else(invalid)?;
                if !crate::task::is_canonical_task_uuid(id)
                    || !tasks.insert(id.clone())
                    || tasks.len() > super::task_service::MAX_TASKS
                {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
    }
    if !task_mode && !tasks.is_empty() {
        return Err(invalid());
    }
    Ok(Options {
        id: id.clone(),
        input_file: input_file.ok_or_else(invalid)?,
        mode: if task_mode {
            Mode::TaskObservations(tasks)
        } else {
            Mode::Json
        },
    })
}

/// One call per CLI process; owns signal dispositions until output is delivered.
pub fn run(options: Options, repository: Option<&Path>) -> i32 {
    let signals = Signals::install();
    let result = signals
        .as_ref()
        .map_err(|_| Failure::before(Code::Io))
        .and_then(|_| execute(options, repository));
    let result = result.and_then(|value| {
        let output = success_document(value)?;
        write_fd(libc::STDOUT_FILENO, &output, true).map_err(Failure::after)
    });
    match result {
        Ok(()) => 0,
        Err(failure) => {
            let (code, exit) = match failure.code {
                Code::InvalidInput => ("invalid_input", 2),
                Code::Denied => ("denied", 4),
                Code::Unavailable => ("unavailable", 4),
                Code::Cancelled => ("cancelled", 5),
                Code::TimedOut => ("timed_out", 5),
                Code::OutputLimit => ("output_limit", 5),
                Code::Protocol => ("protocol", 5),
                Code::Failed => ("failed", 5),
                Code::Io => ("io", 5),
            };
            let output = format!(
                "{{\"schema_version\":1,\"status\":\"error\",\"code\":\"{code}\",\"outcome_may_be_unknown\":{}}}\n",
                failure.outcome_may_be_unknown
            );
            let _ = write_fd(libc::STDERR_FILENO, output.as_bytes(), false);
            exit
        }
    }
}

fn execute(options: Options, repository: Option<&Path>) -> Result<serde_json::Value> {
    crate::util::require_host_operator("plugin invocations")
        .map_err(|_| Failure::before(Code::Denied))?;
    if let Some(path) = repository {
        std::env::set_current_dir(path).map_err(|_| Failure::before(Code::Unavailable))?;
    }
    let repo = crate::commands::repo_from_cwd().map_err(|_| Failure::before(Code::Unavailable))?;
    initialize_telemetry(&repo);
    let mode = match &options.mode {
        Mode::Json => "json",
        Mode::TaskObservations(_) => "task_observations",
    };
    let mut span = crate::telemetry::span(
        "ahu.plugin.invoke",
        [
            ("ahu.plugin.id", options.id.clone()),
            ("ahu.plugin.mode", mode.to_string()),
        ],
    );
    let result = (|| {
        let invocation_id = crate::task::new_task_id().map_err(|_| Failure::before(Code::Io))?;
        let context = serde_json::json!({"invocation_id": invocation_id}).to_string();
        let grant = HostGrant::new(&repo, &options.id, context.as_bytes())?;
        super::authorized_binding(&repo, &grant)?;
        let input = read_input(&options.input_file)?;
        match options.mode {
            Mode::Json => super::invoke(&repo, &grant, &input, &CANCELLED),
            Mode::TaskObservations(tasks) => {
                super::task_service::invoke_inspected(&repo, grant, tasks, &input, &CANCELLED)
            }
        }
    })();
    match &result {
        Ok(_) => span.set_string("ahu.plugin.outcome", "succeeded"),
        Err(failure) => {
            span.set_string("ahu.plugin.outcome", failure_code(failure.code));
            span.set_bool(
                "ahu.plugin.outcome_may_be_unknown",
                failure.outcome_may_be_unknown,
            );
        }
    }
    result
}

fn initialize_telemetry(repo: &crate::git::Repo) {
    let Ok(root) = repo.primary_root() else {
        return;
    };
    let Ok(Some(config)) = crate::config::load(&root) else {
        return;
    };
    // Telemetry is optional and must never prevent an authorized plugin call.
    let _ = crate::telemetry::initialize(&config.config.telemetry);
}

fn failure_code(code: Code) -> &'static str {
    match code {
        Code::Denied => "denied",
        Code::InvalidInput => "invalid_input",
        Code::Unavailable => "unavailable",
        Code::Cancelled => "cancelled",
        Code::TimedOut => "timed_out",
        Code::OutputLimit => "output_limit",
        Code::Protocol => "protocol",
        Code::Failed => "failed",
        Code::Io => "io",
    }
}

fn check(deadline: Instant, cancel: bool) -> std::result::Result<(), Code> {
    if cancel && CANCELLED.load(Ordering::Relaxed) {
        return Err(Code::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(Code::TimedOut);
    }
    Ok(())
}

fn read_input(path: &Path) -> Result<Vec<u8>> {
    let deadline = Instant::now() + IO_TIMEOUT;
    check(deadline, true).map_err(Failure::before)?;
    let file;
    let fd = if path == Path::new("-") {
        if std::io::stdin().is_terminal() {
            return Err(Failure::before(Code::InvalidInput));
        }
        libc::STDIN_FILENO
    } else {
        file = File::options()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| Failure::before(Code::InvalidInput))?;
        let meta = file
            .metadata()
            .map_err(|_| Failure::before(Code::InvalidInput))?;
        if !meta.is_file() || meta.len() > MAX_BYTES as u64 {
            return Err(Failure::before(Code::InvalidInput));
        }
        file.as_raw_fd()
    };
    let _flags = Nonblocking::new(fd).map_err(Failure::before)?;
    let mut bytes = Vec::new();
    loop {
        check(deadline, true).map_err(Failure::before)?;
        wait_fd(fd, libc::POLLIN).map_err(Failure::before)?;
        let mut buffer = [0u8; 4096];
        let count = buffer.len().min(MAX_BYTES + 1 - bytes.len());
        // SAFETY: valid writable buffer and live borrowed descriptor.
        let n = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), count) };
        if n == 0 {
            break;
        }
        if n < 0 {
            if retry_io() {
                continue;
            }
            return Err(Failure::before(Code::Io));
        }
        bytes.extend_from_slice(&buffer[..n as usize]);
        if bytes.len() > MAX_BYTES {
            return Err(Failure::before(Code::InvalidInput));
        }
    }
    check(deadline, true).map_err(Failure::before)?;
    Ok(bytes)
}

fn success_document(value: serde_json::Value) -> Result<Vec<u8>> {
    let raw = serde_json::to_string(
        &serde_json::json!({"schema_version":1,"status":"ok","result":value}),
    )
    .map_err(|_| Failure::after(Code::Io))?;
    let mut output = Vec::new();
    // JSON-preserving ASCII escaping also covers C1, bidi and line controls.
    for character in raw.chars() {
        if character.is_ascii() && character != '\u{7f}' {
            output.push(character as u8);
        } else {
            for unit in character.encode_utf16(&mut [0; 2]) {
                output.extend_from_slice(format!("\\u{unit:04x}").as_bytes());
            }
        }
        if output.len() >= MAX_BYTES {
            return Err(Failure::after(Code::OutputLimit));
        }
    }
    output.push(b'\n');
    Ok(output)
}

fn write_fd(fd: i32, mut bytes: &[u8], cancel: bool) -> std::result::Result<(), Code> {
    let deadline = Instant::now() + IO_TIMEOUT;
    let _flags = Nonblocking::new(fd)?;
    while !bytes.is_empty() {
        check(deadline, cancel)?;
        wait_fd(fd, libc::POLLOUT)?;
        // SAFETY: valid readable buffer and borrowed stdout/stderr descriptor.
        let n = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if n < 0 && retry_io() {
            continue;
        }
        if n <= 0 {
            return Err(Code::Io);
        }
        bytes = &bytes[n as usize..];
    }
    Ok(())
}
fn retry_io() -> bool {
    matches!(
        std::io::Error::last_os_error().kind(),
        std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
    )
}
fn wait_fd(fd: i32, events: i16) -> std::result::Result<(), Code> {
    let mut poll = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    // SAFETY: one initialized pollfd, fixed short wait. Actual I/O is nonblocking.
    if unsafe { libc::poll(&mut poll, 1, 20) } < 0 && !retry_io() {
        return Err(Code::Io);
    }
    Ok(())
}
struct Nonblocking {
    fd: i32,
    flags: i32,
}
impl Nonblocking {
    fn new(fd: i32) -> std::result::Result<Self, Code> {
        // SAFETY: fcntl inspects/updates a borrowed descriptor; restore on drop.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return Err(Code::Io);
            }
            Ok(Self { fd, flags })
        }
    }
}
impl Drop for Nonblocking {
    fn drop(&mut self) {
        // SAFETY: descriptor is still owned by its enclosing scope.
        unsafe {
            libc::fcntl(self.fd, libc::F_SETFL, self.flags);
        }
    }
}
extern "C" fn cancel(_: libc::c_int) {
    CANCELLED.store(true, Ordering::Relaxed);
}
struct Signals(libc::sigaction, libc::sigaction, libc::sigaction);
impl Signals {
    fn install() -> std::io::Result<Self> {
        CANCELLED.store(false, Ordering::Relaxed);
        // SAFETY: initialized structures; signal handler only stores an atomic.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            let mut term = std::mem::zeroed();
            let mut interrupt = std::mem::zeroed();
            let mut pipe = std::mem::zeroed();
            action.sa_sigaction = cancel as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            if libc::sigaction(libc::SIGTERM, &action, &mut term) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::sigaction(libc::SIGINT, &action, &mut interrupt) != 0 {
                let error = std::io::Error::last_os_error();
                libc::sigaction(libc::SIGTERM, &term, std::ptr::null_mut());
                return Err(error);
            }
            // Do not rely on the embedding runtime's SIGPIPE disposition:
            // closed stdout/stderr must return EPIPE for the fixed Io mapping.
            action.sa_sigaction = libc::SIG_IGN;
            if libc::sigaction(libc::SIGPIPE, &action, &mut pipe) != 0 {
                let error = std::io::Error::last_os_error();
                libc::sigaction(libc::SIGTERM, &term, std::ptr::null_mut());
                libc::sigaction(libc::SIGINT, &interrupt, std::ptr::null_mut());
                return Err(error);
            }
            Ok(Self(term, interrupt, pipe))
        }
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        // SAFETY: restore saved dispositions on every exit.
        unsafe {
            libc::sigaction(libc::SIGTERM, &self.0, std::ptr::null_mut());
            libc::sigaction(libc::SIGINT, &self.1, std::ptr::null_mut());
            libc::sigaction(libc::SIGPIPE, &self.2, std::ptr::null_mut());
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sigpipe_is_suppressed_and_prior_disposition_is_restored() {
        const CHILD: &str = "AHU_TEST_PLUGIN_SIGPIPE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "plugin_exec::cli::tests::sigpipe_is_suppressed_and_prior_disposition_is_restored", "--test-threads=1"])
                .env(CHILD, "1")
                .output().unwrap();
            assert!(
                output.status.success(),
                "isolated SIGPIPE test failed: {:?}",
                output.status
            );
            return;
        }
        // SAFETY: only the isolated child modifies process-wide dispositions.
        unsafe {
            let mut default: libc::sigaction = std::mem::zeroed();
            default.sa_sigaction = libc::SIG_DFL;
            libc::sigemptyset(&mut default.sa_mask);
            assert_eq!(
                libc::sigaction(libc::SIGPIPE, &default, std::ptr::null_mut()),
                0
            );
            let guard = super::Signals::install().unwrap();
            assert_eq!(libc::raise(libc::SIGPIPE), 0); // Default would kill this child.
            drop(guard);
            let mut restored: libc::sigaction = std::mem::zeroed();
            assert_eq!(
                libc::sigaction(libc::SIGPIPE, std::ptr::null(), &mut restored),
                0
            );
            assert_eq!(restored.sa_sigaction, libc::SIG_DFL);
        }
    }
}

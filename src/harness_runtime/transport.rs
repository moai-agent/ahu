use super::{Operation, Prepared};
use crate::util::{Error, Result};
use ahu_harness_api::{MAX_FRAME_BYTES, PROTOCOL_VERSION, RemoteError, Request, Response};
use serde_json::Value;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// An owned persistent peer. Failure poisons and terminates it; callers cannot
/// replay an uncertain operation through this session. Drop kills the owned
/// group before reaping its leader, avoiding signals after PID reuse.
pub struct Session {
    prepared: Arc<Prepared>,
    child: Option<Child>,
    input: Option<ChildStdin>,
    output: Option<ChildStdout>,
    next_id: u64,
    decoder: Decoder,
}
#[derive(PartialEq, Eq)]
enum Decoder {
    Ready,
    Active,
    Finished,
}
fn failure(code: &str) -> Error {
    Error::new(format!(
        "harness runtime {code}; effects may be unknown; no automatic retry"
    ))
}
impl Session {
    pub(super) fn open(prepared: Arc<Prepared>) -> Result<Self> {
        let child = Command::new(&prepared.executable)
            .arg("--ahu-harness-runtime-protocol=1")
            .current_dir(prepared.snapshot.path())
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|_| Error::new("cannot start installed harness runtime"))?;
        let mut session = Self {
            prepared,
            child: Some(child),
            input: None,
            output: None,
            next_id: 1,
            decoder: Decoder::Ready,
        };
        let child = session
            .child
            .as_mut()
            .ok_or_else(|| failure("unavailable"))?;
        session.input = child.stdin.take();
        session.output = child.stdout.take();
        nonblocking(
            session
                .input
                .as_ref()
                .ok_or_else(|| failure("stdin unavailable"))?
                .as_raw_fd(),
        )?;
        nonblocking(
            session
                .output
                .as_ref()
                .ok_or_else(|| failure("stdout unavailable"))?
                .as_raw_fd(),
        )?;
        Ok(session)
    }
    pub fn call(&mut self, operation: &str, params: Value) -> Result<Value> {
        self.call_cancellable(operation, params, &AtomicBool::new(false))
    }
    /// Cancellation is checked while writing and reading. The flag is host-owned;
    /// cancellation terminates the peer and leaves dispatched effects unknown.
    pub fn call_cancellable(
        &mut self,
        operation: &str,
        params: Value,
        cancelled: &AtomicBool,
    ) -> Result<Value> {
        let result = self.exchange(operation, params, cancelled);
        if result.is_err() {
            let _ = self.terminate();
        }
        result
    }
    fn exchange(
        &mut self,
        operation: &str,
        params: Value,
        cancelled: &AtomicBool,
    ) -> Result<Value> {
        let operation = self.prepared.operation(operation)?;
        if (operation == Operation::DecodeStart && self.decoder != Decoder::Ready)
            || (matches!(operation, Operation::DecodeEvent | Operation::DecodeFinish)
                && self.decoder != Decoder::Active)
        {
            return Err(failure("invalid decoder sequence"));
        }
        let id = self.next_id;
        self.next_id = id
            .checked_add(1)
            .ok_or_else(|| failure("sequence exhausted"))?;
        let mut encoded = FrameWriter(Vec::new());
        serde_json::to_writer(
            &mut encoded,
            &Request {
                protocol_version: PROTOCOL_VERSION,
                id,
                operation,
                params,
            },
        )
        .map_err(|_| failure("request exceeds frame limit or is invalid"))?;
        let mut request = encoded.0;
        request.push(b'\n');
        let deadline = Instant::now() + self.prepared.timeout;
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| failure("session closed"))?;
        let output = self
            .output
            .as_mut()
            .ok_or_else(|| failure("session closed"))?;
        // Refuse queued unsolicited output before dispatching the next request.
        let mut probe = [0; 1];
        match output.read(&mut probe) {
            Err(error) if retry(&error) => {}
            _ => return Err(failure("unsolicited output or closed peer")),
        }
        let mut sent = 0;
        let mut frame = Vec::new();
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Err(failure("cancelled"));
            }
            if Instant::now() >= deadline {
                return Err(failure("timed out"));
            }
            if sent < request.len() {
                match input.write(&request[sent..]) {
                    Ok(0) => return Err(failure("write failed")),
                    Ok(n) => sent += n,
                    Err(error) if retry(&error) => {}
                    Err(_) => return Err(failure("write failed")),
                }
            }
            let mut chunk = [0; 8192];
            match output.read(&mut chunk) {
                Ok(0) => return Err(failure("partial frame or closed peer")),
                Ok(n) => {
                    if sent != request.len() || frame.len() + n > MAX_FRAME_BYTES {
                        return Err(failure("invalid or oversized frame"));
                    }
                    let end = chunk[..n]
                        .iter()
                        .position(|b| *b == b'\n')
                        .map(|offset| frame.len() + offset);
                    frame.extend_from_slice(&chunk[..n]);
                    if let Some(end) = end {
                        if end + 1 != frame.len() {
                            return Err(failure("unsolicited frame"));
                        }
                        let response: Response = ahu_harness_api::json::from_slice(&frame[..end])
                            .map_err(|_| failure("invalid response"))?;
                        let result = match response {
                            Response::Success(success)
                                if success.protocol_version == PROTOCOL_VERSION
                                    && success.id == id =>
                            {
                                success.result
                            }
                            Response::Failure(error)
                                if error.protocol_version == PROTOCOL_VERSION && error.id == id =>
                            {
                                return Err(match error.error {
                                    RemoteError::NeedsNativeTrust => failure(&format!(
                                        "operation refused ({operation:?}): the native CLI is waiting for its folder-trust decision in this checkout; open it there interactively and review the prompt (ahu never approves native trust)"
                                    ))
                                    .with_cause(super::HealthCause::NeedsNativeTrust),
                                    RemoteError::NotSignedIn => failure(&format!(
                                        "operation refused ({operation:?}): the native CLI is not signed in"
                                    ))
                                    .with_cause(super::HealthCause::NotSignedIn),
                                    _ => failure(&format!("operation refused ({operation:?})")),
                                });
                            }
                            _ => return Err(failure("response sequence or version mismatch")),
                        };
                        // Check already queued bytes as well as bytes in this read.
                        match output.read(&mut probe) {
                            Ok(0) => {}
                            Err(error) if retry(&error) => {}
                            _ => return Err(failure("unsolicited frame")),
                        }
                        if operation == Operation::DecodeStart {
                            self.decoder = Decoder::Active;
                        }
                        if operation == Operation::DecodeFinish {
                            self.decoder = Decoder::Finished;
                        }
                        return Ok(result);
                    }
                }
                Err(error) if retry(&error) => {}
                Err(_) => return Err(failure("read failed")),
            }
            poll(
                input.as_raw_fd(),
                output.as_raw_fd(),
                sent < request.len(),
                deadline,
            )?;
        }
    }
    /// Close stdin, require clean EOF and successful exit within the operation
    /// deadline, then terminate remaining owned descendants. No decoder finalization
    /// is invented: callers must send decode_finish before finishing a decoder.
    pub fn finish(mut self) -> Result<()> {
        let result = self.finish_inner();
        let cleanup = self.terminate();
        result.and(cleanup)
    }
    fn finish_inner(&mut self) -> Result<()> {
        if self.decoder == Decoder::Active {
            return Err(failure("decoder was not finalized"));
        }
        self.input = None;
        let deadline = Instant::now() + self.prepared.timeout;
        let output = self
            .output
            .as_mut()
            .ok_or_else(|| failure("session closed"))?;
        let child = self
            .child
            .as_ref()
            .ok_or_else(|| failure("session closed"))?;
        let mut eof = false;
        loop {
            if Instant::now() >= deadline {
                return Err(failure("finalization timed out"));
            }
            let mut byte = [0; 1];
            if !eof {
                match output.read(&mut byte) {
                    Ok(0) => eof = true,
                    Err(error) if retry(&error) => {}
                    _ => return Err(failure("unsolicited output during finalization")),
                }
            }
            if eof
                && crate::headless::child_exited(child.id())
                    .map_err(|_| failure("exit inspection failed"))?
            {
                break;
            }
            // Do not reap the leader before signalling its owned process group.
            std::thread::sleep(Duration::from_millis(2));
        }
        let status = self.terminate_status()?;
        if !status.is_some_and(|status| status.success()) {
            return Err(failure("peer failed"));
        }
        Ok(())
    }
    fn terminate_status(&mut self) -> Result<Option<std::process::ExitStatus>> {
        self.input = None;
        self.output = None;
        let Some(mut child) = self.child.take() else {
            return Ok(None);
        };
        crate::headless::signal_group(child.id(), libc::SIGKILL);
        let _ = child.kill();
        let until = Instant::now() + Duration::from_millis(250);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Ok(Some(status)),
                Err(_) => return Err(failure("cleanup failed")),
                Ok(None) if Instant::now() >= until => {
                    return Err(failure("cleanup deadline exceeded"));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(2)),
            }
        }
    }
    fn terminate(&mut self) -> Result<()> {
        self.terminate_status().map(|_| ())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}
fn retry(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
    )
}
fn nonblocking(fd: i32) -> Result<()> {
    // SAFETY: fd is a live pipe owned by this session for both calls.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    // SAFETY: F_SETFL changes only the flags of this live owned pipe.
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(failure("pipe configuration failed"));
    }
    Ok(())
}
fn poll(input: i32, output: i32, writing: bool, deadline: Instant) -> Result<()> {
    let mut descriptors = [
        libc::pollfd {
            fd: if writing { input } else { -1 },
            events: libc::POLLOUT,
            revents: 0,
        },
        libc::pollfd {
            fd: output,
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    let millis = deadline
        .saturating_duration_since(Instant::now())
        .as_millis()
        .clamp(1, 20) as i32;
    // SAFETY: descriptors points to two live pollfd entries; -1 is ignored.
    if unsafe { libc::poll(descriptors.as_mut_ptr(), descriptors.len() as _, millis) } < 0
        && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
    {
        return Err(failure("poll failed"));
    }
    Ok(())
}

// Bound allocation while encoding too: a caller's Value may already exceed the
// protocol budget. Reserve one byte for JSONL termination.
struct FrameWriter(Vec<u8>);
impl Write for FrameWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() >= MAX_FRAME_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other("frame limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

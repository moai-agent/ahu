//! Owned, bounded inspection processes; no main agent supervision.
use crate::{Error, Result};
use std::os::{fd::AsRawFd, unix::process::CommandExt};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
pub(crate) fn configure(command: &mut Command, home: &Path, environment: &crate::Environment) {
    // Explicit home and a minimal environment prevent accidental ambient auth/config routing.
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .envs(environment)
        .env("NO_COLOR", "1");
}
pub(crate) fn signal_group(id: u32, signal: i32) {
    if let Ok(id) = i32::try_from(id) {
        // SAFETY: negative owned child PID selects only its freshly created process group.
        unsafe {
            libc::kill(-id, signal);
        }
    }
}
pub(crate) fn run(
    executable: &Path,
    args: &[&str],
    cwd: &Path,
    home: &Path,
    environment: &crate::Environment,
    timeout: Duration,
) -> Result<Vec<u8>> {
    let mut command = Command::new(executable);
    configure(&mut command, home, environment);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = command.spawn().map_err(|_| Error::invalid())?;
    let outcome = (|| {
        let mut stdout = child.stdout.take().ok_or_else(Error::invalid)?;
        let fd = stdout.as_raw_fd();
        // SAFETY: stdout owns fd for the entire loop. Only descriptor flags are changed.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(Error::uncertain());
        }
        let deadline = Instant::now() + timeout;
        let mut output = Vec::new();
        let mut buffer = [0; 8192];
        let mut eof = false;
        loop {
            match stdout.read(&mut buffer) {
                Ok(0) => eof = true,
                Ok(n) => {
                    output.extend_from_slice(&buffer[..n]);
                    if output.len() > 1024 * 1024 {
                        return Err(Error::uncertain());
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return Err(Error::uncertain()),
            }
            if eof && let Some(status) = child.try_wait()? {
                return if status.success() {
                    Ok(output)
                } else {
                    Err(Error::uncertain())
                };
            }
            if Instant::now() >= deadline {
                return Err(Error::uncertain());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    })();
    signal_group(child.id(), libc::SIGKILL);
    let _ = child.wait();
    outcome.map_err(|_| Error::uncertain())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn timeout_and_output_limits_kill_owned_probe_without_waiting_for_eof() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let exe = root.join("fake");
        for body in [
            "sleep 30",
            "while :; do printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'; done",
        ] {
            std::fs::write(&exe, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
            let start = Instant::now();
            let error = run(
                &exe,
                &[],
                &root,
                &root,
                &crate::Environment::new(),
                Duration::from_millis(100),
            )
            .unwrap_err();
            assert!(error.effects_unknown);
            assert!(start.elapsed() < Duration::from_secs(2));
        }
    }
}

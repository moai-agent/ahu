//! One-shot launch planner; the caller owns all native execution.
mod planner;
mod protocol;

fn main() -> std::process::ExitCode {
    // Bound stalled stdin/stdout as well as parsing. The host also enforces
    // the manifest timeout. Process exit terminates this watchdog on success.
    if std::thread::Builder::new()
        .spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(5000));
            std::process::exit(2);
        })
        .is_err()
    {
        return std::process::ExitCode::from(2);
    }
    match protocol::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            std::process::ExitCode::from(2)
        }
    }
}

//! Interactive session hosting, independent of harness execution.
//!
//! V1 is an in-process Rust boundary, not a dynamic plugin ABI or wire protocol.
//! Discovery deliberately retains cmux as the only built-in host. Headless
//! execution does not discover a host; native hook isolation remains separate.

pub mod repository;
pub mod v1;

use crate::util::{Result, shell_single_quote};
use std::path::Path;

/// Resolve the current interactive host, preserving cmux discovery and errors.
pub fn discover() -> Result<Box<dyn v1::Host>> {
    Ok(Box::new(crate::cmux::Cmux::discover()?))
}

/// A coordinator outside a cmux workspace still runs in its invoking terminal.
/// Keep this selection at the adapter boundary, before probing any host.
pub fn discover_for_coordinator() -> Result<Option<Box<dyn v1::Host>>> {
    if crate::cmux::inherited_workspace().is_none() {
        return Ok(None);
    }
    discover().map(Some)
}

/// Build the shell-interpreted startup command for a task workspace.
///
/// This is the single place where anything is interpolated into shell input.
/// It contains exactly two values, both owned by ahu and both single-quoted:
/// the ahu executable and the task directory. The prompt is not here — the
/// launched `ahu run-task` reads it from a file — so a prompt containing
/// backticks, `$(...)`, `&&`, or newlines cannot become shell code.
pub fn startup_command(ahu_executable: &Path, task_dir: &Path) -> String {
    format!(
        "{} run-task --task-dir {}",
        shell_single_quote(&ahu_executable.to_string_lossy()),
        shell_single_quote(&task_dir.to_string_lossy())
    )
}

/// Task text gets the entire title budget; identity lives in its own pill.
pub fn workspace_title(_agent_label: &str, task_title: &str) -> String {
    crate::util::task_title_from_prompt(task_title)
}

pub fn workspace_identity(agent: &str, model: &str) -> String {
    format!(
        "{} · {}",
        crate::util::sidebar_text(agent, 32),
        crate::util::sidebar_text(model, 48)
    )
}

//! Interactive session hosting, independent of harness execution.
//!
//! Host V1 has compiled adapters and a versioned executable wire protocol.
//! The optional compiled cmux package supplies session mechanics; inventory is
//! side-effect-free and activation is separate from executable discovery.

pub mod external;
pub mod plugins;
pub mod repository;
pub mod v1;

use crate::util::{Result, shell_single_quote};
use std::path::Path;

/// Activate the selected host, preserving the historical connection API.
/// Side-effect-free plugin discovery uses `plugins::status`, not this function.
pub fn discover() -> Result<Box<dyn v1::Host>> {
    let status = plugins::status()?;
    if status.selected && status.origin == "local-manifest" {
        return Ok(Box::new(external::ExternalHost::connect(&status)?));
    }
    plugins::require_cmux()?;
    #[cfg(feature = "cmux-plugin")]
    {
        Ok(Box::new(crate::cmux::Cmux::discover()?))
    }
    #[cfg(not(feature = "cmux-plugin"))]
    unreachable!("require_cmux refuses a build without the plugin")
}

/// Existing durable records have no external host binding. Never let a
/// selected executable interpret cmux identifiers or liveness evidence.
pub fn discover_for_tasks() -> Result<Box<dyn v1::Host>> {
    let status = plugins::status()?;
    if status.selected && status.origin == "local-manifest" {
        return Err(crate::util::Error::new(external::PERSISTENCE_BLOCKER)
            .with_kind(crate::util::ErrorKind::Prerequisite));
    }
    discover()
}

/// A coordinator outside a cmux workspace still runs in its invoking terminal.
/// Keep this selection at the adapter boundary, before probing any host.
pub fn discover_for_coordinator() -> Result<Option<Box<dyn v1::Host>>> {
    let status = plugins::status()?;
    if !status.selected {
        return Ok(None);
    }
    if status.origin == "local-manifest" {
        return Err(crate::util::Error::new(external::PERSISTENCE_BLOCKER)
            .with_kind(crate::util::ErrorKind::Prerequisite));
    }
    plugins::require_cmux()?;
    if std::env::var("CMUX_WORKSPACE_ID")
        .ok()
        .filter(|id| !id.is_empty())
        .is_none()
    {
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

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

pub mod binding;

/// Selection intent only: no package reads, authority checks, or host execution.
pub(crate) struct Selection {
    pub id: String,
    pub source: &'static str,
}

pub(crate) fn selection(repo: Option<&crate::git::Repo>) -> Result<Selection> {
    match std::env::var("AHU_INTERACTIVE_PLUGIN") {
        Ok(id) => {
            return Ok(Selection {
                id,
                source: "environment",
            });
        }
        Err(std::env::VarError::NotPresent) => {}
        Err(_) => return Err(crate::util::Error::new("invalid session host selection")),
    }
    if let Some(repo) = repo {
        let enabled: Vec<_> = plugins::project::inventory(&repo.root)?
            .bindings
            .into_iter()
            .filter(|s| s.binding.enabled)
            .map(|s| s.binding.id)
            .collect();
        match enabled.as_slice() {
            [] => {}
            [id] => {
                return Ok(Selection {
                    id: id.clone(),
                    source: "committed-project-binding",
                });
            }
            _ => {
                return Err(crate::util::Error::new(
                    "multiple enabled session hosts; select an exact AHU_INTERACTIVE_PLUGIN id",
                ));
            }
        }
    }
    Ok(Selection {
        id: "cmux".into(),
        source: "build-default",
    })
}

fn compiled() -> Result<Box<dyn v1::Host>> {
    #[cfg(feature = "cmux-plugin")]
    {
        if std::env::var("AHU_INTERACTIVE_PLUGIN").as_deref() == Ok("none") {
            return Err(crate::util::Error::new(
                "cmux session plugin is disabled by AHU_INTERACTIVE_PLUGIN=none. Set AHU_INTERACTIVE_PLUGIN=cmux for this operation. No backend was substituted.",
            ).with_kind(crate::util::ErrorKind::Prerequisite));
        }
        Ok(Box::new(crate::cmux::Cmux::connect_recorded()?))
    }
    #[cfg(not(feature = "cmux-plugin"))]
    {
        Err(
            crate::util::Error::new("cmux session plugin is not included in this build; rebuild with --features cmux-plugin")
                .with_kind(crate::util::ErrorKind::Prerequisite),
        )
    }
}

pub fn discover_for_launch(
    repo: &crate::git::Repo,
) -> Result<(Box<dyn v1::Host>, Option<binding::FrozenHost>)> {
    match selection(Some(repo))?.id.as_str() {
        "cmux" => Ok((compiled()?, None)),
        "none" => Err(crate::util::Error::new("session hosting is disabled")
            .with_kind(crate::util::ErrorKind::Prerequisite)),
        id => {
            let (status, frozen) = binding::select(repo, id)?;
            let host = external::ExternalHost::connect_bound(&status, &frozen)?;
            Ok((Box::new(host), Some(frozen)))
        }
    }
}

/// Public activation always checks committed policy before executing a package.
pub fn discover() -> Result<Box<dyn v1::Host>> {
    let repo = crate::git::discover(&std::env::current_dir()?);
    match selection(repo.as_ref().ok())?.id.as_str() {
        "cmux" | "none" => compiled(),
        _ => discover_for_launch(&repo.map_err(|_| {
            crate::util::Error::new("session host requires a committed project binding")
                .with_kind(crate::util::ErrorKind::Prerequisite)
        })?)
        .map(|(host, _)| host),
    }
}

/// Legacy records belong to compiled cmux regardless of a newer host selection.
pub fn discover_for_tasks() -> Result<Box<dyn v1::Host>> {
    compiled()
}

pub fn discover_for_record(
    repo: &crate::git::Repo,
    record: &crate::task::TaskRecord,
) -> Result<Box<dyn v1::Host>> {
    match &record.session_host {
        None => compiled(),
        Some(frozen) => {
            if std::env::var("AHU_INTERACTIVE_PLUGIN").as_deref() == Ok("none") {
                return Err(crate::util::Error::new("session hosting is disabled"));
            }
            let policy_repo = crate::git::discover(&record.repo_root)?;
            if policy_repo.identity() != repo.identity() || record.repo_identity != repo.identity()
            {
                return Err(crate::util::Error::new(
                    "task session host belongs to a different repository",
                ));
            }
            let status = binding::verify(&policy_repo, frozen)?;
            Ok(Box::new(external::ExternalHost::connect_bound(
                &status, frozen,
            )?))
        }
    }
}

/// No host is needed by a coordinator in its current terminal.
pub fn discover_for_coordinator() -> Result<Option<Box<dyn v1::Host>>> {
    if !cfg!(feature = "cmux-plugin") && std::env::var_os("AHU_INTERACTIVE_PLUGIN").is_none() {
        return Ok(None);
    }
    if std::env::var("CMUX_WORKSPACE_ID")
        .ok()
        .filter(|id| !id.is_empty())
        .is_none()
    {
        return Ok(None);
    }
    if std::env::var("AHU_INTERACTIVE_PLUGIN").as_deref() == Ok("none") {
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

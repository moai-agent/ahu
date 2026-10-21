//! Provider-neutral lifecycle for interactive task sessions.

use std::path::Path;

use super::v2::{
    CoordinatorRequest, CreatedSession, Host, SessionCreationError, SessionCreationResult,
    TaskSessionRequest, WorkspaceOwnership,
};
use crate::bail;
use crate::git::Repo;
use crate::state::{self, LaunchLock};
use crate::task::TaskRecord;
use crate::util::{Error, Result, shell_single_quote};

pub struct CoordinatorPlacement {
    pub notes: Vec<String>,
    pub opened_workspace: bool,
}

pub struct SessionManager<'a> {
    client: &'a dyn Host,
    repo: &'a Repo,
}

impl<'a> SessionManager<'a> {
    pub fn new(client: &'a dyn Host, repo: &'a Repo) -> Self {
        Self { client, repo }
    }

    pub fn create_task_session(
        &self,
        title: &str,
        summary: &str,
        worktree: &Path,
        startup_command: &str,
        focus: bool,
    ) -> SessionCreationResult<CreatedSession> {
        self.client
            .check_persistence()
            .map_err(SessionCreationError::NotCreated)?;
        self.client.create_task_session(&TaskSessionRequest {
            repository_identity: &self.repo.identity(),
            repository_name: &self.repo.display_name(),
            repository_root: &self.repo.root,
            title,
            summary,
            worktree,
            startup_command,
            focus,
        })
    }

    fn recorded_session<'b>(&self, record: &'b TaskRecord) -> Result<Option<&'b str>> {
        self.client.check_persistence()?;
        if record.repo_identity != self.repo.identity() {
            bail!("task belongs to a different repository; no session was selected or closed");
        }
        let expected = record
            .session_host
            .as_ref()
            .map(|pin| pin.key())
            .transpose()?;
        if expected != self.client.persistence_key() {
            bail!("task session host identity mismatch; no session was selected or closed");
        }
        let Some(session_ref) = record.session_ref.as_deref() else {
            if record.session_host.is_some() {
                bail!(
                    "task has no confirmed session reference; preserve its work and reconcile ownership before cleanup"
                );
            }
            return Ok(None);
        };
        if session_ref.is_empty() {
            bail!("task session reference is empty; no session was selected or closed");
        }
        match self.client.verify_task_session(
            session_ref,
            &self.repo.identity(),
            &record.worktree,
        )? {
            WorkspaceOwnership::Absent => Ok(None),
            WorkspaceOwnership::Owned => Ok(Some(session_ref)),
            WorkspaceOwnership::Mismatch => bail!(
                "task session ownership does not match its recorded worktree; no session was selected or closed"
            ),
        }
    }

    pub fn select_task_session(&self, record: &TaskRecord) -> Result<()> {
        let session_ref = self.recorded_session(record)?.ok_or_else(|| {
            Error::new(format!("task {} has no recorded session", record.task_id))
        })?;
        self.client
            .select_task_session(session_ref, &self.repo.identity(), &record.worktree)
    }

    pub fn close_task_session(&self, record: &TaskRecord) -> Result<bool> {
        let Some(session_ref) = self.recorded_session(record)? else {
            return Ok(false);
        };
        self.client
            .close_task_session(session_ref, &self.repo.identity(), &record.worktree)?;
        Ok(true)
    }

    pub fn set_task_status(&self, record: &TaskRecord, status: &str) -> Result<()> {
        let Some(session_ref) = self.recorded_session(record)? else {
            return Ok(());
        };
        self.client
            .set_task_status(session_ref, &self.repo.identity(), &record.worktree, status)
    }

    pub fn expand_task_session(&self, record: &TaskRecord) -> Result<()> {
        let Some(session_ref) = self.recorded_session(record)? else {
            return Ok(());
        };
        self.client
            .expand_task_session(session_ref, &self.repo.identity(), &record.worktree)
    }

    pub fn set_agent_metadata(&self, record: &TaskRecord) -> Result<()> {
        let Some(session_ref) = self.recorded_session(record)? else {
            return Ok(());
        };
        self.client.set_agent_metadata(
            session_ref,
            &self.repo.identity(),
            &record.worktree,
            &record.identity.agent,
            &record.identity.harness,
            &record.identity.model,
        )
    }

    pub fn group_coordinator(
        &self,
        executable: &str,
        label: &str,
        harness: &str,
        model: &str,
        args: &[String],
    ) -> Result<CoordinatorPlacement> {
        self.client.check_persistence()?;
        if self.client.inherited_workspace().is_none() {
            return Ok(CoordinatorPlacement {
                notes: Vec::new(),
                opened_workspace: false,
            });
        }
        self.client.check_capabilities()?;
        let _lock =
            LaunchLock::acquire_at(state::coordination_dir(self.repo)?.join("launch.lock"))?;
        let telemetry = crate::config::load(&self.repo.root)?
            .map(|loaded| loaded.config.telemetry)
            .unwrap_or_default();
        let mut command = std::process::Command::new(executable);
        command.args(args).env("AHU_BIN", std::env::current_exe()?);
        crate::telemetry::configure_coordinator(
            &mut command,
            &telemetry,
            harness,
            model,
            crate::selection::probe_version(executable).as_deref(),
        );
        let startup = coordinator_startup(&command);
        let result = self.client.place_coordinator(&CoordinatorRequest {
            repository_identity: &self.repo.identity(),
            repository_name: &self.repo.display_name(),
            repository_root: &self.repo.root,
            label,
            harness,
            model,
            startup_command: &startup,
        })?;
        Ok(CoordinatorPlacement {
            notes: result.notes,
            opened_workspace: result.opened_session,
        })
    }
}

pub fn group_coordinator(
    repo: &Repo,
    executable: &str,
    label: &str,
    harness: &str,
    model: &str,
    args: &[String],
) -> Result<CoordinatorPlacement> {
    let Some(client) = super::discover_for_coordinator()? else {
        return Ok(CoordinatorPlacement {
            notes: Vec::new(),
            opened_workspace: false,
        });
    };
    SessionManager::new(client.as_ref(), repo)
        .group_coordinator(executable, label, harness, model, args)
}

/// Serialize only explicitly configured environment entries for an interactive
/// session process. The provider interprets this shell string.
fn coordinator_startup(command: &std::process::Command) -> String {
    let mut words = vec!["/usr/bin/env".to_string()];
    for (key, value) in command.get_envs() {
        if value.is_none() {
            words.push("-u".to_string());
            words.push(shell_single_quote(&key.to_string_lossy()));
        }
    }
    for (key, value) in command.get_envs() {
        if let Some(value) = value {
            words.push(shell_single_quote(&format!(
                "{}={}",
                key.to_string_lossy(),
                value.to_string_lossy()
            )));
        }
    }
    words.push(shell_single_quote(&command.get_program().to_string_lossy()));
    words.extend(
        command
            .get_args()
            .map(|arg| shell_single_quote(&arg.to_string_lossy())),
    );
    words.join(" ")
}

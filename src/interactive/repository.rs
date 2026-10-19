//! Repository-owned interactive topology and task-to-workspace associations.
//!
//! Harness adapters never choose groups or surfaces. Interactive work enters
//! through this manager after harness resolution; every task records the exact
//! repository group, window, and workspace returned here. A CMUX workspace is
//! the addressable task surface: the installed CMUX API used by ahu does not
//! return a stable pane/surface identifier for the shell it creates.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::v1::{
    CreatedWorkspace, Group, Host, WorkspaceCreationError, WorkspaceCreationResult,
    WorkspaceRequest,
};
use crate::bail;
use crate::git::{self, Repo};
use crate::state::{self, LaunchLock};
use crate::task::TaskRecord;
use crate::util::{Error, Result, shell_single_quote};

/// Where a repository's CMUX group lives. Stored by object id, not title.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupMapping {
    pub group_id: Option<String>,
    pub window_id: Option<String>,
    pub anchor_workspace_id: Option<String>,
}

pub fn mapping_path(repo: &Repo) -> Result<PathBuf> {
    Ok(state::coordination_dir(repo)?.join("cmux.json"))
}

// External versions never reuse cmux.json or another package's group mapping.
fn host_mapping_path(repo: &Repo, host: &dyn Host) -> Result<PathBuf> {
    match host.persistence_key() {
        None => mapping_path(repo),
        Some(key) => Ok(state::coordination_dir(repo)?.join(format!(
            "host-{}.json",
            crate::util::digest_bytes(key.as_bytes())
        ))),
    }
}

#[derive(Debug, Clone)]
pub struct ManagedTaskWorkspace {
    pub group: Group,
    pub workspace: CreatedWorkspace,
}

pub struct CoordinatorPlacement {
    pub notes: Vec<String>,
    pub opened_workspace: bool,
}

/// The single owner of interactive repository grouping and task workspace
/// associations. The versioned host supplies the transport; this manager owns the
/// repository and lifecycle invariants layered on top of it.
pub struct RepositoryManager<'a> {
    client: &'a dyn Host,
    repo: &'a Repo,
}

impl<'a> RepositoryManager<'a> {
    pub fn new(client: &'a dyn Host, repo: &'a Repo) -> Self {
        Self { client, repo }
    }

    pub fn ensure_group(&self, notes: &mut Vec<String>) -> Result<Group> {
        self.client.check_persistence()?;
        ensure_group(self.client, self.repo, notes)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_task_workspace(
        &self,
        title: &str,
        summary: &str,
        cwd: &Path,
        startup_command: &str,
        focus: bool,
        notes: &mut Vec<String>,
    ) -> WorkspaceCreationResult<ManagedTaskWorkspace> {
        let group = self
            .ensure_group(notes)
            .map_err(WorkspaceCreationError::NotCreated)?;
        let path = host_mapping_path(self.repo, self.client)
            .map_err(WorkspaceCreationError::NotCreated)?;
        let mapping: GroupMapping =
            state::read_json(&path).map_err(WorkspaceCreationError::NotCreated)?;
        let workspace = self.client.create_task_workspace(&WorkspaceRequest {
            group_id: &group.id,
            window_id: mapping.window_id.as_deref(),
            title,
            summary,
            cwd,
            startup_command,
            focus,
        })?;
        // An empty identifier is recorded as the task's durable ownership and
        // can never match a later group/window check, which would strand the
        // workspace beyond focus, status, cancellation and removal.
        if workspace.workspace_id.is_empty()
            || workspace.window_id.is_empty()
            || workspace.group_id.as_deref() != Some(group.id.as_str())
            || mapping
                .window_id
                .as_deref()
                .is_some_and(|window| window != workspace.window_id)
        {
            // Inconsistent ownership is not permission to close a resource,
            // nor evidence that the startup command never ran.
            return Err(WorkspaceCreationError::OutcomeUnknown(Error::new(
                "cmux reported a task workspace with an unexpected group or window; startup is unconfirmed and no workspace was closed",
            )));
        }
        Ok(ManagedTaskWorkspace { group, workspace })
    }

    /// Revalidate the recorded group/workspace pair before focus or cleanup.
    /// Missing and ambiguous relationships fail closed instead of operating on
    /// a similarly named or newly reused surface.
    fn recorded_workspace(&self, record: &TaskRecord) -> Result<Option<String>> {
        self.client.check_persistence()?;
        if record.repo_identity != self.repo.identity() {
            bail!("task belongs to a different repository; no workspace was selected or closed");
        }
        let expected = record
            .session_host
            .as_ref()
            .map(|pin| pin.key())
            .transpose()?;
        if expected != self.client.persistence_key() {
            bail!("task session host identity mismatch; no workspace was selected or closed");
        }
        // Executable hosts always publish all three identifiers together.
        // Missing identifiers may mean creation succeeded without a reply, not
        // that no workspace exists. Legacy cmux records predate this contract.
        if record.session_host.is_some()
            && [
                &record.cmux_workspace_id,
                &record.cmux_group_id,
                &record.cmux_window_id,
            ]
            .iter()
            .any(|id| id.as_deref().is_none_or(str::is_empty))
        {
            bail!(
                "external session workspace ownership is incomplete; creation may have succeeded. Preserve task inputs and reconcile ownership before focus or cleanup"
            );
        }
        let Some(workspace_id) = record.cmux_workspace_id.as_deref() else {
            return Ok(None);
        };
        let inventory = self.client.workspaces()?;
        let Some(workspace) = inventory.get(workspace_id) else {
            return Ok(None);
        };
        if record.session_host.is_some() && Path::new(&workspace.directory) != record.worktree {
            bail!("task workspace directory mismatch; no workspace was selected or closed");
        }
        let groups = self.client.list_groups(record.cmux_window_id.as_deref())?;
        let owners: Vec<_> = groups
            .iter()
            .filter(|group| {
                group
                    .member_workspace_ids
                    .iter()
                    .any(|id| id == workspace_id)
            })
            .collect();
        if owners.len() != 1 {
            bail!(
                "cannot confirm the CMUX group for task {}: its recorded workspace belongs to {} groups; no workspace was selected or closed",
                record.task_id,
                owners.len()
            );
        }
        let owner = owners[0];
        if record
            .cmux_group_id
            .as_deref()
            .is_some_and(|expected| expected != owner.id)
        {
            bail!(
                "cannot confirm the CMUX group for task {}: recorded group {} does not own workspace {}; no workspace was selected or closed",
                record.task_id,
                record.cmux_group_id.as_deref().unwrap_or_default(),
                workspace_id
            );
        }
        if let Some(window) = record.cmux_window_id.as_deref()
            && self.client.window_for_workspace(workspace_id)? != window
        {
            bail!(
                "cannot confirm the CMUX window for task {}; no workspace was selected or closed",
                record.task_id
            );
        }
        Ok(Some(workspace_id.to_string()))
    }

    pub fn select_task_workspace(&self, record: &TaskRecord) -> Result<()> {
        let workspace_id = self.recorded_workspace(record)?.ok_or_else(|| {
            Error::new(format!(
                "task {} has no recorded CMUX workspace",
                record.task_id
            ))
        })?;
        self.client.select_workspace(&workspace_id)
    }

    pub fn close_task_workspace(&self, record: &TaskRecord) -> Result<bool> {
        let Some(workspace_id) = self.recorded_workspace(record)? else {
            return Ok(false);
        };
        self.client.close_workspace(&workspace_id)?;
        Ok(true)
    }

    pub fn set_task_status(&self, record: &TaskRecord, status: &str) -> Result<()> {
        let Some(workspace_id) = self.recorded_workspace(record)? else {
            return Ok(());
        };
        self.client.set_status(&workspace_id, status)
    }

    pub fn expand_task_group(&self, record: &TaskRecord) -> Result<()> {
        if self.recorded_workspace(record)?.is_none() {
            return Ok(());
        }
        let group_id = record.cmux_group_id.as_deref().ok_or_else(|| {
            Error::new(format!(
                "task {} has no recorded CMUX group",
                record.task_id
            ))
        })?;
        self.client.expand_group(group_id)
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
        let Some(workspace) = self.client.inherited_workspace() else {
            return Ok(CoordinatorPlacement {
                notes: Vec::new(),
                opened_workspace: false,
            });
        };
        self.client.check_capabilities()?;
        let caller_window = self.client.window_for_workspace(&workspace)?;
        let _lock =
            LaunchLock::acquire_at(state::coordination_dir(self.repo)?.join("launch.lock"))?;
        let mut notes = Vec::new();
        let group = self.ensure_group(&mut notes)?;
        let mapping: GroupMapping = state::read_json(&host_mapping_path(self.repo, self.client)?)?;
        let window = mapping
            .window_id
            .ok_or_else(|| Error::new("cmux repository group has no known window"))?;
        if !group.member_workspace_ids.contains(&workspace) {
            // The repository mapping may point into a different window. Query
            // the caller's window before allowing a move into the target group.
            let belongs_to_other_group = self
                .client
                .list_groups(Some(&caller_window))?
                .into_iter()
                .any(|candidate| {
                    candidate.id != group.id
                        && candidate
                            .member_workspace_ids
                            .iter()
                            .any(|member| member == &workspace)
                });
            if belongs_to_other_group {
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
                let created = self.client.create_task_workspace(&WorkspaceRequest {
                    group_id: &group.id,
                    window_id: Some(&window),
                    title: &format!("{label} coordinator"),
                    summary: &format!("{label} coordinator for {}", self.repo.display_name()),
                    cwd: &self.repo.root,
                    startup_command: &startup,
                    focus: true,
                })?;
                if let Err(error) = self.client.set_agent_metadata(
                    &created.workspace_id,
                    "director",
                    harness,
                    model,
                ) {
                    notes.push(format!(
                        "could not set coordinator metadata in cmux: {error}"
                    ));
                }
                notes.push(format!(
                    "the invoking workspace already belongs to another cmux group; ahu opened a new {label} coordinator workspace under the {} group.",
                    self.repo.display_name()
                ));
                return Ok(CoordinatorPlacement {
                    notes,
                    opened_workspace: true,
                });
            }
            self.client
                .add_workspace_to_group(&group.id, &workspace, &window)?;
        }
        if let Err(error) = self
            .client
            .set_agent_metadata(&workspace, "director", harness, model)
        {
            notes.push(format!(
                "could not set coordinator metadata in cmux: {error}"
            ));
        }
        self.client.expand_group(&group.id)?;
        Ok(CoordinatorPlacement {
            notes,
            opened_workspace: false,
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
    RepositoryManager::new(client.as_ref(), repo)
        .group_coordinator(executable, label, harness, model, args)
}

fn ensure_group(client: &dyn Host, repo: &Repo, notes: &mut Vec<String>) -> Result<Group> {
    let path = host_mapping_path(repo, client)?;
    let mut mapping: GroupMapping = state::read_json(&path)?;
    let current_window = client.current_window().ok().flatten();
    let groups = client.list_groups(current_window.as_deref())?;
    let current_workspace = client.current_workspace()?;
    let workspaces = client.workspaces_in_window(current_window.as_deref())?;
    let candidates = repository_group_candidates(&groups, &repo.display_name(), |id| {
        workspaces.get(id).is_some_and(|workspace| {
            git::discover_untrusted(Path::new(&workspace.directory))
                .is_ok_and(|found| found.identity() == repo.identity())
        })
    });
    let current_group = if candidates.iter().any(|group| {
        current_workspace
            .as_ref()
            .is_some_and(|id| group.member_workspace_ids.contains(id))
    }) {
        recover_group(&candidates, current_workspace.as_deref())?
    } else {
        None
    };
    if let Some(group) = current_group
        && mapping.group_id.as_deref() != Some(group.id.as_str())
    {
        mapping = GroupMapping {
            group_id: Some(group.id.clone()),
            window_id: current_window.clone(),
            anchor_workspace_id: Some(group.anchor_workspace_id.clone()),
        };
        state::write_json(&path, &mapping)?;
        return Ok(group.clone());
    }
    if let Some(group_id) = mapping.group_id.clone() {
        let window = mapping.window_id.clone().or_else(|| current_window.clone());
        if let Some(group) = client.find_group(&group_id, window.as_deref())? {
            if mapping.anchor_workspace_id.as_deref() != Some(group.anchor_workspace_id.as_str()) {
                match restore_anchor(client, repo, &group) {
                    Ok(anchor) => {
                        notes.push("the repository group's anchor workspace had been closed, so cmux had promoted a task into the header row. ahu created a new anchor so that task is visible again.".to_string());
                        mapping.anchor_workspace_id = Some(anchor);
                    }
                    Err(error) => notes.push(format!("the repository group's anchor changed and ahu could not restore a dedicated one ({error}); one task may be hidden under the group header.")),
                }
                mapping.window_id = window.clone();
                state::write_json(&path, &mapping)?;
                if let Some(refreshed) = client.find_group(&group_id, window.as_deref())? {
                    return Ok(refreshed);
                }
            }
            return Ok(group);
        }
        notes.push(format!("the cmux group recorded for this repository ({group_id}) no longer exists; ahu will find an existing group or create one."));
    }
    let group = match recover_group(&candidates, None)? {
        Some(group) => group.clone(),
        None => client.create_group(&repo.display_name(), &repo.root)?,
    };
    mapping = GroupMapping {
        group_id: Some(group.id.clone()),
        window_id: current_window,
        anchor_workspace_id: Some(group.anchor_workspace_id.clone()),
    };
    state::write_json(&path, &mapping)?;
    Ok(group)
}

pub(crate) fn repository_group_candidates<'a>(
    groups: &'a [Group],
    name: &str,
    belongs: impl Fn(&str) -> bool,
) -> Vec<&'a Group> {
    groups
        .iter()
        .filter(|group| {
            group.name == name && group.member_workspace_ids.iter().any(|id| belongs(id))
        })
        .collect()
}

pub(crate) fn recover_group<'a>(
    groups: &[&'a Group],
    current: Option<&str>,
) -> Result<Option<&'a Group>> {
    if let Some(group) = groups.iter().find(|group| {
        current.is_some_and(|id| group.member_workspace_ids.iter().any(|member| member == id))
    }) {
        return Ok(Some(group));
    }
    match groups {
        [group] => Ok(Some(group)),
        [] => Ok(None),
        _ => bail!(
            "Multiple cmux groups match this repository. Run ahu from the group you want to use."
        ),
    }
}

fn restore_anchor(client: &dyn Host, repo: &Repo, group: &Group) -> Result<String> {
    let anchor = client.create_anchor_workspace(&group.id, &repo.display_name(), &repo.root)?;
    client.set_anchor(&group.id, &anchor)?;
    Ok(anchor)
}

/// Serialize only the explicit settings configured on this fresh command.
/// Neither cmux's server environment nor arbitrary caller variables are copied.
fn coordinator_startup(command: &std::process::Command) -> String {
    let mut words = vec!["/usr/bin/env".to_string()];
    // env options must precede assignments on both BSD and GNU env.
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

#[cfg(test)]
mod coordinator_environment_tests {
    use super::*;

    #[test]
    fn relocated_coordinator_preserves_only_a_valid_eval_receiver() {
        for endpoint in ["http://127.0.0.1:54321", "https://example.invalid"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .env_clear()
                .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|value| ("LLVM_PROFILE_FILE", value)))
                .env("AHU_TEST_COORDINATOR_EVAL", "1")
                .env("AHU_EVAL_OTEL_ENDPOINT", endpoint)
                .args([
                    "--exact",
                    "interactive::repository::coordinator_environment_tests::coordinator_eval_environment_child",
                    "--nocapture",
                ])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("coordinator eval checked"));
        }
    }

    #[test]
    fn coordinator_eval_environment_child() {
        if std::env::var("AHU_TEST_COORDINATOR_EVAL").as_deref() != Ok("1") {
            return;
        }
        let expected = crate::telemetry::eval_endpoint_override().unwrap_or_default();
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", "printf '%s' \"${AHU_EVAL_OTEL_ENDPOINT-}\""]);
        crate::telemetry::configure_coordinator(
            &mut command,
            &Default::default(),
            "codex",
            "fixture",
            None,
        );
        let output = std::process::Command::new("/bin/sh")
            .env_clear()
            .env("AHU_EVAL_OTEL_ENDPOINT", "http://127.0.0.1:9")
            .args(["-c", &coordinator_startup(&command)])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
        println!("coordinator eval checked");
    }

    #[test]
    fn relocated_coordinator_shell_applies_export_configuration_and_removals() {
        if crate::telemetry::test_without_eval_endpoint(
            "interactive::repository::coordinator_environment_tests::relocated_coordinator_shell_applies_export_configuration_and_removals",
        ) {
            return;
        }
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", r#"test -z "${OTEL_EXPORTER_OTLP_HEADERS+x}" && test -z "${HTTP_PROXY+x}" && printf '%s\n' "$OTEL_EXPORTER_OTLP_ENDPOINT" "$OTEL_SERVICE_NAME" "$AHU_MCP_RESOURCE_ATTRIBUTES" "$1" "${UNRELATED_PRIVATE+x}""#, "fixture", "literal ' $(false)"]);
        crate::telemetry::configure_coordinator(
            &mut command,
            &crate::config::TelemetryConfig {
                enabled: true,
                ..Default::default()
            },
            "codex",
            "synthetic-model",
            Some("1.2.3"),
        );
        let output = std::process::Command::new("/bin/sh")
            .env_clear()
            .env("OTEL_EXPORTER_OTLP_HEADERS", "synthetic-secret")
            .env("HTTP_PROXY", "http://synthetic.invalid")
            .args(["-c", &coordinator_startup(&command)])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("http://127.0.0.1:4318"));
        assert!(text.contains("ahu-agent"));
        assert!(text.contains("ahu.agent.name=director"));
        assert!(text.contains("ahu.execution.mode=interactive"));
        assert!(text.contains("literal ' $(false)"));
        assert!(!text.contains("synthetic-secret"));
    }
}

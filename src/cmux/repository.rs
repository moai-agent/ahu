//! Repository-owned CMUX topology and task-to-workspace associations.
//!
//! Harness adapters never choose groups or surfaces. Interactive work enters
//! through this manager after harness resolution; every task records the exact
//! repository group, window, and workspace returned here. A CMUX workspace is
//! the addressable task surface: the installed CMUX API used by ahu does not
//! return a stable pane/surface identifier for the shell it creates.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bail;
use crate::cmux::{self, Cmux, Group};
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

#[derive(Debug, Clone)]
pub struct ManagedTaskWorkspace {
    pub group: Group,
    pub workspace: cmux::CreatedWorkspace,
}

pub struct CoordinatorPlacement {
    pub notes: Vec<String>,
    pub opened_workspace: bool,
}

/// The single owner of interactive repository grouping and task workspace
/// associations. The CMUX client remains the transport; this manager owns the
/// repository and lifecycle invariants layered on top of it.
pub struct RepositoryManager<'a> {
    client: &'a Cmux,
    repo: &'a Repo,
}

impl<'a> RepositoryManager<'a> {
    pub fn new(client: &'a Cmux, repo: &'a Repo) -> Self {
        Self { client, repo }
    }

    pub fn ensure_group(&self, notes: &mut Vec<String>) -> Result<Group> {
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
    ) -> Result<ManagedTaskWorkspace> {
        let group = self.ensure_group(notes)?;
        let mapping: GroupMapping = state::read_json(&mapping_path(self.repo)?)?;
        let workspace = self.client.create_task_workspace(
            &group.id,
            mapping.window_id.as_deref(),
            title,
            summary,
            cwd,
            startup_command,
            focus,
        )?;
        if workspace.group_id.as_deref() != Some(group.id.as_str())
            || mapping
                .window_id
                .as_deref()
                .is_some_and(|window| window != workspace.window_id)
        {
            // The workspace id is returned by CMUX itself, so cleanup targets
            // only this newly created workspace if CMUX reports inconsistent
            // ownership. Never guess which existing workspace should be moved.
            let _ = self.client.close_workspace(&workspace.workspace_id);
            bail!(
                "cmux created a task workspace with an unexpected group or window; the new workspace was closed and no task was started"
            );
        }
        Ok(ManagedTaskWorkspace { group, workspace })
    }

    /// Revalidate the recorded group/workspace pair before focus or cleanup.
    /// Missing and ambiguous relationships fail closed instead of operating on
    /// a similarly named or newly reused surface.
    fn recorded_workspace(&self, record: &TaskRecord) -> Result<Option<String>> {
        let Some(workspace_id) = record.cmux_workspace_id.as_deref() else {
            return Ok(None);
        };
        if !self.client.workspaces()?.contains_key(workspace_id) {
            return Ok(None);
        }
        let groups = self.client.list_groups(None)?;
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
        let Some(workspace) = std::env::var("CMUX_WORKSPACE_ID")
            .ok()
            .filter(|id| !id.is_empty())
        else {
            return Ok(CoordinatorPlacement {
                notes: Vec::new(),
                opened_workspace: false,
            });
        };
        self.client.check_capabilities()?;
        self.client.window_for_workspace(&workspace)?;
        let _lock =
            LaunchLock::acquire_at(state::coordination_dir(self.repo)?.join("launch.lock"))?;
        let mut notes = Vec::new();
        let group = self.ensure_group(&mut notes)?;
        let mapping: GroupMapping = state::read_json(&mapping_path(self.repo)?)?;
        let window = mapping
            .window_id
            .ok_or_else(|| Error::new("cmux repository group has no known window"))?;
        if !group.member_workspace_ids.contains(&workspace) {
            let belongs_to_other_group =
                self.client
                    .list_groups(Some(&window))?
                    .into_iter()
                    .any(|candidate| {
                        candidate.id != group.id
                            && candidate
                                .member_workspace_ids
                                .iter()
                                .any(|member| member == &workspace)
                    });
            if belongs_to_other_group {
                let startup = std::iter::once(shell_single_quote(executable))
                    .chain(args.iter().map(|arg| shell_single_quote(arg)))
                    .collect::<Vec<_>>()
                    .join(" ");
                let created = self.client.create_coordinator_workspace(
                    &group.id,
                    Some(&window),
                    &format!("{label} coordinator"),
                    &format!("{label} coordinator for {}", self.repo.display_name()),
                    &self.repo.root,
                    &startup,
                )?;
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
    if std::env::var("CMUX_WORKSPACE_ID")
        .ok()
        .filter(|id| !id.is_empty())
        .is_none()
    {
        return Ok(CoordinatorPlacement {
            notes: Vec::new(),
            opened_workspace: false,
        });
    }
    let client = Cmux::discover()?;
    RepositoryManager::new(&client, repo).group_coordinator(executable, label, harness, model, args)
}

fn ensure_group(client: &Cmux, repo: &Repo, notes: &mut Vec<String>) -> Result<Group> {
    let path = mapping_path(repo)?;
    let mut mapping: GroupMapping = state::read_json(&path)?;
    let current_window = client.current_window().ok().flatten();
    let groups = client.list_groups(current_window.as_deref())?;
    let current_workspace = client.current_workspace()?;
    let workspaces = client.workspaces_in_window(current_window.as_deref())?;
    let candidates = repository_group_candidates(&groups, &repo.display_name(), |id| {
        workspaces.get(id).is_some_and(|workspace| {
            git::discover(Path::new(&workspace.directory))
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

fn restore_anchor(client: &Cmux, repo: &Repo, group: &Group) -> Result<String> {
    let anchor = client.create_anchor_workspace(&group.id, &repo.display_name(), &repo.root)?;
    client.set_anchor(&group.id, &anchor)?;
    Ok(anchor)
}

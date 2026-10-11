//! Version 1 of ahu's interactive host contract.
//!
//! Implementations supply terminal operations only. ahu owns task identity,
//! native authentication/approvals, records, repository association checks,
//! cancellation confirmation and worktree cleanup. Errors must remain errors:
//! an incomplete inventory must never be returned as an empty successful list.
//! This contract currently models cmux's group/window/workspace topology.

use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub anchor_workspace_id: String,
    pub member_workspace_ids: Vec<String>,
    pub is_collapsed: bool,
}

/// What `workspace.list` reports about one workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceInfo {
    pub directory: String,
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreatedWorkspace {
    pub workspace_id: String,
    pub window_id: String,
    pub group_id: Option<String>,
}

/// A failed create request is not evidence that its startup command never ran.
#[derive(Debug)]
pub enum WorkspaceCreationError {
    /// The request could not have created a workspace or started its command.
    NotCreated(crate::Error),
    /// Creation or startup may have occurred. Preserve all task inputs and work.
    OutcomeUnknown(crate::Error),
}

pub type WorkspaceCreationResult<T> = std::result::Result<T, WorkspaceCreationError>;

impl std::fmt::Display for WorkspaceCreationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotCreated(error) => write!(f, "workspace was not created: {error}"),
            Self::OutcomeUnknown(error) => {
                write!(f, "workspace creation outcome is unknown: {error}")
            }
        }
    }
}

impl std::error::Error for WorkspaceCreationError {}

impl From<WorkspaceCreationError> for crate::Error {
    fn from(error: WorkspaceCreationError) -> Self {
        let kind = match &error {
            WorkspaceCreationError::NotCreated(error)
            | WorkspaceCreationError::OutcomeUnknown(error) => error.kind(),
        };
        Self::new(error.to_string()).with_kind(kind)
    }
}

/// A shell startup command assembled by ahu, never a task prompt.
/// The host may start it immediately; callers must persist task inputs first.
pub struct WorkspaceRequest<'a> {
    pub group_id: &'a str,
    pub window_id: Option<&'a str>,
    pub title: &'a str,
    pub summary: &'a str,
    pub cwd: &'a Path,
    pub startup_command: &'a str,
    pub focus: bool,
}

/// Implement V1 explicitly; later incompatible contracts belong in a new module.
/// This trait is object safe so lifecycle code can use a synthetic host in tests.
pub trait Host {
    /// Workspace inherited by the invoking process, without a host query.
    fn inherited_workspace(&self) -> Option<String>;
    /// Refuse missing launch prerequisites before ahu creates a worktree.
    fn check_capabilities(&self) -> Result<Vec<String>>;

    fn list_groups(&self, window_id: Option<&str>) -> Result<Vec<Group>>;

    fn find_group(&self, group_id: &str, window_id: Option<&str>) -> Result<Option<Group>>;

    fn current_window(&self) -> Result<Option<String>>;

    fn window_for_workspace(&self, workspace_id: &str) -> Result<String>;

    fn add_workspace_to_group(
        &self,
        group_id: &str,
        workspace_id: &str,
        window_id: &str,
    ) -> Result<()>;

    fn current_workspace(&self) -> Result<Option<String>>;

    fn create_anchor_workspace(&self, group_id: &str, title: &str, cwd: &Path) -> Result<String>;

    fn set_anchor(&self, group_id: &str, workspace_id: &str) -> Result<()>;

    fn create_group(&self, name: &str, cwd: &Path) -> Result<Group>;

    fn expand_group(&self, group_id: &str) -> Result<()>;

    /// Success must identify the new workspace by its exact requested directory,
    /// including when it is the only new candidate. A failed command or lookup
    /// after dispatch must not claim non-creation without affirmative evidence.
    fn create_task_workspace(
        &self,
        request: &WorkspaceRequest<'_>,
    ) -> WorkspaceCreationResult<CreatedWorkspace>;

    fn open_markdown(&self, path: &Path, focus: bool) -> Result<String>;

    fn select_workspace(&self, workspace_id: &str) -> Result<()>;

    fn close_workspace(&self, workspace_id: &str) -> Result<()>;

    fn set_status(&self, workspace_id: &str, value: &str) -> Result<()>;

    fn set_agent_metadata(
        &self,
        workspace_id: &str,
        agent: &str,
        harness: &str,
        model: &str,
    ) -> Result<()>;

    /// Complete inventory across all windows; partial coverage is an error.
    fn workspaces(&self) -> Result<BTreeMap<String, WorkspaceInfo>>;

    fn workspaces_in_window(&self, window: Option<&str>)
    -> Result<BTreeMap<String, WorkspaceInfo>>;
}

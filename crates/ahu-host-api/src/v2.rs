//! Provider-neutral interactive session contract. Providers own their native
//! topology, persistence, and workspace lifecycle rules; Ahu stores opaque refs.

use crate::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreatedSession {
    pub session_ref: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceOwnership {
    Absent,
    Owned,
    Mismatch,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoordinatorPlacement {
    pub notes: Vec<String>,
    pub opened_session: bool,
}

/// A failed create request is not evidence that its startup command never ran.
#[derive(Debug)]
pub enum SessionCreationError {
    NotCreated(crate::Error),
    OutcomeUnknown(crate::Error),
}
pub type SessionCreationResult<T> = std::result::Result<T, SessionCreationError>;

impl std::fmt::Display for SessionCreationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotCreated(error) => write!(f, "session was not created: {error}"),
            Self::OutcomeUnknown(error) => {
                write!(f, "session creation outcome is unknown: {error}")
            }
        }
    }
}
impl std::error::Error for SessionCreationError {}
impl From<SessionCreationError> for crate::Error {
    fn from(error: SessionCreationError) -> Self {
        let kind = match &error {
            SessionCreationError::NotCreated(error)
            | SessionCreationError::OutcomeUnknown(error) => error.kind(),
        };
        Self::new(error.to_string()).with_kind(kind)
    }
}

/// Request for a task session. The provider chooses its own topology.
pub struct TaskSessionRequest<'a> {
    pub repository_identity: &'a str,
    pub repository_name: &'a str,
    pub repository_root: &'a Path,
    pub title: &'a str,
    pub summary: &'a str,
    pub worktree: &'a Path,
    pub startup_command: &'a str,
    pub focus: bool,
}

pub struct CoordinatorRequest<'a> {
    pub repository_identity: &'a str,
    pub repository_name: &'a str,
    pub repository_root: &'a Path,
    pub label: &'a str,
    pub harness: &'a str,
    pub model: &'a str,
    pub startup_command: &'a str,
}

/// Implement this exact current contract; pre-release builds do not retain
/// compatibility with earlier host schemas or protocol versions.
///
/// Ahu can pass an empty session reference while reconciling a task whose
/// provider created a workspace but whose creation reply was not persisted.
/// Hosts must resolve it by exact repository identity and worktree path, return
/// `Absent` when none exists, and refuse ambiguous matches. Mutations receiving
/// an empty reference must apply the same ownership check; empty is never a
/// wildcard.
pub trait Host {
    fn check_persistence(&self) -> Result<()> {
        Ok(())
    }
    fn persistence_key(&self) -> Option<String> {
        None
    }
    fn inherited_workspace(&self) -> Option<String>;
    fn check_capabilities(&self) -> Result<Vec<String>>;

    fn create_task_session(
        &self,
        request: &TaskSessionRequest<'_>,
    ) -> SessionCreationResult<CreatedSession>;
    fn verify_task_session(
        &self,
        session_ref: &str,
        repository_identity: &str,
        expected_directory: &Path,
    ) -> Result<WorkspaceOwnership>;
    fn select_task_session(
        &self,
        session_ref: &str,
        repository_identity: &str,
        expected_directory: &Path,
    ) -> Result<()>;
    fn close_task_session(
        &self,
        session_ref: &str,
        repository_identity: &str,
        expected_directory: &Path,
    ) -> Result<()>;
    fn set_task_status(
        &self,
        session_ref: &str,
        repository_identity: &str,
        expected_directory: &Path,
        value: &str,
    ) -> Result<()>;
    fn expand_task_session(
        &self,
        session_ref: &str,
        repository_identity: &str,
        expected_directory: &Path,
    ) -> Result<()>;
    fn set_agent_metadata(
        &self,
        session_ref: &str,
        repository_identity: &str,
        expected_directory: &Path,
        agent: &str,
        harness: &str,
        model: &str,
    ) -> Result<()>;
    fn place_coordinator(&self, request: &CoordinatorRequest<'_>) -> Result<CoordinatorPlacement>;
    fn open_markdown(&self, path: &Path, focus: bool) -> Result<String>;
}

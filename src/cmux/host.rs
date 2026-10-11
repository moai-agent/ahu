//! Built-in implementation of the versioned interactive host contract.
use super::Cmux;
use crate::interactive::v1::{CreatedWorkspace, Group, Host, WorkspaceInfo, WorkspaceRequest};
use crate::util::Result;
use std::collections::BTreeMap;
use std::path::Path;

impl Host for Cmux {
    fn inherited_workspace(&self) -> Option<String> {
        super::inherited_workspace()
    }
    fn check_capabilities(&self) -> Result<Vec<String>> {
        Cmux::check_capabilities(self)
    }

    fn list_groups(&self, window_id: Option<&str>) -> Result<Vec<Group>> {
        Cmux::list_groups(self, window_id)
    }

    fn find_group(&self, group_id: &str, window_id: Option<&str>) -> Result<Option<Group>> {
        Cmux::find_group(self, group_id, window_id)
    }

    fn current_window(&self) -> Result<Option<String>> {
        Cmux::current_window(self)
    }

    fn window_for_workspace(&self, workspace_id: &str) -> Result<String> {
        Cmux::window_for_workspace(self, workspace_id)
    }

    fn add_workspace_to_group(
        &self,
        group_id: &str,
        workspace_id: &str,
        window_id: &str,
    ) -> Result<()> {
        Cmux::add_workspace_to_group(self, group_id, workspace_id, window_id)
    }

    fn current_workspace(&self) -> Result<Option<String>> {
        Cmux::current_workspace(self)
    }

    fn create_anchor_workspace(&self, group_id: &str, title: &str, cwd: &Path) -> Result<String> {
        Cmux::create_anchor_workspace(self, group_id, title, cwd)
    }

    fn set_anchor(&self, group_id: &str, workspace_id: &str) -> Result<()> {
        Cmux::set_anchor(self, group_id, workspace_id)
    }

    fn create_group(&self, name: &str, cwd: &Path) -> Result<Group> {
        Cmux::create_group(self, name, cwd)
    }

    fn expand_group(&self, group_id: &str) -> Result<()> {
        Cmux::expand_group(self, group_id)
    }

    fn create_task_workspace(&self, request: &WorkspaceRequest<'_>) -> Result<CreatedWorkspace> {
        Cmux::create_task_workspace(
            self,
            request.group_id,
            request.window_id,
            request.title,
            request.summary,
            request.cwd,
            request.startup_command,
            request.focus,
        )
    }

    fn open_markdown(&self, path: &Path, focus: bool) -> Result<String> {
        Cmux::open_markdown(self, path, focus)
    }

    fn select_workspace(&self, workspace_id: &str) -> Result<()> {
        Cmux::select_workspace(self, workspace_id)
    }

    fn close_workspace(&self, workspace_id: &str) -> Result<()> {
        Cmux::close_workspace(self, workspace_id)
    }

    fn set_status(&self, workspace_id: &str, value: &str) -> Result<()> {
        Cmux::set_status(self, workspace_id, value)
    }

    fn set_agent_metadata(
        &self,
        workspace_id: &str,
        agent: &str,
        harness: &str,
        model: &str,
    ) -> Result<()> {
        Cmux::set_agent_metadata(self, workspace_id, agent, harness, model)
    }

    fn workspaces(&self) -> Result<BTreeMap<String, WorkspaceInfo>> {
        Cmux::workspaces(self)
    }

    fn workspaces_in_window(
        &self,
        window: Option<&str>,
    ) -> Result<BTreeMap<String, WorkspaceInfo>> {
        Cmux::workspaces_in_window(self, window)
    }
}

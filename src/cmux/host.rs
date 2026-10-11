//! Compatibility wrapper delegating the V1 contract to the optional package.
use super::Cmux;
use crate::interactive::v1::{
    CreatedWorkspace, Group, Host, WorkspaceCreationResult, WorkspaceInfo, WorkspaceRequest,
};
use crate::util::Result;
use std::collections::BTreeMap;
use std::path::Path;

impl Host for Cmux {
    fn inherited_workspace(&self) -> Option<String> {
        ahu_cmux_plugin::inherited_workspace()
    }
    fn check_capabilities(&self) -> Result<Vec<String>> {
        self.0.check_capabilities()
    }

    fn list_groups(&self, window_id: Option<&str>) -> Result<Vec<Group>> {
        self.0.list_groups(window_id)
    }

    fn find_group(&self, group_id: &str, window_id: Option<&str>) -> Result<Option<Group>> {
        self.0.find_group(group_id, window_id)
    }

    fn current_window(&self) -> Result<Option<String>> {
        self.0.current_window()
    }

    fn window_for_workspace(&self, workspace_id: &str) -> Result<String> {
        self.0.window_for_workspace(workspace_id)
    }

    fn add_workspace_to_group(
        &self,
        group_id: &str,
        workspace_id: &str,
        window_id: &str,
    ) -> Result<()> {
        self.0
            .add_workspace_to_group(group_id, workspace_id, window_id)
    }

    fn current_workspace(&self) -> Result<Option<String>> {
        self.0.current_workspace()
    }

    fn create_anchor_workspace(&self, group_id: &str, title: &str, cwd: &Path) -> Result<String> {
        self.0.create_anchor_workspace(group_id, title, cwd)
    }

    fn set_anchor(&self, group_id: &str, workspace_id: &str) -> Result<()> {
        self.0.set_anchor(group_id, workspace_id)
    }

    fn create_group(&self, name: &str, cwd: &Path) -> Result<Group> {
        self.0.create_group(name, cwd)
    }

    fn expand_group(&self, group_id: &str) -> Result<()> {
        self.0.expand_group(group_id)
    }

    fn create_task_workspace(
        &self,
        request: &WorkspaceRequest<'_>,
    ) -> WorkspaceCreationResult<CreatedWorkspace> {
        self.0.create_task_workspace(
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
        self.0.open_markdown(path, focus)
    }

    fn select_workspace(&self, workspace_id: &str) -> Result<()> {
        self.0.select_workspace(workspace_id)
    }

    fn close_workspace(&self, workspace_id: &str) -> Result<()> {
        self.0.close_workspace(workspace_id)
    }

    fn set_status(&self, workspace_id: &str, value: &str) -> Result<()> {
        self.0.set_status(workspace_id, value)
    }

    fn set_agent_metadata(
        &self,
        workspace_id: &str,
        agent: &str,
        harness: &str,
        model: &str,
    ) -> Result<()> {
        self.0
            .set_agent_metadata(workspace_id, agent, harness, model)
    }

    fn workspaces(&self) -> Result<BTreeMap<String, WorkspaceInfo>> {
        self.0.workspaces()
    }

    fn workspaces_in_window(
        &self,
        window: Option<&str>,
    ) -> Result<BTreeMap<String, WorkspaceInfo>> {
        self.0.workspaces_in_window(window)
    }
}

//! The cmux integration.
//!
//! Verified against cmux 0.64.22 (102) [ddd4a01bc] on 2026-09-11.
//!
//! Queries use `cmux rpc <method> <json>` with validated structured responses.
//! Task creation uses `new-workspace --command` and identifies the result by
//! group membership because that CLI does not return JSON in this build.
//!
//! Presentation: one native workspace group per repository, one child workspace
//! per task. The group's anchor workspace *is* the group header row, so it is
//! reserved for repository context and never used for an agent task — otherwise
//! that task would have no visible row of its own.

mod host;
pub use ahu_host_api::v1::{CreatedWorkspace, Group, WorkspaceInfo};

use ahu_host_api::v1::{WorkspaceCreationError, WorkspaceCreationResult};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

macro_rules! bail {
    ($($arg:tt)*) => { return Err(Error::new(format!($($arg)*))) };
}
use ahu_host_api::{Error, Result};

/// Capabilities ahu needs before it will create anything.
const REQUIRED_CAPABILITIES: &[&str] = &[
    "workspace.groups.v1",
    "workspace.group_create.v1",
    "workspace.create_in_group.v1",
];

/// A cmux instance reachable over its socket.
pub struct Cmux {
    executable: PathBuf,
    socket_path: Option<String>,
}

impl Cmux {
    /// Activate using the executable already selected by core. Executes a ping.
    pub fn connect(executable: PathBuf) -> Result<Self> {
        let socket_path = std::env::var("CMUX_SOCKET_PATH")
            .ok()
            .filter(|s| !s.is_empty());
        let cmux = Self {
            executable,
            socket_path,
        };
        cmux.ping()
            .map_err(|error| error.with_kind(ahu_host_api::ErrorKind::Prerequisite))?;
        Ok(cmux)
    }

    fn ping(&self) -> Result<()> {
        let output = Command::new(&self.executable).arg("ping").output();
        match output {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => bail!(
                "cmux is installed but not responding: {}\n\
                 Start the cmux app, then run ahu again.",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => bail!(
                "cmux executable was not found.\n\
                 Native cmux workspace, group, and integration operations are unavailable.\n\
                 Install cmux, or set AHU_CMUX_BIN to its executable."
            ),
            Err(e) => bail!("cannot run cmux: {e}"),
        }
    }

    fn rpc(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
        let mut command = Command::new(&self.executable);
        command.arg("rpc").arg(method).arg(params.to_string());
        if let Some(socket) = &self.socket_path {
            command.env("CMUX_SOCKET_PATH", socket);
        }
        let output = command
            .output()
            .map_err(|e| Error::new(format!("cannot run cmux rpc {method}: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let detail = if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                stderr.trim()
            };
            bail!("cmux rpc {method} failed: {detail}");
        }
        serde_json::from_slice(&output.stdout).map_err(|e| {
            Error::new(format!(
                "cmux rpc {method} returned output ahu could not parse as JSON ({e}).\n\
                 This usually means the installed cmux speaks a different protocol than ahu expects."
            ))
        })
    }

    /// Confirm the installed build advertises the group operations ahu uses.
    pub fn check_capabilities(&self) -> Result<Vec<String>> {
        let output = Command::new(&self.executable)
            .arg("capabilities")
            .output()
            .map_err(|e| Error::new(format!("cannot run cmux capabilities: {e}")))?;
        if !output.status.success() {
            bail!(
                "cmux capabilities failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let advertised: Vec<String> = value
            .get("capabilities")
            .and_then(|c| c.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|i| i.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let missing: Vec<&str> = REQUIRED_CAPABILITIES
            .iter()
            .copied()
            .filter(|needed| !advertised.iter().any(|a| a == needed))
            .collect();
        if !missing.is_empty() {
            return Err(Error::new(format!(
                "this cmux build does not advertise: {}.\n\
                 ahu needs native workspace groups to give every task a visible row under its repository.",
                missing.join(", ")
            )).with_kind(ahu_host_api::ErrorKind::Prerequisite));
        }
        Ok(advertised)
    }

    /// List groups, optionally scoped to a window.
    ///
    /// Groups are window-scoped: a group created in one window is not visible
    /// from another. ahu stores the window a repository's group lives in so a
    /// later launch reuses it instead of creating a second group per window.
    pub fn list_groups(&self, window_id: Option<&str>) -> Result<Vec<Group>> {
        let params = match window_id {
            Some(window) => serde_json::json!({ "window_id": window }),
            None => serde_json::json!({}),
        };
        let value = match self.rpc("workspace.group.list", params) {
            Ok(value) => value,
            // Older builds may not accept a window filter; fall back rather than
            // failing the launch.
            Err(_) if window_id.is_some() => {
                self.rpc("workspace.group.list", serde_json::json!({}))?
            }
            Err(e) => return Err(e),
        };
        let items = value
            .get("groups")
            .and_then(|g| g.as_array())
            .ok_or_else(|| Error::new("cmux workspace.group.list returned no `groups` array"))?;
        items.iter().map(parse_group).collect()
    }

    pub fn find_group(&self, group_id: &str, window_id: Option<&str>) -> Result<Option<Group>> {
        Ok(self
            .list_groups(window_id)?
            .into_iter()
            .find(|g| g.id == group_id))
    }

    /// The window ahu is currently being invoked from, when it is inside cmux.
    pub fn current_window(&self) -> Result<Option<String>> {
        if let Ok(workspace) = std::env::var("CMUX_WORKSPACE_ID")
            && !workspace.is_empty()
        {
            return self.window_for_workspace(&workspace).map(Some);
        }
        let value = self.rpc("workspace.current", serde_json::json!({}))?;
        Ok(value
            .get("window_id")
            .and_then(|v| v.as_str())
            .map(str::to_string))
    }

    /// Resolve the caller's window rather than whichever window has focus.
    pub fn window_for_workspace(&self, workspace_id: &str) -> Result<String> {
        let value = self.rpc(
            "system.identify",
            serde_json::json!({ "caller": { "workspace_id": workspace_id } }),
        )?;
        value
            .pointer("/caller/window_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| Error::new("cmux could not locate the invoking workspace's window"))
    }

    /// Keep an existing terminal session in its repository's window and group.
    pub fn add_workspace_to_group(
        &self,
        group_id: &str,
        workspace_id: &str,
        window_id: &str,
    ) -> Result<()> {
        if self.window_for_workspace(workspace_id)? != window_id {
            self.rpc(
                "workspace.move_to_window",
                serde_json::json!({
                    "workspace_id": workspace_id, "window_id": window_id,
                }),
            )?;
        }
        self.rpc(
            "workspace.group.add",
            serde_json::json!({
                "group_id": group_id, "workspace_id": workspace_id, "window_id": window_id,
            }),
        )?;
        if !self
            .find_group(group_id, Some(window_id))?
            .is_some_and(|group| {
                group
                    .member_workspace_ids
                    .iter()
                    .any(|id| id == workspace_id)
            })
        {
            bail!("cmux did not place the invoking workspace in its repository group");
        }
        Ok(())
    }

    pub fn current_workspace(&self) -> Result<Option<String>> {
        if let Ok(id) = std::env::var("CMUX_WORKSPACE_ID") {
            return Ok(Some(id));
        }
        let value = self.rpc("workspace.current", serde_json::json!({}))?;
        Ok(value
            .get("workspace_id")
            .and_then(|v| v.as_str())
            .map(str::to_string))
    }

    /// Create a plain workspace in a group, used to restore a group's anchor.
    pub fn create_anchor_workspace(
        &self,
        group_id: &str,
        title: &str,
        cwd: &Path,
    ) -> Result<String> {
        let value = self.rpc(
            "workspace.create",
            serde_json::json!({
                "title": title,
                "cwd": cwd.to_string_lossy(),
                "group_id": group_id,
                "group_placement": "top",
                "focus": false,
            }),
        )?;
        value
            .get("workspace_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| Error::new("cmux workspace.create returned no `workspace_id`"))
    }

    /// Make `workspace_id` the group's anchor, so it becomes the header row.
    pub fn set_anchor(&self, group_id: &str, workspace_id: &str) -> Result<()> {
        self.rpc(
            "workspace.group.set_anchor",
            serde_json::json!({ "group_id": group_id, "workspace_id": workspace_id }),
        )?;
        Ok(())
    }

    pub fn create_group(&self, name: &str, cwd: &Path) -> Result<Group> {
        let window = self.current_window()?;
        let value = self.rpc(
            "workspace.group.create",
            serde_json::json!({ "name": name, "cwd": cwd.to_string_lossy(), "window_id": window }),
        )?;
        let group = value
            .get("group")
            .ok_or_else(|| Error::new("cmux workspace.group.create returned no `group`"))?;
        parse_group(group)
    }

    pub fn expand_group(&self, group_id: &str) -> Result<()> {
        self.rpc(
            "workspace.group.expand",
            serde_json::json!({ "group_id": group_id }),
        )?;
        Ok(())
    }

    /// Create a task workspace inside a group and return its identifiers.
    ///
    /// This is the one operation ahu performs through the `new-workspace` CLI
    /// rather than the RPC surface. Verified against cmux 0.64.22: the socket's
    /// `initial_input` on `workspace.create` is accepted but never executed,
    /// while the CLI's `--command` runs in the workspace's interactive shell and
    /// leaves that shell alive after the harness exits. The socket's
    /// `initial_command` also runs, but it replaces the shell with a one-shot
    /// process, so the terminal would disappear when the session ends.
    ///
    /// The CLI does not return JSON in this build, so the new workspace is
    /// identified by diffing group membership and confirming the working
    /// directory. Callers hold ahu's per-repository launch lock, but external
    /// clients can add members concurrently. Only a unique exact directory
    /// match is accepted; uncertain discovery never closes a workspace.
    ///
    /// `startup_command` is shell-interpreted. Callers must pass a command built
    /// by core’s `interactive::startup_command`, which contains only ahu-owned, shell-quoted
    /// paths — never a task prompt.
    #[allow(clippy::too_many_arguments)]
    pub fn create_task_workspace(
        &self,
        group_id: &str,
        window_id: Option<&str>,
        title: &str,
        summary: &str,
        cwd: &Path,
        startup_command: &str,
        focus: bool,
    ) -> WorkspaceCreationResult<CreatedWorkspace> {
        let before: std::collections::BTreeSet<String> = self
            .find_group(group_id, window_id)
            .map_err(WorkspaceCreationError::NotCreated)?
            .ok_or_else(|| {
                Error::new(format!(
                    "cmux group {group_id} disappeared before the task was created"
                ))
            })
            .map_err(WorkspaceCreationError::NotCreated)?
            .member_workspace_ids
            .into_iter()
            .collect();

        let cwd_string = cwd.to_string_lossy().to_string();
        let mut command = Command::new(&self.executable);
        command.args([
            "new-workspace",
            "--name",
            title,
            "--description",
            summary,
            "--cwd",
            &cwd_string,
            "--command",
            startup_command,
            "--group",
            group_id,
            "--group-placement",
            "end",
            "--focus",
            if focus { "true" } else { "false" },
        ]);
        if let Some(window) = window_id {
            command.args(["--window", window]);
        }
        if let Some(socket) = &self.socket_path {
            command.env("CMUX_SOCKET_PATH", socket);
        }
        // Separate spawn from wait: an output/read/wait error after spawn is not
        // proof that cmux never delivered the request to its server.
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                WorkspaceCreationError::NotCreated(Error::new(format!(
                    "cannot run cmux new-workspace: {e}"
                )))
            })?;
        let output = child.wait_with_output().map_err(|e| {
            WorkspaceCreationError::OutcomeUnknown(Error::new(format!(
                "cannot collect cmux new-workspace outcome: {e}"
            )))
        })?;
        if !output.status.success() {
            return Err(WorkspaceCreationError::OutcomeUnknown(Error::new(format!(
                "cmux new-workspace failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ))));
        }

        self.discover_task_workspace(
            &before,
            group_id,
            window_id,
            cwd,
            std::time::Duration::from_secs(10),
        )
        .map_err(WorkspaceCreationError::OutcomeUnknown)
    }

    fn discover_task_workspace(
        &self,
        before: &std::collections::BTreeSet<String>,
        group_id: &str,
        window_id: Option<&str>,
        cwd: &Path,
        timeout: std::time::Duration,
    ) -> Result<CreatedWorkspace> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let group = self.find_group(group_id, window_id)?.ok_or_else(|| {
                Error::new(format!(
                    "cmux group {group_id} disappeared while the task was starting"
                ))
            })?;
            let added: std::collections::BTreeSet<_> = group
                .member_workspace_ids
                .iter()
                .filter(|id| !before.contains(*id))
                .collect();
            if !added.is_empty() {
                // A sole new member can belong to a concurrent external launch.
                // Never infer ownership from cardinality, title, or CLI output.
                let listed = self.workspaces_in_window(window_id)?;
                let mine: Vec<_> = added
                    .into_iter()
                    .filter(|id| {
                        listed
                            .get(*id)
                            .is_some_and(|info| Path::new(&info.directory) == cwd)
                    })
                    .collect();
                if mine.len() == 1 {
                    return Ok(CreatedWorkspace {
                        workspace_id: mine[0].clone(),
                        window_id: window_id.unwrap_or_default().to_string(),
                        group_id: Some(group_id.to_string()),
                    });
                }
            }
            if std::time::Instant::now() >= deadline {
                bail!(
                    "cmux accepted the create request, but ahu could not uniquely identify a new \
                     workspace in group {group_id} with directory {}. Startup is unconfirmed; \
                     no workspace was closed.",
                    cwd.display()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
    }

    /// Open a coordinator harness in a fresh workspace under a repository group.
    /// This is used when the invoking terminal already anchors another group;
    /// cmux cannot move that anchor into a second group.
    pub fn create_coordinator_workspace(
        &self,
        group_id: &str,
        window_id: Option<&str>,
        title: &str,
        summary: &str,
        cwd: &Path,
        startup_command: &str,
    ) -> WorkspaceCreationResult<CreatedWorkspace> {
        self.create_task_workspace(
            group_id,
            window_id,
            title,
            summary,
            cwd,
            startup_command,
            true,
        )
    }

    /// Open a Markdown file in cmux's own Markdown viewer.
    ///
    /// The bundled viewer renders ```` ```mermaid ```` fences as diagrams and
    /// watches the file, so a regenerated document updates in place. Verified
    /// against cmux 0.64.22.
    pub fn open_markdown(&self, path: &Path, focus: bool) -> Result<String> {
        let value = self.rpc(
            "markdown.open",
            serde_json::json!({ "path": path.to_string_lossy(), "focus": focus }),
        )?;
        value
            .get("surface_id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| Error::new("cmux markdown.open returned no `surface_id`"))
    }

    pub fn select_workspace(&self, workspace_id: &str) -> Result<()> {
        self.rpc(
            "workspace.select",
            serde_json::json!({ "workspace_id": workspace_id }),
        )?;
        Ok(())
    }

    pub fn close_workspace(&self, workspace_id: &str) -> Result<()> {
        self.rpc(
            "workspace.close",
            serde_json::json!({ "workspace_id": workspace_id }),
        )?;
        Ok(())
    }

    /// Set a namespaced sidebar status pill for a task workspace.
    pub fn set_status(&self, workspace_id: &str, value: &str) -> Result<()> {
        self.set_status_entry(workspace_id, "ahu.task", value)
    }

    fn set_status_entry(&self, workspace_id: &str, key: &str, value: &str) -> Result<()> {
        let output = Command::new(&self.executable)
            .args(["set-status", key, value, "--workspace", workspace_id])
            .output()
            .map_err(|e| Error::new(format!("cannot run cmux set-status: {e}")))?;
        if !output.status.success() {
            bail!(
                "cmux set-status failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    /// Publish the ahu coordinator identity in cmux's sidebar metadata.
    ///
    /// cmux exposes status entries as the stable metadata surface available to
    /// the installed client. Keep the value compact and human-readable while
    /// retaining all three identity fields needed to identify a coordinator.
    pub fn set_agent_metadata(
        &self,
        workspace_id: &str,
        agent: &str,
        harness: &str,
        model: &str,
    ) -> Result<()> {
        self.set_status_entry(
            workspace_id,
            "ahu.agent",
            &format!("{agent} · {harness} · {model}"),
        )
    }

    /// All windows must be inspected before absence can mean a stale session.
    pub fn workspaces(&self) -> Result<BTreeMap<String, WorkspaceInfo>> {
        let value = self.rpc("window.list", serde_json::json!({}))?;
        let windows = value
            .get("windows")
            .and_then(|v| v.as_array())
            .ok_or_else(|| Error::new("cmux window.list returned no windows array"))?;
        let mut found = BTreeMap::new();
        for window in windows {
            let id = window
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::new("cmux window.list returned a window without an id"))?;
            found.extend(self.workspaces_in_window(Some(id))?);
        }
        Ok(found)
    }

    /// Explicit window scope for repository discovery and task identification.
    pub fn workspaces_in_window(
        &self,
        window: Option<&str>,
    ) -> Result<BTreeMap<String, WorkspaceInfo>> {
        let value = self.rpc("workspace.list", serde_json::json!({ "window_id": window }))?;
        let mut found = BTreeMap::new();
        let items = value
            .get("workspaces")
            .and_then(|w| w.as_array())
            .ok_or_else(|| Error::new("cmux workspace.list returned no workspaces array"))?;
        for item in items {
            let id = item.get("id").and_then(|v| v.as_str()).ok_or_else(|| {
                Error::new("cmux workspace.list returned a workspace without an id")
            })?;
            found.insert(
                id.to_string(),
                WorkspaceInfo {
                    directory: item
                        .get("current_directory")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    title: item
                        .get("custom_title")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                    description: item
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                },
            );
        }
        Ok(found)
    }
}

fn parse_group(value: &serde_json::Value) -> Result<Group> {
    let id = value
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::new("cmux group is missing `id`"))?
        .to_string();
    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let anchor_workspace_id = value
        .get("anchor_workspace_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::new("cmux group is missing `anchor_workspace_id`"))?
        .to_string();
    let member_workspace_ids = value
        .get("member_workspace_ids")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|i| i.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let is_collapsed = value
        .get("is_collapsed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(Group {
        id,
        name,
        anchor_workspace_id,
        member_workspace_ids,
        is_collapsed,
    })
}

/// The invoking cmux workspace, without contacting cmux.
pub fn inherited_workspace() -> Option<String> {
    std::env::var("CMUX_WORKSPACE_ID")
        .ok()
        .filter(|id| !id.is_empty())
}

#[cfg(test)]
mod presentation_tests {
    use super::*;
    use ahu_host_api::v1::{Host, WorkspaceRequest};
    fn shell_single_quote(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    #[cfg(unix)]
    fn fake_cmux(root: &Path, script: &str) -> Cmux {
        use std::os::unix::fs::PermissionsExt;
        let executable = root.join("fake-cmux");
        std::fs::write(&executable, format!("#!/bin/sh\n{script}\n")).unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&executable, permissions).unwrap();
        Cmux {
            executable,
            socket_path: None,
        }
    }

    #[test]
    fn group_parser_requires_identity_and_defaults_optional_fields() {
        let parsed = parse_group(&serde_json::json!({
            "id":"group-1", "anchor_workspace_id":"anchor",
            "member_workspace_ids":["anchor", 3, "task"], "is_collapsed":true
        }))
        .unwrap();
        assert_eq!(parsed.name, "");
        assert_eq!(parsed.member_workspace_ids, ["anchor", "task"]);
        assert!(parsed.is_collapsed);
        assert!(parse_group(&serde_json::json!({"anchor_workspace_id":"a"})).is_err());
        assert!(parse_group(&serde_json::json!({"id":"g"})).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn capability_check_accepts_required_group_support_and_explains_missing_support() {
        let root = tempfile::tempdir().unwrap();
        let supported = fake_cmux(
            root.path(),
            "printf '%s\\n' '{\"capabilities\":[\"workspace.groups.v1\",\"workspace.group_create.v1\",\"workspace.create_in_group.v1\",\"extra\"]}'",
        );
        let capabilities = supported.check_capabilities().unwrap();
        assert!(capabilities.contains(&"extra".to_string()));

        let unsupported = fake_cmux(root.path(), "printf '%s\\n' '{\"capabilities\":[]} '");
        assert!(
            unsupported
                .check_capabilities()
                .unwrap_err()
                .to_string()
                .contains("workspace.group_create.v1")
        );

        let failed = fake_cmux(root.path(), "echo unavailable >&2; exit 2");
        assert!(
            failed
                .check_capabilities()
                .unwrap_err()
                .to_string()
                .contains("unavailable")
        );
    }

    #[cfg(unix)]
    #[test]
    fn group_listing_falls_back_from_window_filter_and_validates_rpc_json() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("first-call");
        let script = format!(
            "if [ \\\"$2\\\" = workspace.group.list ] && [ ! -f {} ]; then touch {}; echo unsupported >&2; exit 1; fi\nprintf '%s\\n' '{{\"groups\":[{{\"id\":\"g\",\"anchor_workspace_id\":\"a\"}}]}}'",
            shell_single_quote(&marker.to_string_lossy()),
            shell_single_quote(&marker.to_string_lossy())
        );
        let client = fake_cmux(root.path(), &script);
        assert_eq!(
            client.find_group("g", Some("window")).unwrap().unwrap().id,
            "g"
        );
        assert!(client.find_group("missing", None).unwrap().is_none());

        let malformed = fake_cmux(root.path(), "echo not-json");
        assert!(
            malformed
                .list_groups(None)
                .unwrap_err()
                .to_string()
                .contains("could not parse as JSON")
        );
        let invalid_shape = fake_cmux(root.path(), "printf '%s\\n' '{}' ");
        assert!(
            invalid_shape
                .list_groups(None)
                .unwrap_err()
                .to_string()
                .contains("no `groups` array")
        );
    }

    #[cfg(unix)]
    fn creation_fixture(root: &Path, mode: &str) -> Cmux {
        use std::os::unix::fs::PermissionsExt;
        let executable = root.join("cmux");
        std::fs::write(&executable, format!(r#"#!/usr/bin/env python3
import json, os, signal, sys
from pathlib import Path
root = Path({root})
mode = {mode}
marker = root/'created'
if sys.argv[1] == 'new-workspace':
 marker.touch()
 if mode == 'command-failure': sys.exit(7)
 if mode == 'command-signal': os.kill(os.getpid(), signal.SIGTERM)
 sys.exit(0)
method = sys.argv[2]
if method == 'workspace.group.list':
 if mode == 'preflight-failure' or (marker.exists() and mode == 'group-failure'): sys.exit(8)
 if mode == 'spawn-failure': Path(sys.argv[0]).unlink()
 count_file = root/'polls'
 count = int(count_file.read_text())+1 if count_file.exists() else 1
 count_file.write_text(str(count))
 added = []
 if marker.exists():
  if mode != 'no-candidate': added = ['other']
  if mode in ['matching', 'duplicate-match'] or (mode == 'delayed-match' and count > 1): added += ['mine']
 print(json.dumps({{'groups':[{{'id':'group','anchor_workspace_id':'anchor','member_workspace_ids':['anchor']+added}}]}}))
elif method == 'workspace.list':
 if mode == 'lookup-failure': sys.exit(9)
 if mode == 'malformed-lookup': print('not-json'); sys.exit(0)
 other = str(root) if mode == 'duplicate-match' else str(root/'unrelated')
 rows = [{{'id':'other','current_directory':other}},{{'id':'mine','current_directory':str(root)}}]
 if mode == 'missing-directory': rows = [{{'id':'other'}}]
 print(json.dumps({{'workspaces':rows}}))
else:
 (root/'unexpected-mutation').touch()
 raise AssertionError(method)
"#, root=serde_json::to_string(root).unwrap(), mode=serde_json::to_string(mode).unwrap())).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        Cmux {
            executable,
            socket_path: None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn discovery_never_accepts_or_closes_unrelated_or_uncertain_candidates() {
        for mode in [
            "unrelated",
            "no-candidate",
            "missing-directory",
            "duplicate-match",
            "lookup-failure",
            "malformed-lookup",
            "group-failure",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let client = creation_fixture(temp.path(), mode);
            std::fs::write(temp.path().join("created"), "").unwrap();
            let result = client.discover_task_workspace(
                &["anchor".to_string()].into(),
                "group",
                Some("window"),
                temp.path(),
                std::time::Duration::ZERO,
            );
            assert!(result.is_err(), "{mode}: {result:?}");
            assert!(!temp.path().join("unexpected-mutation").exists(), "{mode}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn discovery_waits_past_a_sole_unrelated_candidate_for_the_matching_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let client = creation_fixture(temp.path(), "delayed-match");
        std::fs::write(temp.path().join("created"), "").unwrap();
        let created = client
            .discover_task_workspace(
                &["anchor".to_string()].into(),
                "group",
                Some("window"),
                temp.path(),
                std::time::Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(created.workspace_id, "mine");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("polls")).unwrap(),
            "2"
        );
        assert!(!temp.path().join("unexpected-mutation").exists());
    }

    #[cfg(unix)]
    #[test]
    fn creation_distinguishes_pre_start_failure_from_ambiguous_command_and_lookup_outcomes() {
        for mode in [
            "preflight-failure",
            "spawn-failure",
            "command-failure",
            "command-signal",
            "group-failure",
            "lookup-failure",
            "malformed-lookup",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let client = creation_fixture(temp.path(), mode);
            let error = client
                .create_task_workspace(
                    "group",
                    Some("window"),
                    "title",
                    "",
                    temp.path(),
                    "true",
                    false,
                )
                .unwrap_err();
            let not_created = matches!(mode, "preflight-failure" | "spawn-failure");
            assert_eq!(
                matches!(error, WorkspaceCreationError::NotCreated(_)),
                not_created,
                "{mode}: {error}"
            );
            assert_eq!(temp.path().join("created").exists(), !not_created, "{mode}");
            assert!(!temp.path().join("unexpected-mutation").exists(), "{mode}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_task_creation_identifies_the_workspace_in_the_target_window() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("cmux");
        let marker = temp.path().join("created");
        std::fs::write(&executable, format!(r#"#!/usr/bin/env python3
import json,sys
from pathlib import Path
marker = Path({marker})
if sys.argv[1] == 'new-workspace':
 assert sys.argv[sys.argv.index('--window')+1] == 'window-b'
 marker.touch(); sys.exit(0)
method,params = sys.argv[2],json.loads(sys.argv[3])
assert params['window_id'] == 'window-b'
if method == 'workspace.group.list':
 print(json.dumps({{'groups':[{{'id':'group-b','anchor_workspace_id':'anchor','member_workspace_ids':['anchor']+(['other','mine'] if marker.exists() else [])}}]}}))
elif method == 'workspace.list':
 print(json.dumps({{'workspaces':[{{'id':'other','current_directory':'/elsewhere'}},{{'id':'mine','current_directory':{cwd}}}]}}))
else: raise AssertionError(method)
"#, marker=serde_json::to_string(&marker).unwrap(), cwd=serde_json::to_string(temp.path()).unwrap())).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let client = Cmux {
            executable,
            socket_path: None,
        };
        let host: &dyn Host = &client;
        let created = host
            .create_task_workspace(&WorkspaceRequest {
                group_id: "group-b",
                window_id: Some("window-b"),
                title: "Task",
                summary: "",
                cwd: temp.path(),
                startup_command: "true",
                focus: false,
            })
            .unwrap();
        assert_eq!(created.workspace_id, "mine");
        assert_eq!(created.window_id, "window-b");
    }

    #[cfg(unix)]
    #[test]
    fn cmux_receives_description_as_one_literal_argument_and_identity_only() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("cmux");
        let log = temp.path().join("argv");
        let marker = temp.path().join("created");
        std::fs::write(&executable, format!(r#"#!/bin/sh
case "$1" in
rpc)
  if [ "$2" = workspace.list ]; then printf '%s\n' {inventory}; exit 0; fi
  if [ -f {marker} ]; then members='["anchor","task"]'; else members='["anchor"]'; fi
  printf '{{"groups":[{{"id":"group","anchor_workspace_id":"anchor","member_workspace_ids":%s}}]}}\n' "$members"
  ;;
new-workspace)
  printf '%s\n' "$@" > {log}
  touch {marker}
  ;;
set-status)
  printf '%s\n' "$@" >> {log}
  ;;
esac
"#, marker = shell_single_quote(&marker.to_string_lossy()), log = shell_single_quote(&log.to_string_lossy()), inventory = shell_single_quote(&serde_json::json!({"workspaces":[{"id":"task","current_directory":temp.path()}]}).to_string()))).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let client = Cmux {
            executable,
            socket_path: None,
        };
        let summary = "Useful text; $(false) 'quoted' --color=always";
        let host: &dyn Host = &client;
        let created = host
            .create_task_workspace(&WorkspaceRequest {
                group_id: "group",
                window_id: Some("window"),
                title: "Fix parser",
                summary,
                cwd: temp.path(),
                startup_command: "true",
                focus: false,
            })
            .unwrap();
        host.set_status(&created.workspace_id, "sable · model-a")
            .unwrap();
        let text = std::fs::read_to_string(log).unwrap();
        let args: Vec<_> = text.lines().collect();
        assert!(args.windows(2).any(|args| args == ["--name", "Fix parser"]));
        assert!(
            args.windows(2)
                .any(|args| args == ["--description", summary])
        );
        assert!(
            args.windows(3)
                .any(|args| args == ["set-status", "ahu.task", "sable · model-a"])
        );
        assert!(!args.contains(&"running"));
        assert!(!args.contains(&"starting"));
    }
}

//! The cmux integration.
//!
//! Verified against cmux 0.64.22 (102) [ddd4a01bc] on 2026-09-11.
//!
//! Everything goes through `cmux rpc <method> <json>`, which returns structured
//! JSON. The ordinary `new-workspace` CLI does not return JSON even with
//! `--json` in this build, so it is not used. Responses are validated field by
//! field rather than assumed.
//!
//! Presentation: one native workspace group per repository, one child workspace
//! per task. The group's anchor workspace *is* the group header row, so it is
//! reserved for repository context and never used for an agent task — otherwise
//! that task would have no visible row of its own.

pub mod integration;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::bail;
use crate::util::{Error, Result, shell_single_quote};

/// Capabilities ahu needs before it will create anything.
const REQUIRED_CAPABILITIES: &[&str] = &[
    "workspace.groups.v1",
    "workspace.group_create.v1",
    "workspace.create_in_group.v1",
];

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

/// A cmux instance reachable over its socket.
pub struct Cmux {
    executable: PathBuf,
    socket_path: Option<String>,
}

/// Resolve the native CLI independently of application/socket reachability.
pub fn resolve_executable() -> Result<PathBuf> {
    match std::env::var_os("AHU_CMUX_BIN") {
        Some(path) => {
            let path = PathBuf::from(path);
            if path.is_absolute() {
                Ok(path.canonicalize().unwrap_or(path))
            } else if path.as_os_str().as_encoded_bytes().contains(&b'/') {
                let path = std::env::current_dir()?.join(path);
                Ok(path.canonicalize().unwrap_or(path))
            } else {
                resolve_explicit_name(&path)
            }
        }
        None => crate::selection::resolve_utility("cmux").map_err(|error| {
            Error::new(format!(
                "{error}\nInstall cmux, or set AHU_CMUX_BIN to its executable."
            ))
            .with_kind(crate::util::ErrorKind::Prerequisite)
        }),
    }
}

/// Explicit user selection retains PATH semantics, including relative entries
/// and repository-local commands. The implicit resolver remains restricted.
fn resolve_explicit_name(name: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let cwd = std::env::current_dir()?;
    let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
    for directory in std::env::split_paths(&path) {
        let candidate = cwd.join(directory).join(name);
        if std::fs::metadata(&candidate)
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        {
            return candidate.canonicalize().map_err(Error::from);
        }
    }
    Err(Error::new(
        "explicit AHU_CMUX_BIN command was not found on PATH",
    ))
}

impl Cmux {
    /// Locate cmux, preferring the socket cmux itself told us about.
    ///
    /// The installed build's socket is under the user's state directory, not the
    /// legacy `/tmp/cmux.sock` from the generic documentation, so the path is
    /// never hard-coded: it comes from `CMUX_SOCKET_PATH` when ahu is running
    /// inside a cmux terminal, and otherwise from cmux's own resolver.
    pub fn discover() -> Result<Self> {
        let executable = resolve_executable()?;
        let socket_path = std::env::var("CMUX_SOCKET_PATH")
            .ok()
            .filter(|s| !s.is_empty());
        let cmux = Cmux {
            executable,
            socket_path,
        };
        cmux.ping()
            .map_err(|error| error.with_kind(crate::util::ErrorKind::Prerequisite))?;
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
                "cmux was not found on PATH.\n\
                 ahu organises task sessions in cmux, so it cannot launch without it.\n\
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
            )).with_kind(crate::util::ErrorKind::Prerequisite));
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
    /// directory. Callers hold ahu's per-repository launch lock, and the task
    /// worktree path is unique, so the match is unambiguous.
    ///
    /// `startup_command` is shell-interpreted. Callers must pass a command built
    /// by [`startup_command`], which contains only ahu-owned, shell-quoted
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
    ) -> Result<CreatedWorkspace> {
        let before: std::collections::BTreeSet<String> = self
            .find_group(group_id, window_id)?
            .ok_or_else(|| {
                Error::new(format!(
                    "cmux group {group_id} disappeared before the task was created"
                ))
            })?
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
        let output = command
            .output()
            .map_err(|e| Error::new(format!("cannot run cmux new-workspace: {e}")))?;
        if !output.status.success() {
            bail!(
                "cmux new-workspace failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let group = self.find_group(group_id, window_id)?.ok_or_else(|| {
                Error::new(format!(
                    "cmux group {group_id} disappeared while the task was starting"
                ))
            })?;
            let added: Vec<String> = group
                .member_workspace_ids
                .iter()
                .filter(|id| !before.contains(*id))
                .cloned()
                .collect();
            match added.len() {
                0 => {}
                1 => {
                    return Ok(CreatedWorkspace {
                        workspace_id: added[0].clone(),
                        window_id: window_id.unwrap_or_default().to_string(),
                        group_id: Some(group_id.to_string()),
                    });
                }
                _ => {
                    // Someone else added a workspace to this group at the same
                    // moment. Pick the one sitting in this task's worktree.
                    let listed = self.workspaces()?;
                    let mine: Vec<String> = added
                        .into_iter()
                        .filter(|id| {
                            listed
                                .get(id)
                                .map(|info| Path::new(&info.directory) == cwd)
                                .unwrap_or(false)
                        })
                        .collect();
                    if mine.len() == 1 {
                        return Ok(CreatedWorkspace {
                            workspace_id: mine[0].clone(),
                            window_id: window_id.unwrap_or_default().to_string(),
                            group_id: Some(group_id.to_string()),
                        });
                    }
                    bail!(
                        "cmux created the task workspace, but ahu could not identify it among {} \
                         new members of group {group_id}. The session is running; find it in the \
                         sidebar under this repository.",
                        mine.len().max(2)
                    );
                }
            }
            if std::time::Instant::now() >= deadline {
                bail!(
                    "cmux accepted the task workspace but it did not appear in group {group_id}."
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
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
        let output = Command::new(&self.executable)
            .args(["set-status", "ahu.task", value, "--workspace", workspace_id])
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

    /// Which workspaces currently exist, by id.
    pub fn workspaces(&self) -> Result<BTreeMap<String, WorkspaceInfo>> {
        let window = self.current_window()?;
        let value = self.rpc("workspace.list", serde_json::json!({ "window_id": window }))?;
        let mut found = BTreeMap::new();
        if let Some(items) = value.get("workspaces").and_then(|w| w.as_array()) {
            for item in items {
                if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
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
            }
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

#[cfg(test)]
mod presentation_tests {
    use super::*;

    #[test]
    fn title_budget_belongs_to_the_task() {
        assert_eq!(
            workspace_title("agent@1.2.3", "## **Fix parser**"),
            "Fix parser"
        );
        let title = workspace_title(&"agent".repeat(100), &"界🦀".repeat(100));
        assert_eq!(title.chars().count(), 60);
        assert!(title.ends_with('…'));
        assert!(!title.contains("agent"));
        assert_eq!(workspace_identity("sable", "model-a"), "sable · model-a");
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
"#, marker = shell_single_quote(&marker.to_string_lossy()), log = shell_single_quote(&log.to_string_lossy()))).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let client = Cmux {
            executable,
            socket_path: None,
        };
        let summary = "Useful text; $(false) 'quoted' --color=always";
        let created = client
            .create_task_workspace(
                "group",
                Some("window"),
                "Fix parser",
                summary,
                temp.path(),
                "true",
                false,
            )
            .unwrap();
        client
            .set_status(
                &created.workspace_id,
                &workspace_identity("sable", "model-a"),
            )
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

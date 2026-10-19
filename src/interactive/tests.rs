//! Synthetic host tests reuse the frozen launch fixture, without a live terminal,
//! native harness, or additional Git worktree.
use super::*;
use crate::interactive::repository::RepositoryManager;
use crate::interactive::v1::{
    CreatedWorkspace, Group, Host, WorkspaceCreationResult, WorkspaceInfo, WorkspaceRequest,
};
use std::cell::RefCell;
use std::collections::BTreeMap;

struct SyntheticHost {
    coordinator: Option<(String, Vec<Group>)>,
    groups: Vec<Group>,
    groups_for_window: Option<Vec<Group>>,
    window: String,
    workspaces: BTreeMap<String, WorkspaceInfo>,
    inventory_error: bool,
    close_error: bool,
    groups_error: bool,
    window_error: bool,
    created: CreatedWorkspace,
    mutations: RefCell<Vec<String>>,
    requests: RefCell<Vec<(String, String, bool)>>,
}

impl SyntheticHost {
    fn new(repo: &Repo) -> Self {
        Self {
            coordinator: None,
            groups: vec![Group {
                id: "group".into(),
                name: repo.display_name(),
                anchor_workspace_id: "anchor".into(),
                member_workspace_ids: vec!["task-workspace".into()],
                is_collapsed: false,
            }],
            groups_for_window: None,
            window: "window".into(),
            workspaces: BTreeMap::from([(
                "task-workspace".into(),
                WorkspaceInfo {
                    directory: repo.root.to_string_lossy().into_owned(),
                    title: None,
                    description: None,
                },
            )]),
            inventory_error: false,
            close_error: false,
            groups_error: false,
            window_error: false,
            created: CreatedWorkspace {
                workspace_id: "new-workspace".into(),
                window_id: "window".into(),
                group_id: Some("group".into()),
            },
            mutations: RefCell::new(Vec::new()),
            requests: RefCell::new(Vec::new()),
        }
    }
}

// Unexpected operations fail the test, rather than silently simulating success.
impl Host for SyntheticHost {
    fn inherited_workspace(&self) -> Option<String> {
        self.coordinator.as_ref().map(|_| "caller".into())
    }
    fn check_capabilities(&self) -> Result<Vec<String>> {
        if self.coordinator.is_some() {
            return Ok(Vec::new());
        }
        panic!("unexpected capability probe")
    }
    fn list_groups(&self, window: Option<&str>) -> Result<Vec<Group>> {
        if self.groups_error {
            bail!("synthetic group query failed");
        }
        if let Some((source_window, groups)) = &self.coordinator
            && window == Some(source_window.as_str())
        {
            return Ok(groups.clone());
        }
        if window == Some(self.window.as_str())
            && let Some(groups) = &self.groups_for_window
        {
            return Ok(groups.clone());
        }
        Ok(self.groups.clone())
    }
    fn find_group(&self, id: &str, window: Option<&str>) -> Result<Option<Group>> {
        Ok(self
            .list_groups(window)?
            .into_iter()
            .find(|group| group.id == id))
    }
    fn current_window(&self) -> Result<Option<String>> {
        if let Some((window, _)) = &self.coordinator {
            return Ok(Some(window.clone()));
        }
        Ok(Some(self.window.clone()))
    }
    fn current_workspace(&self) -> Result<Option<String>> {
        Ok(self.inherited_workspace())
    }
    fn window_for_workspace(&self, workspace: &str) -> Result<String> {
        if self.window_error {
            bail!("synthetic window query failed");
        }
        if workspace == "caller"
            && let Some((window, _)) = &self.coordinator
        {
            return Ok(window.clone());
        }
        Ok(self.window.clone())
    }
    fn workspaces(&self) -> Result<BTreeMap<String, WorkspaceInfo>> {
        if self.inventory_error {
            bail!("synthetic inventory incomplete");
        }
        Ok(self.workspaces.clone())
    }
    fn workspaces_in_window(&self, _: Option<&str>) -> Result<BTreeMap<String, WorkspaceInfo>> {
        self.workspaces()
    }
    fn create_task_workspace(
        &self,
        request: &WorkspaceRequest<'_>,
    ) -> WorkspaceCreationResult<CreatedWorkspace> {
        assert_eq!(request.group_id, "group");
        assert_eq!(request.window_id, Some("window"));
        self.requests.borrow_mut().push((
            request.summary.into(),
            request.startup_command.into(),
            request.focus,
        ));
        Ok(self.created.clone())
    }
    fn select_workspace(&self, id: &str) -> Result<()> {
        self.mutations.borrow_mut().push(format!("focus:{id}"));
        Ok(())
    }
    fn close_workspace(&self, id: &str) -> Result<()> {
        if self.close_error {
            bail!("synthetic close failed");
        }
        self.mutations.borrow_mut().push(format!("close:{id}"));
        Ok(())
    }
    fn set_status(&self, id: &str, status: &str) -> Result<()> {
        self.mutations
            .borrow_mut()
            .push(format!("status:{id}:{status}"));
        Ok(())
    }
    fn expand_group(&self, id: &str) -> Result<()> {
        self.mutations.borrow_mut().push(format!("expand:{id}"));
        Ok(())
    }
    fn add_workspace_to_group(&self, group: &str, workspace: &str, window: &str) -> Result<()> {
        if self.coordinator.is_some() {
            self.mutations
                .borrow_mut()
                .push(format!("move:{workspace}:{group}:{window}"));
            return Ok(());
        }
        panic!("unexpected move")
    }
    fn create_anchor_workspace(&self, _: &str, _: &str, _: &Path) -> Result<String> {
        panic!("unexpected anchor")
    }
    fn set_anchor(&self, _: &str, _: &str) -> Result<()> {
        panic!("unexpected anchor change")
    }
    fn create_group(&self, _: &str, _: &Path) -> Result<Group> {
        panic!("unexpected group creation")
    }
    fn open_markdown(&self, _: &Path, _: bool) -> Result<String> {
        panic!("unexpected document open")
    }
    fn set_agent_metadata(&self, workspace: &str, _: &str, _: &str, _: &str) -> Result<()> {
        if self.coordinator.is_some() {
            self.mutations
                .borrow_mut()
                .push(format!("metadata:{workspace}"));
            return Ok(());
        }
        panic!("unexpected metadata")
    }
}

#[test]
fn task_ownership_uses_the_recorded_window_when_resolving_its_group() {
    let (_temp, repo, loaded, plan) = fixture();
    let mut record = prepared_record(&repo, &loaded, &plan, PROMPT, Default::default());
    record.cmux_workspace_id = Some("task-workspace".into());
    record.cmux_group_id = Some("group".into());
    record.cmux_window_id = Some("window".into());

    let mut host = SyntheticHost::new(&repo);
    let window_group = host.groups[0].clone();
    host.groups.push(Group {
        id: "other-window-group".into(),
        name: "other window".into(),
        anchor_workspace_id: "other-anchor".into(),
        member_workspace_ids: vec!["task-workspace".into()],
        is_collapsed: false,
    });
    host.groups_for_window = Some(vec![window_group]);

    let manager = RepositoryManager::new(&host, &repo);
    manager.select_task_workspace(&record).unwrap();
    assert_eq!(*host.mutations.borrow(), ["focus:task-workspace"]);
}

#[test]
fn host_boundary_preserves_ownership_checks_before_every_task_mutation() {
    let (_temp, repo, loaded, plan) = fixture();
    let mut record = prepared_record(&repo, &loaded, &plan, PROMPT, Default::default());
    record.cmux_workspace_id = Some("task-workspace".into());
    record.cmux_group_id = Some("group".into());
    record.cmux_window_id = Some("window".into());
    for case in [
        "valid",
        "no-owner",
        "ambiguous-owner",
        "wrong-group",
        "wrong-window",
        "inventory-error",
        "groups-error",
        "window-error",
    ] {
        let mut host = SyntheticHost::new(&repo);
        match case {
            "no-owner" => host.groups.clear(),
            "ambiguous-owner" => host.groups.push(host.groups[0].clone()),
            "wrong-group" => host.groups[0].id = "unrelated".into(),
            "wrong-window" => host.window = "unrelated".into(),
            "inventory-error" => host.inventory_error = true,
            "groups-error" => host.groups_error = true,
            "window-error" => host.window_error = true,
            _ => {}
        }
        let manager = RepositoryManager::new(&host, &repo);
        for result in [
            manager.select_task_workspace(&record),
            manager.close_task_workspace(&record).map(|_| ()),
            manager.set_task_status(&record, "waiting"),
            manager.expand_task_group(&record),
        ] {
            assert_eq!(result.is_ok(), case == "valid", "{case}: {result:?}");
        }
        let actual = host.mutations.borrow();
        if case == "valid" {
            assert_eq!(
                *actual,
                [
                    "focus:task-workspace",
                    "close:task-workspace",
                    "status:task-workspace:waiting",
                    "expand:group"
                ]
            );
        } else {
            assert!(actual.is_empty(), "{case}: {actual:?}");
        }
    }
}

#[cfg(all(unix, feature = "cmux-plugin"))]
#[test]
fn cmux_scoped_group_query_failure_blocks_every_task_mutation() {
    use std::os::unix::fs::PermissionsExt;

    let (temp, repo, loaded, plan) = fixture();
    let mut record = prepared_record(&repo, &loaded, &plan, PROMPT, Default::default());
    record.cmux_workspace_id = Some("task-workspace".into());
    record.cmux_group_id = Some("group".into());
    record.cmux_window_id = Some("window".into());
    let executable = temp.path().join("fake-cmux");
    let mutation = temp.path().join("mutated");
    for response in ["echo scoped-query-failed >&2; exit 7", "echo not-json"] {
        std::fs::write(
            &executable,
            format!(
                r#"#!/bin/sh
if [ "$1" = ping ]; then exit 0; fi
case "$2" in
  window.list) printf '%s\n' '{{"windows":[{{"id":"window"}}]}}' ;;
  workspace.list) printf '%s\n' '{{"workspaces":[{{"id":"task-workspace"}}]}}' ;;
  workspace.group.list)
    if [ "$3" = '{{"window_id":"window"}}' ]; then
      {response}
    else
      printf '%s\n' '{{"groups":[{{"id":"group","anchor_workspace_id":"anchor","member_workspace_ids":["task-workspace"]}}]}}'
    fi ;;
  system.identify) printf '%s\n' '{{"caller":{{"window_id":"window"}}}}' ;;
  *) touch {mutation}; printf '%s\n' '{{}}' ;;
esac
"#,
                mutation = crate::util::shell_single_quote(&mutation.to_string_lossy()),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let host = ahu_cmux_plugin::Cmux::connect(executable.clone()).unwrap();
        let manager = RepositoryManager::new(&host, &repo);
        for result in [
            manager.select_task_workspace(&record),
            manager.close_task_workspace(&record).map(|_| ()),
            manager.set_task_status(&record, "waiting"),
            manager.expand_task_group(&record),
        ] {
            assert!(
                result.is_err(),
                "scoped query failure permitted mutation: {result:?}"
            );
        }
        assert!(!mutation.exists());
    }
}

#[test]
fn host_boundary_distinguishes_missing_workspace_from_failed_inventory() {
    let (_temp, repo, loaded, plan) = fixture();
    let mut record = prepared_record(&repo, &loaded, &plan, PROMPT, Default::default());
    let mut host = SyntheticHost::new(&repo);
    // Headless/legacy records without a workspace must not contact the host.
    host.inventory_error = true;
    let manager = RepositoryManager::new(&host, &repo);
    assert!(!manager.close_task_workspace(&record).unwrap());
    manager.set_task_status(&record, "waiting").unwrap();
    manager.expand_task_group(&record).unwrap();
    assert!(manager.select_task_workspace(&record).is_err());
    record.cmux_workspace_id = Some("missing".into());
    assert!(manager.close_task_workspace(&record).is_err());
    host.inventory_error = false;
    let manager = RepositoryManager::new(&host, &repo);
    assert!(!manager.close_task_workspace(&record).unwrap());
    assert!(host.mutations.borrow().is_empty());
}

#[test]
fn host_boundary_keeps_creation_routing_and_preserves_inconsistent_workspace() {
    let (_temp, repo, _, plan) = fixture();
    state::write_json(
        &mapping_path(&repo).unwrap(),
        &GroupMapping {
            group_id: Some("group".into()),
            window_id: Some("window".into()),
            anchor_workspace_id: Some("anchor".into()),
        },
    )
    .unwrap();
    let startup =
        interactive::startup_command(Path::new("/synthetic/ahu's binary"), &plan.task_dir);
    let summary = "literal $(do-not-execute) 'summary'";
    for case in ["valid", "wrong-group", "no-group", "wrong-window"] {
        let mut host = SyntheticHost::new(&repo);
        match case {
            "wrong-group" => host.created.group_id = Some("unrelated".into()),
            "no-group" => host.created.group_id = None,
            "wrong-window" => host.created.window_id = "unrelated".into(),
            _ => {}
        }
        let manager = RepositoryManager::new(&host, &repo);
        let result = manager.create_task_workspace(
            "title",
            summary,
            &plan.worktree,
            &startup,
            false,
            &mut Vec::new(),
        );
        assert_eq!(result.is_ok(), case == "valid", "{case}: {result:?}");
        assert_eq!(
            *host.requests.borrow(),
            [(summary.into(), startup.clone(), false)]
        );
        if case == "valid" {
            assert_eq!(result.unwrap().workspace.workspace_id, "new-workspace");
            assert!(host.mutations.borrow().is_empty());
        } else {
            assert!(matches!(
                result,
                Err(interactive::v1::WorkspaceCreationError::OutcomeUnknown(_))
            ));
            assert!(host.mutations.borrow().is_empty());
        }
    }
}

#[test]
fn coordinator_without_inherited_workspace_does_not_probe_or_mutate_host() {
    let (_temp, repo, _, _) = fixture();
    let host = SyntheticHost::new(&repo);
    let placement = RepositoryManager::new(&host, &repo)
        .group_coordinator("synthetic-harness", "synthetic", "codex", "gpt-6", &[])
        .unwrap();
    assert!(!placement.opened_workspace);
    assert!(placement.notes.is_empty());
    assert!(host.mutations.borrow().is_empty());
    assert!(host.requests.borrow().is_empty());
}

#[test]
fn coordinator_preserves_other_groups_in_the_callers_window() {
    let (_temp, repo, _, _) = fixture();
    state::write_json(
        &mapping_path(&repo).unwrap(),
        &GroupMapping {
            group_id: Some("group".into()),
            window_id: Some("window".into()),
            anchor_workspace_id: Some("anchor".into()),
        },
    )
    .unwrap();
    for source_window in ["window", "other-window"] {
        for grouped in [true, false] {
            let mut host = SyntheticHost::new(&repo);
            let mut source_groups = if source_window == "window" {
                host.groups.clone()
            } else {
                Vec::new()
            };
            if grouped {
                source_groups.push(Group {
                    id: "unrelated-group".into(),
                    name: "unrelated repository".into(),
                    anchor_workspace_id: "caller".into(),
                    member_workspace_ids: vec!["caller".into(), "unrelated-session".into()],
                    is_collapsed: false,
                });
            }
            host.coordinator = Some((source_window.into(), source_groups));
            let placement = RepositoryManager::new(&host, &repo)
                .group_coordinator("/usr/bin/true", "synthetic", "codex", "fixture", &[])
                .unwrap();
            assert_eq!(
                placement.opened_workspace,
                grouped,
                "{source_window}, grouped={grouped}: {:?}",
                host.mutations.borrow()
            );
            if grouped {
                assert_eq!(*host.mutations.borrow(), ["metadata:new-workspace"]);
                assert_eq!(host.requests.borrow().len(), 1);
            } else {
                assert_eq!(
                    *host.mutations.borrow(),
                    [
                        "move:caller:group:window",
                        "metadata:caller",
                        "expand:group"
                    ]
                );
                assert!(host.requests.borrow().is_empty());
            }
        }
    }
}

#[test]
fn cancellation_cleanup_uses_ownership_published_before_confirmation() {
    cancellation_cleanup_for_publication_order(true);
}

#[test]
fn cancellation_cleanup_hands_off_ownership_published_after_confirmation() {
    cancellation_cleanup_for_publication_order(false);
}

fn cancellation_cleanup_for_publication_order(ownership_first: bool) {
    for case in [
        "valid",
        "no-owner",
        "ambiguous-owner",
        "wrong-group",
        "wrong-window",
        "inventory-error",
        "groups-error",
        "window-error",
        "close-error",
    ] {
        let (_temp, repo, loaded, plan) = fixture();
        let mut launcher = prepared_record(&repo, &loaded, &plan, PROMPT, Default::default());
        let mut canceller = launcher.clone();
        std::fs::create_dir_all(&plan.worktree).unwrap();
        task::save(&plan.task_dir, &launcher, PROMPT).unwrap();
        let marker = plan.worktree.join("keep.txt");
        std::fs::write(&marker, "keep work").unwrap();
        let mut host = SyntheticHost::new(&repo);
        let placement = interactive::repository::ManagedTaskWorkspace {
            group: host.groups[0].clone(),
            workspace: CreatedWorkspace {
                workspace_id: "task-workspace".into(),
                window_id: "window".into(),
                group_id: Some("group".into()),
            },
        };
        match case {
            "no-owner" => host.groups.clear(),
            "ambiguous-owner" => host.groups.push(host.groups[0].clone()),
            "wrong-group" => host.groups[0].id = "unrelated".into(),
            "wrong-window" => host.window = "unrelated".into(),
            "inventory-error" => host.inventory_error = true,
            "groups-error" => host.groups_error = true,
            "window-error" => host.window_error = true,
            "close-error" => host.close_error = true,
            _ => {}
        }
        let manager = RepositoryManager::new(&host, &repo);
        if ownership_first {
            // Publication happens after cancel's initial read, before its
            // confirmation read. Cleanup must use the refreshed ownership.
            record_created_workspace(
                &plan.task_dir,
                &mut launcher,
                "group",
                "task-workspace",
                "window",
            )
            .unwrap();
        }
        task::set_state(&plan.task_dir, TaskState::Cancelled).unwrap();
        let confirmation = crate::commands::confirm_interactive_cancellation(
            &plan.task_dir,
            &mut canceller,
            Duration::ZERO,
            |current| manager.close_task_workspace(current).map(|_| ()),
        );
        if ownership_first {
            if case == "valid" {
                assert_eq!(confirmation.unwrap(), ("confirmed", "closed"));
            } else {
                assert!(
                    confirmation
                        .unwrap_err()
                        .to_string()
                        .contains("close failed")
                );
            }
            assert_eq!(canceller, task::load(&plan.task_dir).unwrap());
        } else {
            assert_eq!(confirmation.unwrap(), ("confirmed", "absent"));
            // The cancel command has already returned. The launcher must
            // finish cleanup after publishing the workspace's ownership.
            let mut notes = Vec::new();
            finish_workspace_launch(
                &plan.task_dir,
                &mut launcher,
                &manager,
                &placement,
                &mut notes,
            );
            if case == "valid" {
                assert!(notes.is_empty(), "{notes:?}");
            } else {
                assert!(
                    notes.iter().any(|n| n.contains("close failed")),
                    "{notes:?}"
                );
            }
            assert_eq!(launcher, task::load(&plan.task_dir).unwrap());
        }
        let mutations = host.mutations.borrow();
        if case == "valid" {
            assert_eq!(*mutations, ["close:task-workspace"]);
        } else {
            assert!(mutations.is_empty(), "{case}: {mutations:?}");
        }
        let durable = task::load(&plan.task_dir).unwrap();
        assert_eq!(durable.state, TaskState::Cancelled);
        assert_eq!(durable.cmux_workspace_id.as_deref(), Some("task-workspace"));
        assert_eq!(durable.cmux_group_id.as_deref(), Some("group"));
        assert_eq!(durable.cmux_window_id.as_deref(), Some("window"));
        assert_eq!(std::fs::read_to_string(marker).unwrap(), "keep work");
        assert_eq!(task::load_prompt(&plan.task_dir).unwrap(), PROMPT);
    }
}

#[test]
fn cancellation_cleanup_preserves_unconfirmed_and_other_terminal_outcomes() {
    for outcome in [TaskState::Running, TaskState::Failed, TaskState::Exited] {
        let (_temp, repo, loaded, plan) = fixture();
        let mut launcher = prepared_record(&repo, &loaded, &plan, PROMPT, Default::default());
        let mut canceller = launcher.clone();
        std::fs::create_dir_all(&plan.worktree).unwrap();
        task::save(&plan.task_dir, &launcher, PROMPT).unwrap();
        task::set_state(&plan.task_dir, outcome).unwrap();
        let host = SyntheticHost::new(&repo);
        let manager = RepositoryManager::new(&host, &repo);
        let placement = interactive::repository::ManagedTaskWorkspace {
            group: host.groups[0].clone(),
            workspace: CreatedWorkspace {
                workspace_id: "task-workspace".into(),
                window_id: "window".into(),
                group_id: Some("group".into()),
            },
        };
        finish_workspace_launch(
            &plan.task_dir,
            &mut launcher,
            &manager,
            &placement,
            &mut Vec::new(),
        );
        assert!(
            !host
                .mutations
                .borrow()
                .iter()
                .any(|m| m.starts_with("close:"))
        );
        let (cancellation, workspace) = crate::commands::confirm_interactive_cancellation(
            &plan.task_dir,
            &mut canceller,
            Duration::ZERO,
            |_| panic!("unconfirmed cancellation must not close a workspace"),
        )
        .unwrap();
        assert_eq!(
            cancellation,
            if outcome.is_live() {
                "requested-unconfirmed"
            } else {
                "finished-on-its-own"
            }
        );
        assert_eq!(workspace, "left open");
        assert_eq!(canceller, task::load(&plan.task_dir).unwrap());
        assert_eq!(canceller.state, outcome);
        state::write_private_file(&plan.task_dir.join("task.json"), b"{}").unwrap();
        assert_eq!(
            crate::commands::confirm_interactive_cancellation(
                &plan.task_dir,
                &mut canceller,
                Duration::ZERO,
                |_| panic!("unreadable outcome must not close a workspace"),
            )
            .unwrap(),
            ("requested-unconfirmed", "left open")
        );
    }
}

#[test]
fn cancellation_cleanup_preserves_workspace_when_publication_cannot_be_verified() {
    let (_temp, repo, loaded, plan) = fixture();
    let mut record = prepared_record(&repo, &loaded, &plan, PROMPT, Default::default());
    std::fs::create_dir_all(&plan.worktree).unwrap();
    task::save(&plan.task_dir, &record, PROMPT).unwrap();
    let host = SyntheticHost::new(&repo);
    let manager = RepositoryManager::new(&host, &repo);
    let placement = interactive::repository::ManagedTaskWorkspace {
        group: host.groups[0].clone(),
        workspace: CreatedWorkspace {
            workspace_id: "task-workspace".into(),
            window_id: "window".into(),
            group_id: Some("group".into()),
        },
    };
    // Even an in-memory cancellation outcome cannot authorize cleanup if
    // publication cannot reload and persist verified durable ownership.
    record.state = TaskState::Cancelled;
    state::write_private_file(&plan.task_dir.join("task.json"), b"{}").unwrap();
    let mut notes = Vec::new();
    finish_workspace_launch(
        &plan.task_dir,
        &mut record,
        &manager,
        &placement,
        &mut notes,
    );
    assert!(host.mutations.borrow().is_empty());
    assert_eq!(notes.len(), 1);
    assert!(notes[0].contains("task record could not be updated"));
    assert_eq!(
        std::fs::read(plan.task_dir.join("task.json")).unwrap(),
        b"{}"
    );
    assert_eq!(task::load_prompt(&plan.task_dir).unwrap(), PROMPT);
}

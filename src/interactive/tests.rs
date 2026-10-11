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
    groups: Vec<Group>,
    groups_for_window: Option<Vec<Group>>,
    window: String,
    workspaces: BTreeMap<String, WorkspaceInfo>,
    inventory_error: bool,
    groups_error: bool,
    window_error: bool,
    created: CreatedWorkspace,
    mutations: RefCell<Vec<String>>,
    requests: RefCell<Vec<(String, String, bool)>>,
}

impl SyntheticHost {
    fn new(repo: &Repo) -> Self {
        Self {
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
        None
    }
    fn check_capabilities(&self) -> Result<Vec<String>> {
        panic!("unexpected capability probe")
    }
    fn list_groups(&self, window: Option<&str>) -> Result<Vec<Group>> {
        if self.groups_error {
            bail!("synthetic group query failed");
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
        Ok(Some(self.window.clone()))
    }
    fn current_workspace(&self) -> Result<Option<String>> {
        Ok(None)
    }
    fn window_for_workspace(&self, _: &str) -> Result<String> {
        if self.window_error {
            bail!("synthetic window query failed");
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
    fn add_workspace_to_group(&self, _: &str, _: &str, _: &str) -> Result<()> {
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
    fn set_agent_metadata(&self, _: &str, _: &str, _: &str, _: &str) -> Result<()> {
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

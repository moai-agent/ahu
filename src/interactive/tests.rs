//! Provider-neutral session lifecycle tests use synthetic hosts only.
use super::*;
use crate::interactive::session::SessionManager;
use crate::interactive::v2::{
    CoordinatorPlacement as HostCoordinatorPlacement, CoordinatorRequest, CreatedSession, Host,
    SessionCreationResult, TaskSessionRequest, WorkspaceOwnership,
};
use std::cell::RefCell;
use std::path::Path;

struct SyntheticHost {
    inherited: Option<String>,
    sessions: RefCell<std::collections::BTreeMap<String, String>>,
    mismatch: bool,
    fail_verify: bool,
    mutations: RefCell<Vec<String>>,
    request: RefCell<Option<(String, String, String, bool)>>,
}

impl SyntheticHost {
    fn new(_repo: &crate::git::Repo) -> Self {
        Self {
            inherited: None,
            sessions: RefCell::new(std::collections::BTreeMap::from([(
                "task-session".into(),
                "/work/task".into(),
            )])),
            mismatch: false,
            fail_verify: false,
            mutations: RefCell::new(Vec::new()),
            request: RefCell::new(None),
        }
    }
}

impl Host for SyntheticHost {
    fn inherited_workspace(&self) -> Option<String> {
        self.inherited.clone()
    }
    fn check_capabilities(&self) -> Result<Vec<String>> {
        Ok(vec!["create_task_session".into()])
    }
    fn create_task_session(
        &self,
        request: &TaskSessionRequest<'_>,
    ) -> SessionCreationResult<CreatedSession> {
        *self.request.borrow_mut() = Some((
            request.title.into(),
            request.summary.into(),
            request.startup_command.into(),
            request.focus,
        ));
        self.sessions.borrow_mut().insert(
            "new-session".into(),
            request.worktree.to_string_lossy().into_owned(),
        );
        Ok(CreatedSession {
            session_ref: "new-session".into(),
        })
    }
    fn verify_task_session(
        &self,
        session_ref: &str,
        _: &str,
        _: &Path,
    ) -> Result<WorkspaceOwnership> {
        if self.fail_verify {
            bail!("synthetic verification failed");
        }
        if self.mismatch {
            return Ok(WorkspaceOwnership::Mismatch);
        }
        Ok(if self.sessions.borrow().contains_key(session_ref) {
            WorkspaceOwnership::Owned
        } else {
            WorkspaceOwnership::Absent
        })
    }
    fn select_task_session(&self, session_ref: &str, _: &str, _: &Path) -> Result<()> {
        self.mutations
            .borrow_mut()
            .push(format!("select:{session_ref}"));
        Ok(())
    }
    fn close_task_session(&self, session_ref: &str, _: &str, _: &Path) -> Result<()> {
        self.mutations
            .borrow_mut()
            .push(format!("close:{session_ref}"));
        Ok(())
    }
    fn set_task_status(&self, session_ref: &str, _: &str, _: &Path, status: &str) -> Result<()> {
        self.mutations
            .borrow_mut()
            .push(format!("status:{session_ref}:{status}"));
        Ok(())
    }
    fn expand_task_session(&self, session_ref: &str, _: &str, _: &Path) -> Result<()> {
        self.mutations
            .borrow_mut()
            .push(format!("expand:{session_ref}"));
        Ok(())
    }
    fn set_agent_metadata(
        &self,
        session_ref: &str,
        _: &str,
        _: &Path,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<()> {
        self.mutations
            .borrow_mut()
            .push(format!("metadata:{session_ref}"));
        Ok(())
    }
    fn place_coordinator(&self, _: &CoordinatorRequest<'_>) -> Result<HostCoordinatorPlacement> {
        Ok(HostCoordinatorPlacement {
            notes: Vec::new(),
            opened_session: false,
        })
    }
    fn open_markdown(&self, _: &Path, _: bool) -> Result<String> {
        Ok("document".into())
    }
}

#[test]
fn task_session_creation_passes_only_provider_neutral_inputs() {
    let (_temp, repo, _, plan) = fixture();
    let host = SyntheticHost::new(&repo);
    let startup = interactive::startup_command(Path::new("/synthetic/ahu"), &plan.task_dir);
    let result = SessionManager::new(&host, &repo)
        .create_task_session("title", "summary", &plan.worktree, &startup, true)
        .unwrap();
    assert_eq!(result.session_ref, "new-session");
    assert_eq!(
        *host.request.borrow(),
        Some(("title".into(), "summary".into(), startup, true))
    );
}

#[test]
fn session_mutations_require_provider_confirmation_of_the_task_checkout() {
    let (_temp, repo, loaded, plan) = fixture();
    let mut record = prepared_record(&repo, &loaded, &plan, "prompt", Default::default());
    record.session_ref = Some("task-session".into());

    let host = SyntheticHost::new(&repo);
    let manager = SessionManager::new(&host, &repo);
    manager.select_task_session(&record).unwrap();
    manager.set_task_status(&record, "waiting").unwrap();
    manager.expand_task_session(&record).unwrap();
    manager.close_task_session(&record).unwrap();
    assert_eq!(
        *host.mutations.borrow(),
        [
            "select:task-session",
            "status:task-session:waiting",
            "expand:task-session",
            "close:task-session",
        ]
    );

    let mut host = SyntheticHost::new(&repo);
    host.mismatch = true;
    let manager = SessionManager::new(&host, &repo);
    assert!(manager.select_task_session(&record).is_err());
    assert!(manager.close_task_session(&record).is_err());
    assert!(host.mutations.borrow().is_empty());

    let mut host = SyntheticHost::new(&repo);
    host.fail_verify = true;
    assert!(
        SessionManager::new(&host, &repo)
            .close_task_session(&record)
            .is_err()
    );
}

#[test]
fn absent_sessions_are_reconciled_without_mutation_and_unbound_coordinators_stay_local() {
    let (_temp, repo, loaded, plan) = fixture();
    let mut record = prepared_record(&repo, &loaded, &plan, "prompt", Default::default());
    record.session_ref = Some("missing".into());
    let host = SyntheticHost::new(&repo);
    let manager = SessionManager::new(&host, &repo);
    assert!(!manager.close_task_session(&record).unwrap());
    assert!(host.mutations.borrow().is_empty());

    assert!(
        !manager
            .group_coordinator("/usr/bin/true", "agent", "codex", "model", &[])
            .unwrap()
            .opened_workspace
    );
    assert!(host.request.borrow().is_none());
}

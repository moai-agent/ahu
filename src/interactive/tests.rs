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
    persistence: RefCell<Option<String>>,
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
            persistence: RefCell::new(None),
            mismatch: false,
            fail_verify: false,
            mutations: RefCell::new(Vec::new()),
            request: RefCell::new(None),
        }
    }
}

impl Host for SyntheticHost {
    fn persistence_key(&self) -> Option<String> {
        self.persistence.borrow().clone()
    }
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
        expected_directory: &Path,
    ) -> Result<WorkspaceOwnership> {
        if self.fail_verify {
            bail!("synthetic verification failed");
        }
        if self.mismatch {
            return Ok(WorkspaceOwnership::Mismatch);
        }
        let sessions = self.sessions.borrow();
        let found = if session_ref.is_empty() {
            sessions
                .values()
                .any(|directory| Path::new(directory) == expected_directory)
        } else {
            sessions.contains_key(session_ref)
        };
        Ok(if found {
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

fn frozen_test_host() -> crate::interactive::binding::FrozenHost {
    use crate::plugin_binding::{Binding, Compatibility, Grant, Source, SourceKind};
    crate::interactive::binding::FrozenHost {
        binding: Binding {
            schema_version: 1,
            id: "test-host".into(),
            version: "1.0.0".into(),
            source: Source {
                repository: "/repo/plugin".into(),
                revision: "0".repeat(64),
                sha256: "0".repeat(64),
                subdirectory: None,
                kind: SourceKind::Local,
            },
            compatibility: Compatibility {
                ahu: env!("CARGO_PKG_VERSION").into(),
                host_api: 1,
            },
            enabled: true,
            grants: vec![Grant::InteractiveBackend],
        },
        policy_revision: "0".repeat(40),
        package: "test-package".into(),
        executable: crate::interactive::external::Executable {
            path: "bin/host".into(),
            protocol_version: 2,
            timeout_ms: 5_000,
        },
        executable_sha256: "0".repeat(64),
        discovered_root: None,
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

#[test]
fn missing_external_session_reference_is_reconciled_by_exact_task_directory() {
    let (_temp, repo, loaded, plan) = fixture();
    let mut record = prepared_record(&repo, &loaded, &plan, "prompt", Default::default());
    let frozen = frozen_test_host();
    let host = SyntheticHost::new(&repo);
    *host.persistence.borrow_mut() = Some(frozen.key().unwrap());
    host.sessions.borrow_mut().insert(
        "orphaned-provider-session".into(),
        record.worktree.to_string_lossy().into_owned(),
    );
    record.session_host = Some(frozen);

    let manager = SessionManager::new(&host, &repo);
    assert!(manager.close_task_session(&record).unwrap());
    assert_eq!(&*host.mutations.borrow(), &["close:"]);

    let absent = SyntheticHost::new(&repo);
    *absent.persistence.borrow_mut() = Some(record.session_host.as_ref().unwrap().key().unwrap());
    assert!(
        !SessionManager::new(&absent, &repo)
            .close_task_session(&record)
            .unwrap()
    );
    assert!(absent.mutations.borrow().is_empty());
}

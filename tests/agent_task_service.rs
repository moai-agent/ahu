//! Offline V1 conformance: no native harness, auth, cmux or telemetry sessions.
mod common;
use ahu::{
    agent_task::{Controller, Session, v1::*},
    git, task,
};
use common::TestRepo;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn record_for(repo: &TestRepo, task_id: &str, worktree: &Path) -> task::TaskRecord {
    let discovered = git::discover(repo.path()).unwrap();
    let adapter = ahu::harness::adapter_for("claude-code").unwrap();
    let (delivered, delivery) =
        ahu::orchestration::deliver(Some("You are chris."), "current work").unwrap();
    let command = adapter
        .launch_command(&ahu::harness::LaunchRequest {
            model: "claude-opus-5",
            prompt: &delivered,
            cwd: worktree,
            permissions: Default::default(),
        })
        .unwrap();
    task::TaskRecord {
        schema_version: task::TASK_SCHEMA_VERSION,
        task_id: task_id.to_string(),
        title: format!("task {task_id}"),
        summary: String::new(),
        created_at: format!("2026-09-12T00:00:{:02}Z", task_id.len()),
        repo_identity: discovered.identity(),
        repo_root: discovered.root.clone(),
        branch: format!("ahu/chris/{task_id}"),
        worktree: worktree.to_path_buf(),
        base_commit: discovered.head.clone(),
        identity: task::LaunchIdentity {
            release_id: None,
            mode: task::LaunchMode::Named,
            agent: "chris".to_string(),
            agent_version: Some("1.0.0".to_string()),
            permissions: Default::default(),
            harness: "claude-code".to_string(),
            model: "claude-opus-5".to_string(),
            instructions_source: Some(".claude/agents/chris.md".to_string()),
            source_digest: Some("2".repeat(64)),
            instructions_digest: Some("3".repeat(64)),
            identity_digest: Some("4".repeat(64)),
            selection_basis: None,
        },
        policy_digest: "0".repeat(64),
        catalog_version: ahu::catalog::CATALOG_VERSION.to_string(),
        config_snapshot: Default::default(),
        config_snapshot_digest: "0".repeat(64),
        hooks: Default::default(),
        hooks_digest: String::new(),
        materialize: Default::default(),
        launch_command: command.redacted(),
        delivery,
        prompt_digest: ahu::util::digest_bytes(b"current work"),
        harness_executable: PathBuf::from("/usr/local/bin/claude"),
        reliability_warning: None,
        enforcement: adapter
            .enforcement("claude-opus-5", Default::default())
            .unwrap(),
        session_host: None,
        session_ref: None,
        state: task::TaskState::Starting,
    }
}

fn owner() -> Principal {
    Principal::Operator("operator".into())
}
fn operations() -> BTreeSet<Operation> {
    [
        Operation::List,
        Operation::Read,
        Operation::Replay,
        Operation::Submit,
        Operation::Cancel,
        Operation::Input,
    ]
    .into()
}
fn session(core: &Controller, id: &str) -> Session {
    core.session(owner(), [id.to_owned()].into(), operations())
}
fn mutation(id: &str, op: &str, revision: u64, action: Action) -> Mutation {
    Mutation {
        version: VERSION,
        task: id.into(),
        operation_id: op.into(),
        expected_revision: revision,
        action,
    }
}
struct Adapter {
    starts: Arc<AtomicUsize>,
    inputs: Vec<String>,
    cancels: usize,
    start: StartOutcome,
    effect: EffectOutcome,
    provider: String,
    panic: bool,
}
impl Default for Adapter {
    fn default() -> Self {
        Self {
            starts: Arc::new(AtomicUsize::new(0)),
            inputs: vec![],
            cancels: 0,
            start: StartOutcome::Running,
            effect: EffectOutcome::Confirmed,
            provider: "fixture".into(),
            panic: false,
        }
    }
}
impl ExecutionAdapter for Adapter {
    fn provider_id(&self) -> &str {
        &self.provider
    }
    fn start(&mut self, _: &Identity) -> StartOutcome {
        self.starts.fetch_add(1, Ordering::SeqCst);
        assert!(!self.panic, "injected provider death after startup");
        self.start
    }
    fn cancel(&mut self, _: &Identity) -> EffectOutcome {
        self.cancels += 1;
        self.effect
    }
    fn input(&mut self, _: &Identity, _: &str, text: &str) -> EffectOutcome {
        self.inputs.push(text.into());
        self.effect
    }
}
struct Fixture {
    repo: TestRepo,
    id: String,
    record: task::TaskRecord,
    core: Controller,
    local: bool,
}
impl Fixture {
    fn new(local: bool) -> Self {
        let repo = TestRepo::new();
        let discovered = git::discover(repo.path()).unwrap();
        let core = if local {
            Controller::local(&discovered).unwrap()
        } else {
            Controller::memory(&discovered.identity())
        };
        let id = task::new_task_id().unwrap();
        let record = record_for(&repo, &id, repo.path());
        core.register_admitted(&record, "fixture", owner(), true)
            .unwrap();
        Self {
            repo,
            id,
            record,
            core,
            local,
        }
    }
    fn reconnect(&self) -> Controller {
        if self.local {
            Controller::local(&git::discover(self.repo.path()).unwrap()).unwrap()
        } else {
            self.core.clone()
        }
    }
    fn client(&self) -> Session {
        session(&self.core, &self.id)
    }
    fn start(&self, adapter: &mut Adapter) -> Snapshot {
        self.client()
            .mutate(mutation(&self.id, "submit", 1, Action::Submit))
            .unwrap();
        self.core.dispatch_start(&self.id, adapter).unwrap()
    }
    fn path(&self) -> PathBuf {
        ahu::state::coordination_dir(&git::discover(self.repo.path()).unwrap())
            .unwrap()
            .join("agent-tasks-v1")
            .join(format!("{}.json", self.id))
    }
}

#[test]
fn conformance_denials_versions_ownership_and_capabilities() {
    for local in [false, true] {
        let f = Fixture::new(local);
        let client = f.client();
        assert_eq!(
            client.capabilities(VERSION).unwrap().operations,
            operations()
        );
        assert_eq!(client.get(2, &f.id), Err(Error::UnsupportedVersion));
        let outsider = f.core.session(
            Principal::Plugin("ungranted".into()),
            BTreeSet::new(),
            operations(),
        );
        assert_eq!(outsider.get(1, &f.id), Err(Error::Denied));
        assert_eq!(
            outsider.mutate(mutation(&f.id, "x", 1, Action::Submit)),
            Err(Error::Denied)
        );
        let other_owner = f.core.session(
            Principal::Operator("other".into()),
            [f.id.clone()].into(),
            operations(),
        );
        assert_eq!(
            other_owner.mutate(mutation(&f.id, "x", 1, Action::Submit)),
            Err(Error::Denied)
        );
        let worker = f.core.session(
            Principal::Worker(f.id.clone()),
            [f.id.clone()].into(),
            operations(),
        );
        assert_eq!(
            worker.capabilities(1).unwrap().operations,
            [Operation::List, Operation::Read, Operation::Replay].into()
        );
        assert_eq!(
            worker.mutate(mutation(&f.id, "x", 1, Action::Input("text".into()))),
            Err(Error::Denied)
        );
        assert_eq!(client.get(1, &f.id).unwrap().revision, 1);
        let mut wrong_provider = Adapter {
            provider: "wrong".into(),
            ..Default::default()
        };
        assert_eq!(
            f.core.dispatch_start(&f.id, &mut wrong_provider),
            Err(Error::Denied)
        );
        assert_eq!(wrong_provider.starts.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn scoped_listing_pages_exact_grants_and_reauthorizes_cursors() {
    for local in [false, true] {
        let f = Fixture::new(local);
        let mut granted = BTreeSet::new();
        for n in 1..=3 {
            let id = format!("00000000-0000-7000-8000-{n:012}");
            let record = record_for(&f.repo, &id, f.repo.path());
            f.core
                .register_admitted(&record, "fixture", owner(), true)
                .unwrap();
            granted.insert(id);
        }
        let ids = granted.iter().cloned().collect::<Vec<_>>();
        let plugin = f
            .core
            .plugin_read_session("fixture", granted.clone())
            .unwrap();
        let service: &dyn TaskService = &plugin;
        assert!(
            service
                .capabilities(VERSION)
                .unwrap()
                .operations
                .contains(&Operation::List)
        );
        let first = service.list(VERSION, None, 2).unwrap();
        assert_eq!(first.tasks.len(), 2);
        assert_eq!(first.next_after.as_deref(), Some(ids[1].as_str()));
        for (snapshot, id) in first.tasks.iter().zip(&ids) {
            assert_eq!(*snapshot, plugin.get(VERSION, id).unwrap());
        }
        let reconnected = f
            .reconnect()
            .plugin_read_session("fixture", granted)
            .unwrap();
        let second = reconnected
            .list(VERSION, first.next_after.as_deref(), 2)
            .unwrap();
        assert_eq!(second.tasks, vec![plugin.get(VERSION, &ids[2]).unwrap()]);
        assert_eq!(second.next_after, None);
        assert!(
            plugin
                .list(VERSION, Some(&ids[2]), 1)
                .unwrap()
                .tasks
                .is_empty()
        );
        assert_eq!(plugin.list(VERSION, Some(&f.id), 1), Err(Error::Denied));
        assert_eq!(
            plugin.list(VERSION, Some(&ids[0][..8]), 1),
            Err(Error::Denied)
        );
        let revoked = f
            .reconnect()
            .plugin_read_session("fixture", [ids[2].clone()].into())
            .unwrap();
        assert_eq!(
            revoked.list(VERSION, first.next_after.as_deref(), 2),
            Err(Error::Denied)
        );
        assert_eq!(revoked.list(VERSION, None, 2).unwrap().tasks, second.tasks);
        // A list capability conveys no execution rights or implicit Read grant.
        let list_only = f
            .core
            .session(owner(), [f.id.clone()].into(), [Operation::List].into());
        assert_eq!(
            list_only.list(VERSION, None, 1).unwrap().tasks,
            vec![f.client().get(VERSION, &f.id).unwrap()]
        );
        assert_eq!(list_only.get(VERSION, &f.id), Err(Error::Denied));
        assert_eq!(
            list_only.mutate(mutation(&f.id, "submit", 1, Action::Submit)),
            Err(Error::Denied)
        );
    }
}

#[test]
fn scoped_listing_requires_capability_valid_version_and_bounded_pages() {
    for local in [false, true] {
        let f = Fixture::new(local);
        let read_only = f
            .core
            .session(owner(), [f.id.clone()].into(), [Operation::Read].into());
        assert_eq!(read_only.list(VERSION, None, 1), Err(Error::Denied));
        let empty = f
            .core
            .plugin_read_session("fixture", BTreeSet::new())
            .unwrap();
        assert_eq!(
            empty.list(VERSION, None, 1).unwrap(),
            TaskPage {
                tasks: vec![],
                next_after: None
            }
        );
        assert_eq!(empty.list(VERSION, Some(&f.id), 1), Err(Error::Denied));
        let plugin = f
            .core
            .plugin_read_session("fixture", [f.id.clone()].into())
            .unwrap();
        assert_eq!(
            plugin.list(VERSION + 1, None, 1),
            Err(Error::UnsupportedVersion)
        );
        for limit in [0, MAX_PAGE_SIZE + 1, usize::MAX] {
            assert_eq!(
                plugin.list(VERSION, None, limit),
                Err(Error::InvalidRequest)
            );
        }
        assert_eq!(
            plugin
                .list(VERSION, None, MAX_PAGE_SIZE)
                .unwrap()
                .tasks
                .len(),
            1
        );
        let worker = f.core.session(
            Principal::Worker("fixture".into()),
            [f.id.clone()].into(),
            operations(),
        );
        assert_eq!(
            worker.list(VERSION, None, 1).unwrap().tasks,
            plugin.list(VERSION, None, 1).unwrap().tasks
        );
        assert_eq!(
            worker.mutate(mutation(&f.id, "submit", 1, Action::Submit)),
            Err(Error::Denied)
        );
    }
}

#[test]
fn scoped_listing_propagates_missing_records_and_storage_errors_without_partial_pages() {
    for local in [false, true] {
        let f = Fixture::new(local);
        // Sort after the admitted task to test failure following a valid snapshot.
        let missing = "ffffffff-ffff-7fff-bfff-ffffffffffff".to_string();
        let plugin = f
            .core
            .plugin_read_session("fixture", [f.id.clone(), missing.clone()].into())
            .unwrap();
        assert_eq!(plugin.list(VERSION, None, 2), Err(Error::NotFound));
        // Looking ahead does not read the next page's record.
        let first = plugin.list(VERSION, None, 1).unwrap();
        assert_eq!(first.next_after, Some(f.id.clone()));
        assert_eq!(
            plugin.list(VERSION, first.next_after.as_deref(), 1),
            Err(Error::NotFound)
        );
        assert_eq!(f.client().get(VERSION, &f.id).unwrap().revision, 1);
        if local {
            std::fs::write(f.path(), b"invalid synthetic state").unwrap();
            assert!(matches!(
                plugin.list(VERSION, None, 1),
                Err(Error::Storage(_))
            ));
            let only_missing = f
                .core
                .plugin_read_session("fixture", [missing].into())
                .unwrap();
            assert_eq!(only_missing.list(VERSION, None, 1), Err(Error::NotFound));
        }
    }
}

#[test]
fn conformance_duplicates_stale_revisions_reconnect_and_lost_ack() {
    for local in [false, true] {
        let f = Fixture::new(local);
        let request = mutation(&f.id, "submit", 1, Action::Submit);
        let response = f.client().mutate(request.clone()).unwrap();
        let mut adapter = Adapter::default();
        let running = f.core.dispatch_start(&f.id, &mut adapter).unwrap();
        assert!(running.revision > response.revision);
        let core = f.reconnect();
        let client = session(&core, &f.id);
        // Exact original response, even when the CAS revision is now stale.
        assert_eq!(client.mutate(request.clone()).unwrap(), response);
        assert_eq!(core.dispatch_start(&f.id, &mut adapter).unwrap(), running);
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 1);
        assert_eq!(
            client.mutate(mutation(&f.id, "submit", 1, Action::Cancel)),
            Err(Error::Conflict)
        );
        assert_eq!(
            client.mutate(mutation(&f.id, "cancel", 1, Action::Cancel)),
            Err(Error::StaleRevision {
                current: running.revision
            })
        );
        let revoked = core.session(owner(), [f.id.clone()].into(), [Operation::Read].into());
        assert_eq!(revoked.mutate(request), Err(Error::Denied));
        let replay = client.replay(1, &f.id, 0).unwrap();
        assert_eq!(replay.changes.len() as u64, running.revision);
        assert!(
            replay
                .changes
                .windows(2)
                .all(|w| w[1].revision == w[0].revision + 1)
        );
        assert!(
            client
                .replay(1, &f.id, running.revision)
                .unwrap()
                .changes
                .is_empty()
        );
        assert_eq!(
            client.replay(1, &f.id, running.revision + 1),
            Err(Error::StaleRevision {
                current: running.revision
            })
        );
        assert_eq!(
            core.register_admitted(&f.record, "fixture", owner(), true)
                .unwrap(),
            running
        );
        let mut changed = f.record.clone();
        changed.identity.model = "different-model".into();
        assert_eq!(
            core.register_admitted(&changed, "fixture", owner(), true),
            Err(Error::Conflict)
        );
    }
}

#[test]
fn conformance_cancellation_is_intent_until_confirmed_and_blocks_new_input() {
    for local in [false, true] {
        let f = Fixture::new(local);
        let mut adapter = Adapter::default();
        let running = f.start(&mut adapter);
        let request = mutation(&f.id, "cancel", running.revision, Action::Cancel);
        let requested = f.client().mutate(request.clone()).unwrap();
        assert_eq!(requested.lifecycle, Lifecycle::Running);
        assert!(requested.cancellation_requested);
        assert_eq!(adapter.cancels, 0);
        assert_eq!(
            f.client().mutate(mutation(
                &f.id,
                "input",
                requested.revision,
                Action::Input("stop".into())
            )),
            Err(Error::InvalidLifecycle)
        );
        let cancelled = f.core.dispatch_cancel(&f.id, &mut adapter).unwrap();
        assert_eq!(cancelled.lifecycle, Lifecycle::Cancelled);
        assert_eq!(
            f.core.dispatch_cancel(&f.id, &mut adapter).unwrap(),
            cancelled
        );
        assert_eq!(adapter.cancels, 1);
        assert_eq!(f.client().mutate(request).unwrap(), requested);
        assert_eq!(
            f.core
                .reconcile(&f.id, "fixture", cancelled.revision, Lifecycle::Running),
            Err(Error::InvalidLifecycle)
        );
    }
}

#[test]
fn cancellation_before_dispatch_never_starts_provider() {
    for local in [false, true] {
        let f = Fixture::new(local);
        let queued = f
            .client()
            .mutate(mutation(&f.id, "submit", 1, Action::Submit))
            .unwrap();
        f.client()
            .mutate(mutation(&f.id, "cancel", queued.revision, Action::Cancel))
            .unwrap();
        let mut adapter = Adapter::default();
        assert_eq!(
            f.core
                .dispatch_start(&f.id, &mut adapter)
                .unwrap()
                .lifecycle,
            Lifecycle::Cancelled
        );
        assert_eq!(adapter.starts.load(Ordering::SeqCst), 0);
        assert_eq!(adapter.cancels, 0);
    }
}

#[test]
fn conformance_input_is_literal_bounded_idempotent_and_inspectable() {
    for local in [false, true] {
        let f = Fixture::new(local);
        let mut adapter = Adapter::default();
        let running = f.start(&mut adapter);
        let text = "  $(do-not-execute) `literal`\nkeep newlines  ";
        let request = mutation(&f.id, "input", running.revision, Action::Input(text.into()));
        let accepted = f.client().mutate(request.clone()).unwrap();
        assert_eq!(
            f.client().effects(1, &f.id).unwrap().inputs[0].delivery,
            DeliveryState::Pending
        );
        let delivered = f.core.dispatch_input(&f.id, "input", &mut adapter).unwrap();
        let core = f.reconnect();
        let client = session(&core, &f.id);
        assert_eq!(client.mutate(request).unwrap(), accepted);
        assert_eq!(
            core.dispatch_input(&f.id, "input", &mut adapter).unwrap(),
            delivered
        );
        assert_eq!(adapter.inputs, [text]);
        assert_eq!(
            client.effects(1, &f.id).unwrap().inputs[0].delivery,
            DeliveryState::Confirmed
        );
        assert_eq!(
            client.mutate(mutation(
                &f.id,
                "large",
                delivered.revision,
                Action::Input("x".repeat(MAX_INPUT_BYTES + 1))
            )),
            Err(Error::InvalidRequest)
        );
        assert_eq!(
            client.mutate(mutation(
                &f.id,
                "empty",
                delivered.revision,
                Action::Input("  ".into())
            )),
            Err(Error::InvalidRequest)
        );
        let events = serde_json::to_string(&client.replay(1, &f.id, 0).unwrap()).unwrap();
        assert!(!events.contains("do-not-execute"));
    }
}

#[test]
fn conformance_provider_failure_and_lost_effect_ack_never_reexecute() {
    for local in [false, true] {
        for outcome in [StartOutcome::Unknown, StartOutcome::NotStarted] {
            let f = Fixture::new(local);
            let mut adapter = Adapter {
                start: outcome,
                ..Default::default()
            };
            let result = f.start(&mut adapter);
            let core = f.reconnect();
            if outcome == StartOutcome::Unknown {
                assert_eq!(result.lifecycle, Lifecycle::OutcomeUnknown);
                assert_eq!(
                    core.dispatch_start(&f.id, &mut adapter),
                    Err(Error::OutcomeUnknown)
                );
                core.reconcile(&f.id, "fixture", result.revision, Lifecycle::Running)
                    .unwrap();
                assert_eq!(
                    core.dispatch_start(&f.id, &mut adapter),
                    Err(Error::OutcomeUnknown)
                );
            } else {
                assert_eq!(result.lifecycle, Lifecycle::Failed);
                core.dispatch_start(&f.id, &mut adapter).unwrap();
            }
            assert_eq!(adapter.starts.load(Ordering::SeqCst), 1);
        }
        let f = Fixture::new(local);
        let mut adapter = Adapter {
            effect: EffectOutcome::Unknown,
            ..Default::default()
        };
        let running = f.start(&mut adapter);
        f.client()
            .mutate(mutation(
                &f.id,
                "input",
                running.revision,
                Action::Input("once".into()),
            ))
            .unwrap();
        let delivered = f.core.dispatch_input(&f.id, "input", &mut adapter).unwrap();
        assert_eq!(
            f.reconnect().dispatch_input(&f.id, "input", &mut adapter),
            Err(Error::OutcomeUnknown)
        );
        assert_eq!(adapter.inputs, ["once"]);
        assert_eq!(
            f.client().effects(1, &f.id).unwrap().inputs[0].delivery,
            DeliveryState::Unknown
        );
        f.client()
            .mutate(mutation(
                &f.id,
                "cancel",
                delivered.revision,
                Action::Cancel,
            ))
            .unwrap();
        let unknown = f.core.dispatch_cancel(&f.id, &mut adapter).unwrap();
        assert_eq!(unknown.lifecycle, Lifecycle::Running);
        assert!(unknown.cancellation_requested);
        assert_eq!(
            f.reconnect().dispatch_cancel(&f.id, &mut adapter),
            Err(Error::OutcomeUnknown)
        );
        assert_eq!(adapter.cancels, 1);
    }
}

#[test]
fn durable_provider_death_after_effect_leaves_claim_and_no_restart() {
    let f = Fixture::new(true);
    f.client()
        .mutate(mutation(&f.id, "submit", 1, Action::Submit))
        .unwrap();
    let mut adapter = Adapter {
        panic: true,
        ..Default::default()
    };
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f
            .core
            .dispatch_start(&f.id, &mut adapter)))
        .is_err()
    );
    let core = f.reconnect();
    let client = session(&core, &f.id);
    assert_eq!(client.get(1, &f.id).unwrap().lifecycle, Lifecycle::Starting);
    assert_eq!(
        client.effects(1, &f.id).unwrap().start,
        DeliveryState::Claimed
    );
    assert_eq!(
        core.dispatch_start(&f.id, &mut adapter),
        Err(Error::OutcomeUnknown)
    );
    assert_eq!(adapter.starts.load(Ordering::SeqCst), 1);
}

#[test]
fn durable_parallel_submissions_and_execution_have_one_winner() {
    let f = Fixture::new(true);
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    session(&f.reconnect(), &f.id).mutate(mutation(
                        &f.id,
                        "same",
                        1,
                        Action::Submit,
                    ))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|v| v.join().unwrap())
            .collect::<Vec<_>>()
    });
    let mut accepted = 0;
    for result in results {
        match result {
            Ok(snapshot) => {
                assert_eq!(snapshot.revision, 2);
                accepted += 1;
            }
            Err(Error::Busy) => (),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(accepted > 0);
    let count = Arc::new(AtomicUsize::new(0));
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    let mut adapter = Adapter {
                        starts: count.clone(),
                        ..Default::default()
                    };
                    f.reconnect().dispatch_start(&f.id, &mut adapter)
                })
            })
            .collect();
        for handle in handles {
            assert!(matches!(handle.join().unwrap(), Ok(_) | Err(Error::Busy)));
        }
    });
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn replay_expiry_and_capacity_are_explicit_without_mutation() {
    for local in [false, true] {
        let f = Fixture::new(local);
        let mut adapter = Adapter::default();
        let mut snapshot = f.start(&mut adapter);
        for i in 0..MAX_OPERATIONS - 1 {
            snapshot = f
                .client()
                .mutate(mutation(
                    &f.id,
                    &format!("cancel-{i}"),
                    snapshot.revision,
                    Action::Cancel,
                ))
                .unwrap();
        }
        assert!(matches!(
            f.client().replay(1, &f.id, 0),
            Err(Error::ReplayExpired { .. })
        ));
        let changes = f
            .client()
            .replay(1, &f.id, snapshot.revision - REPLAY_WINDOW as u64)
            .unwrap()
            .changes;
        assert_eq!(changes.len(), REPLAY_WINDOW);
        assert_eq!(
            f.client()
                .mutate(mutation(&f.id, "full", snapshot.revision, Action::Cancel)),
            Err(Error::Capacity)
        );
        assert_eq!(f.client().get(1, &f.id).unwrap(), snapshot);
        // Original idempotency tombstones survive replay compaction.
        assert_eq!(
            f.client()
                .mutate(mutation(&f.id, "submit", 1, Action::Submit))
                .unwrap()
                .revision,
            2
        );
    }
}

#[test]
fn durable_state_refuses_redirection_bad_permissions_versions_and_identity() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for damage in [
        "symlink",
        "hardlink",
        "permissions",
        "version",
        "identity",
        "corrupt",
    ] {
        let f = Fixture::new(true);
        let path = f.path();
        match damage {
            "symlink" => {
                let outside = f.repo.path().join("outside");
                std::fs::rename(&path, &outside).unwrap();
                symlink(&outside, &path).unwrap();
            }
            "hardlink" => std::fs::hard_link(&path, f.repo.path().join("outside")).unwrap(),
            "permissions" => {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap()
            }
            "corrupt" => std::fs::write(&path, "invalid").unwrap(),
            _ => {
                let mut value: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                if damage == "version" {
                    value["version"] = 99.into();
                } else {
                    value["snapshot"]["identity"]["repository"] = "other".into();
                }
                std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
            }
        }
        assert!(f.client().get(1, &f.id).is_err(), "{damage}");
    }
}

#[test]
fn existing_record_attachment_preserves_legacy_state_and_never_restarts() {
    let f = Fixture::new(true);
    let discovered = git::discover(f.repo.path()).unwrap();
    let id = task::new_task_id().unwrap();
    let record = record_for(&f.repo, &id, f.repo.path());
    let dir = ahu::storage::RepositoryStorage::new(&discovered)
        .unwrap()
        .checkout
        .task_dir(&discovered.identity(), &id);
    task::save(&dir, &record, "keep prompt").unwrap();
    let before = std::fs::read(dir.join("task.json")).unwrap();
    let tracked = f
        .core
        .track_existing(&discovered, &id, "fixture", owner(), true)
        .unwrap();
    assert_eq!(tracked.lifecycle, Lifecycle::OutcomeUnknown);
    let core = f.reconnect();
    let client = session(&core, &id);
    assert_eq!(
        client.effects(VERSION, &id).unwrap().start,
        DeliveryState::Unknown
    );
    let mut adapter = Adapter::default();
    assert_eq!(
        core.dispatch_start(&id, &mut adapter),
        Err(Error::OutcomeUnknown)
    );
    assert_eq!(adapter.starts.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(dir.join("task.json")).unwrap(), before);
    assert_eq!(task::load_prompt(&dir).unwrap(), "keep prompt");
    // Re-registration through the new-task path cannot reset imported state.
    f.core
        .register_admitted(&record, "fixture", owner(), true)
        .unwrap();
    assert_eq!(
        f.core.dispatch_start(&id, &mut adapter),
        Err(Error::OutcomeUnknown)
    );
    assert_eq!(client.get(VERSION, &id).unwrap(), tracked);
    assert_eq!(adapter.starts.load(Ordering::SeqCst), 0);
}

#[test]
fn concurrent_cancellation_cannot_confirm_before_startup_returns() {
    for local in [false, true] {
        let f = Fixture::new(local);
        f.client()
            .mutate(mutation(&f.id, "submit", 1, Action::Submit))
            .unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        struct Blocking {
            entered: std::sync::mpsc::Sender<()>,
            release: std::sync::mpsc::Receiver<()>,
        }
        impl ExecutionAdapter for Blocking {
            fn provider_id(&self) -> &str {
                "fixture"
            }
            fn start(&mut self, _: &Identity) -> StartOutcome {
                self.entered.send(()).unwrap();
                self.release
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                StartOutcome::Running
            }
            fn cancel(&mut self, _: &Identity) -> EffectOutcome {
                panic!("unexpected cancellation")
            }
            fn input(&mut self, _: &Identity, _: &str, _: &str) -> EffectOutcome {
                panic!("unexpected input")
            }
        }
        std::thread::scope(|scope| {
            let core = f.reconnect();
            let task_id = &f.id;
            let start = scope.spawn(move || {
                core.dispatch_start(
                    task_id,
                    &mut Blocking {
                        entered: entered_tx,
                        release: release_rx,
                    },
                )
            });
            entered_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            let client = f.client();
            let starting = client.get(1, &f.id).unwrap();
            assert_eq!(starting.lifecycle, Lifecycle::Starting);
            let intent = client
                .mutate(mutation(&f.id, "cancel", starting.revision, Action::Cancel))
                .unwrap();
            let mut cancel = Adapter::default();
            assert_eq!(
                f.reconnect().dispatch_cancel(&f.id, &mut cancel),
                Err(Error::Busy)
            );
            assert_eq!(cancel.cancels, 0);
            assert_eq!(
                f.core
                    .reconcile(&f.id, "fixture", intent.revision, Lifecycle::Cancelled),
                Err(Error::Busy)
            );
            release_tx.send(()).unwrap();
            let running = start.join().unwrap().unwrap();
            assert!(running.cancellation_requested);
            assert_eq!(running.lifecycle, Lifecycle::Running);
            assert_eq!(
                f.core
                    .dispatch_cancel(&f.id, &mut cancel)
                    .unwrap()
                    .lifecycle,
                Lifecycle::Cancelled
            );
            assert_eq!(cancel.cancels, 1);
        });
    }
}

#[test]
fn storage_failure_before_claim_prevents_provider_execution() {
    let f = Fixture::new(true);
    f.client()
        .mutate(mutation(&f.id, "submit", 1, Action::Submit))
        .unwrap();
    let original = f.path();
    let moved = original.with_extension("saved");
    std::fs::rename(&original, &moved).unwrap();
    std::fs::create_dir(&original).unwrap();
    let mut adapter = Adapter::default();
    assert!(matches!(
        f.core.dispatch_start(&f.id, &mut adapter),
        Err(Error::Storage(_))
    ));
    assert_eq!(adapter.starts.load(Ordering::SeqCst), 0);
    std::fs::remove_dir(&original).unwrap();
    std::fs::rename(&moved, &original).unwrap();
    f.core.dispatch_start(&f.id, &mut adapter).unwrap();
    assert_eq!(adapter.starts.load(Ordering::SeqCst), 1);
}

#[test]
fn failure_persisting_provider_ack_preserves_claim_across_reconnect() {
    let f = Fixture::new(true);
    f.client()
        .mutate(mutation(&f.id, "submit", 1, Action::Submit))
        .unwrap();
    struct LostAck {
        path: PathBuf,
        calls: usize,
    }
    impl ExecutionAdapter for LostAck {
        fn provider_id(&self) -> &str {
            "fixture"
        }
        fn start(&mut self, _: &Identity) -> StartOutcome {
            self.calls += 1;
            std::fs::rename(&self.path, self.path.with_extension("claimed")).unwrap();
            std::fs::create_dir(&self.path).unwrap();
            StartOutcome::Running
        }
        fn cancel(&mut self, _: &Identity) -> EffectOutcome {
            unreachable!()
        }
        fn input(&mut self, _: &Identity, _: &str, _: &str) -> EffectOutcome {
            unreachable!()
        }
    }
    let mut adapter = LostAck {
        path: f.path(),
        calls: 0,
    };
    assert!(matches!(
        f.core.dispatch_start(&f.id, &mut adapter),
        Err(Error::Storage(_))
    ));
    std::fs::remove_dir(&adapter.path).unwrap();
    std::fs::rename(adapter.path.with_extension("claimed"), &adapter.path).unwrap();
    assert_eq!(
        f.reconnect().dispatch_start(&f.id, &mut adapter),
        Err(Error::OutcomeUnknown)
    );
    assert_eq!(adapter.calls, 1);
}

#[test]
fn unsupported_input_terminal_transitions_and_invalid_requests_fail_without_changes() {
    for local in [false, true] {
        let f = Fixture::new(local);
        let mut record = f.record.clone();
        record.task_id = task::new_task_id().unwrap();
        let registered = f
            .core
            .register_admitted(&record, "fixture", owner(), false)
            .unwrap();
        assert!(!registered.supports_input);
        let client = session(&f.core, &record.task_id);
        assert_eq!(
            client.mutate(mutation(&record.task_id, "../../escape", 1, Action::Submit)),
            Err(Error::InvalidRequest)
        );
        client
            .mutate(mutation(&record.task_id, "submit", 1, Action::Submit))
            .unwrap();
        let mut adapter = Adapter::default();
        let running = f
            .core
            .dispatch_start(&record.task_id, &mut adapter)
            .unwrap();
        assert_eq!(
            client.mutate(mutation(
                &record.task_id,
                "input",
                running.revision,
                Action::Input("text".into())
            )),
            Err(Error::UnsupportedOperation)
        );
        let exited = f
            .core
            .reconcile(
                &record.task_id,
                "fixture",
                running.revision,
                Lifecycle::Exited,
            )
            .unwrap();
        assert_eq!(
            client.mutate(mutation(
                &record.task_id,
                "cancel",
                exited.revision,
                Action::Cancel
            )),
            Err(Error::InvalidLifecycle)
        );
        assert_eq!(
            f.core.reconcile(
                &record.task_id,
                "fixture",
                exited.revision,
                Lifecycle::Queued
            ),
            Err(Error::InvalidLifecycle)
        );
        assert_eq!(client.get(1, &record.task_id).unwrap(), exited);
        let invalid = f
            .core
            .session(owner(), ["../escape".into()].into(), operations());
        assert_eq!(invalid.get(1, "../escape"), Err(Error::InvalidRequest));
    }
}

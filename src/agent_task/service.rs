use super::{
    store::{Store, storage, task_id},
    v1::*,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Arc};

use super::v1::DeliveryState as Delivery;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    principal: Principal,
    id: String,
    digest: String,
    response: Snapshot,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    id: String,
    text: String,
    delivery: Delivery,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    pub version: u32,
    pub snapshot: Snapshot,
    supports_input: bool,
    start: Delivery,
    cancel: Option<Delivery>,
    inputs: Vec<Input>,
    receipts: Vec<Receipt>,
    changes: Vec<Change>,
}
impl Entry {
    fn change(&mut self, kind: ChangeKind) -> Result<Snapshot> {
        self.snapshot.revision = self
            .snapshot
            .revision
            .checked_add(1)
            .ok_or(Error::Capacity)?;
        self.changes.push(Change {
            revision: self.snapshot.revision,
            kind,
            lifecycle: self.snapshot.lifecycle,
            cancellation_requested: self.snapshot.cancellation_requested,
        });
        if self.changes.len() > REPLAY_WINDOW {
            self.changes.remove(0);
        }
        Ok(self.snapshot.clone())
    }
    pub(super) fn validate(&self) -> Result<()> {
        if self.receipts.len() > MAX_OPERATIONS
            || self.inputs.len() > MAX_OPERATIONS
            || self.changes.is_empty()
            || self.changes.len() > REPLAY_WINDOW
            || self.changes.first().is_some_and(|v| v.revision == 0)
            || self.changes.last().map(|v| v.revision) != Some(self.snapshot.revision)
            || self
                .changes
                .windows(2)
                .any(|w| w[0].revision.checked_add(1) != Some(w[1].revision))
            || self.snapshot.supports_input != self.supports_input
            || self.inputs.iter().map(|v| v.text.len()).sum::<usize>() > 1024 * 1024
            || self.inputs.iter().any(|v| v.text.len() > MAX_INPUT_BYTES)
            || self.cancel.is_some() != self.snapshot.cancellation_requested
        {
            return Err(storage("invalid task service state"));
        }
        let last = self.changes.last().expect("nonempty changes checked above");
        if last.lifecycle != self.snapshot.lifecycle
            || last.cancellation_requested != self.snapshot.cancellation_requested
            || self.receipts.iter().any(|v| {
                v.response.version != VERSION
                    || v.response.identity != self.snapshot.identity
                    || v.response.owner != self.snapshot.owner
                    || v.principal != self.snapshot.owner
                    || v.response.revision == 0
                    || v.response.revision > self.snapshot.revision
            })
            || self
                .receipts
                .iter()
                .map(|v| (&v.principal, &v.id))
                .collect::<BTreeSet<_>>()
                .len()
                != self.receipts.len()
            || self
                .inputs
                .iter()
                .map(|v| &v.id)
                .collect::<BTreeSet<_>>()
                .len()
                != self.inputs.len()
        {
            return Err(storage("inconsistent task service state"));
        }
        Ok(())
    }
}

/// Trusted core ownership surface. Do not pass this object to a plugin. Existing
/// launch policy, registered identity, approval widening, executable resolution,
/// live parent grants and native admission remain prerequisites for registering
/// a prepared task and invoking its adapter. V1 grants never replace those checks.
/// Neither creating this object nor opening a session launches anything.
#[derive(Clone)]
pub struct Controller {
    store: Arc<Store>,
    repository: String,
}
impl Controller {
    /// Conformance implementation: identical transitions/receipts, no disk I/O.
    pub fn memory(repository: &str) -> Self {
        Self {
            store: Arc::new(Store::memory()),
            repository: repository.into(),
        }
    }
    /// Primary-owned coordination; no environment-selected or plugin-supplied path.
    pub fn local(repo: &crate::git::Repo) -> Result<Self> {
        Ok(Self {
            store: Arc::new(Store::local(repo)?),
            repository: repo.identity(),
        })
    }
    pub fn session(
        &self,
        principal: Principal,
        tasks: BTreeSet<String>,
        mut operations: BTreeSet<Operation>,
    ) -> Session {
        // Workers may inspect explicitly granted tasks, but never obtain operator
        // delivery rights by supplying a task ID or by reconnecting.
        if matches!(principal, Principal::Worker(_)) {
            operations.retain(|v| matches!(v, Operation::Read | Operation::Replay));
        }
        Session {
            controller: self.clone(),
            principal,
            tasks,
            operations,
        }
    }

    /// Register a NEW core-admitted task before any execution side effect.
    /// The record must be the frozen result of the ordinary core launch pipeline,
    /// not a deserialized plugin request. Existing tasks use `track_existing`:
    /// importing a Starting record must never authorize a second execution.
    /// This API intentionally does not create a worktree, alter task.json or
    /// bypass launch/native checks; the trusted caller owns that integration.
    pub fn register_admitted(
        &self,
        record: &crate::task::TaskRecord,
        provider: &str,
        owner: Principal,
        supports_input: bool,
    ) -> Result<Snapshot> {
        if record.state != crate::task::TaskState::Starting {
            return Err(Error::InvalidLifecycle);
        }
        self.register(
            record,
            provider,
            owner,
            supports_input,
            Lifecycle::Prepared,
            Delivery::Pending,
        )
    }

    /// Attach existing records through the repository's validated discovery path.
    /// No legacy rewrite/migration, provider execution or native-store read.
    pub fn track_existing(
        &self,
        repo: &crate::git::Repo,
        id: &str,
        provider: &str,
        owner: Principal,
        supports_input: bool,
    ) -> Result<Snapshot> {
        task_id(id)?;
        if repo.identity() != self.repository {
            return Err(Error::Denied);
        }
        let listing = crate::task::list(repo).map_err(storage)?;
        let (_, record) = listing
            .records
            .into_iter()
            .find(|(_, r)| r.task_id == id)
            .ok_or(Error::NotFound)?;
        use crate::task::TaskState as T;
        let lifecycle = match record.state {
            T::Starting => Lifecycle::OutcomeUnknown,
            T::Running => Lifecycle::Running,
            T::WaitingForApproval => Lifecycle::WaitingForApproval,
            T::Exited => Lifecycle::Exited,
            T::Failed => Lifecycle::Failed,
            T::Cancelled => Lifecycle::Cancelled,
        };
        self.register(
            &record,
            provider,
            owner,
            supports_input,
            lifecycle,
            Delivery::Confirmed,
        )
    }

    fn register(
        &self,
        record: &crate::task::TaskRecord,
        provider: &str,
        owner: Principal,
        supports_input: bool,
        lifecycle: Lifecycle,
        start: Delivery,
    ) -> Result<Snapshot> {
        task_id(&record.task_id)?;
        if record.repo_identity != self.repository {
            return Err(Error::Denied);
        }
        if !valid_token(provider)
            || !valid_principal(&owner)
            || matches!(owner, Principal::Worker(_))
            || record.identity.agent.is_empty()
            || record.identity.harness.is_empty()
            || record.identity.model.is_empty()
            || !crate::task::READABLE_SCHEMA_VERSIONS.contains(&record.schema_version)
        {
            return Err(Error::InvalidRequest);
        }
        let identity = Identity {
            repository: self.repository.clone(),
            task: record.task_id.clone(),
            provider: provider.into(),
            agent: record.identity.clone(),
            policy_digest: record.policy_digest.clone(),
            config_digest: record.config_snapshot_digest.clone(),
            prompt_digest: record.prompt_digest.clone(),
        };
        self.store.transaction(&record.task_id, |slot| {
            if let Some(entry) = slot {
                if entry.snapshot.identity != identity
                    || entry.snapshot.owner != owner
                    || entry.supports_input != supports_input
                {
                    return Err(Error::Conflict);
                }
                return Ok(entry.snapshot.clone());
            }
            let mut entry = Entry {
                version: VERSION,
                snapshot: Snapshot {
                    version: VERSION,
                    identity,
                    owner,
                    revision: 0,
                    lifecycle,
                    cancellation_requested: false,
                    supports_input,
                },
                supports_input,
                start,
                cancel: None,
                inputs: vec![],
                receipts: vec![],
                changes: vec![],
            };
            let snapshot = entry.change(ChangeKind::Admitted)?;
            *slot = Some(entry);
            Ok(snapshot)
        })
    }

    /// Durable at-most-once startup invocation. A crash after the claim leaves
    /// Starting/OutcomeUnknown. Reconcile evidence; never automatically re-run.
    pub fn dispatch_start(
        &self,
        task: &str,
        adapter: &mut dyn ExecutionAdapter,
    ) -> Result<Snapshot> {
        let _execution = self.store.execution_lock(task)?;
        let identity = self.store.transaction(task, |slot| {
            let entry = bound_entry(slot, adapter.provider_id())?;
            match entry.start {
                Delivery::Claimed | Delivery::Unknown => return Err(Error::OutcomeUnknown),
                Delivery::Confirmed => return Ok(None),
                Delivery::Pending => (),
            }
            if entry.snapshot.lifecycle != Lifecycle::Queued {
                return Err(Error::InvalidLifecycle);
            }
            if entry.snapshot.cancellation_requested {
                entry.snapshot.lifecycle = Lifecycle::Cancelled;
                entry.start = Delivery::Confirmed;
                entry.cancel = Some(Delivery::Confirmed);
                entry.change(ChangeKind::EffectObserved)?;
                return Ok(None);
            }
            entry.start = Delivery::Claimed;
            entry.snapshot.lifecycle = Lifecycle::Starting;
            entry.change(ChangeKind::EffectClaimed)?;
            Ok(Some(entry.snapshot.identity.clone()))
        })?;
        if let Some(identity) = identity {
            let outcome = adapter.start(&identity);
            self.store.transaction(task, |slot| {
                let entry = bound_entry(slot, adapter.provider_id())?;
                entry.start = if outcome == StartOutcome::Unknown {
                    Delivery::Unknown
                } else {
                    Delivery::Confirmed
                };
                if !entry.snapshot.lifecycle.terminal() {
                    entry.snapshot.lifecycle = match outcome {
                        StartOutcome::Running => Lifecycle::Running,
                        StartOutcome::NotStarted => Lifecycle::Failed,
                        StartOutcome::Unknown => Lifecycle::OutcomeUnknown,
                    };
                }
                entry.change(ChangeKind::EffectObserved)
            })
        } else {
            self.snapshot(task)
        }
    }

    pub fn dispatch_cancel(
        &self,
        task: &str,
        adapter: &mut dyn ExecutionAdapter,
    ) -> Result<Snapshot> {
        self.dispatch_effect(task, adapter, None)
    }
    pub fn dispatch_input(
        &self,
        task: &str,
        operation_id: &str,
        adapter: &mut dyn ExecutionAdapter,
    ) -> Result<Snapshot> {
        self.dispatch_effect(task, adapter, Some(operation_id))
    }
    fn dispatch_effect(
        &self,
        task: &str,
        adapter: &mut dyn ExecutionAdapter,
        input: Option<&str>,
    ) -> Result<Snapshot> {
        let _execution = self.store.execution_lock(task)?;
        let effect = self.store.transaction(task, |slot| {
            let entry = bound_entry(slot, adapter.provider_id())?;
            let (delivery, text) = if let Some(id) = input {
                let item = entry
                    .inputs
                    .iter()
                    .find(|v| v.id == id)
                    .ok_or(Error::NotFound)?;
                (item.delivery, Some(item.text.clone()))
            } else {
                (entry.cancel.ok_or(Error::InvalidLifecycle)?, None)
            };
            match delivery {
                Delivery::Claimed | Delivery::Unknown => return Err(Error::OutcomeUnknown),
                Delivery::Confirmed => return Ok(None),
                Delivery::Pending => (),
            }
            if entry.snapshot.lifecycle.terminal() {
                return Err(Error::InvalidLifecycle);
            }
            if input.is_some()
                && (!matches!(
                    entry.snapshot.lifecycle,
                    Lifecycle::Running | Lifecycle::WaitingForApproval
                ) || entry.snapshot.cancellation_requested)
            {
                return Err(Error::InvalidLifecycle);
            }
            if input.is_none() && entry.start == Delivery::Pending {
                entry.snapshot.lifecycle = Lifecycle::Cancelled;
                entry.start = Delivery::Confirmed;
                entry.cancel = Some(Delivery::Confirmed);
                entry.change(ChangeKind::EffectObserved)?;
                return Ok(None);
            }
            set_delivery(entry, input, Delivery::Claimed)?;
            entry.change(ChangeKind::EffectClaimed)?;
            Ok(Some((entry.snapshot.identity.clone(), text)))
        })?;
        if let Some((identity, text)) = effect {
            let outcome = match (input, text) {
                (Some(id), Some(text)) => adapter.input(&identity, id, &text),
                _ => adapter.cancel(&identity),
            };
            self.store.transaction(task, |slot| {
                let entry = bound_entry(slot, adapter.provider_id())?;
                set_delivery(
                    entry,
                    input,
                    if outcome == EffectOutcome::Confirmed {
                        Delivery::Confirmed
                    } else {
                        Delivery::Unknown
                    },
                )?;
                if input.is_none()
                    && outcome == EffectOutcome::Confirmed
                    && !entry.snapshot.lifecycle.terminal()
                {
                    entry.snapshot.lifecycle = Lifecycle::Cancelled;
                }
                entry.change(ChangeKind::EffectObserved)
            })
        } else {
            self.snapshot(task)
        }
    }

    /// Core supplies evidence from the existing lifecycle owner, never from a
    /// terminal host/plugin claim alone. Process exit does not mean acceptance.
    /// This cannot requeue work or reset a claimed effect for retry.
    pub fn reconcile(
        &self,
        task: &str,
        provider: &str,
        expected_revision: u64,
        lifecycle: Lifecycle,
    ) -> Result<Snapshot> {
        let _execution = self.store.execution_lock(task)?;
        self.store.transaction(task, |slot| {
            let entry = bound_entry(slot, provider)?;
            if entry.snapshot.revision != expected_revision {
                return Err(Error::StaleRevision {
                    current: entry.snapshot.revision,
                });
            }
            if entry.start == Delivery::Pending
                || matches!(
                    lifecycle,
                    Lifecycle::Prepared | Lifecycle::Queued | Lifecycle::Starting
                )
                || (entry.snapshot.lifecycle.terminal() && entry.snapshot.lifecycle != lifecycle)
            {
                return Err(Error::InvalidLifecycle);
            }
            if entry.snapshot.lifecycle == lifecycle {
                return Ok(entry.snapshot.clone());
            }
            entry.snapshot.lifecycle = lifecycle;
            entry.change(ChangeKind::Reconciled)
        })
    }
    fn snapshot(&self, task: &str) -> Result<Snapshot> {
        self.store.transaction(task, |slot| {
            Ok(slot.as_ref().ok_or(Error::NotFound)?.snapshot.clone())
        })
    }
}

/// A core-minted session. It contains no public caller/grant fields and exposes
/// neither the controller nor storage. Reconnect by minting a fresh session;
/// persistent idempotency is scoped to the principal, not the connection.
pub struct Session {
    controller: Controller,
    principal: Principal,
    tasks: BTreeSet<String>,
    operations: BTreeSet<Operation>,
}
impl Session {
    fn authorize(&self, task: &str, operation: Operation) -> Result<()> {
        if !self.tasks.contains(task) || !self.operations.contains(&operation) {
            return Err(Error::Denied);
        }
        Ok(())
    }
}
impl TaskService for Session {
    fn capabilities(&self, version: u32) -> Result<Capabilities> {
        check_version(version)?;
        Ok(Capabilities {
            version: VERSION,
            operations: self.operations.clone(),
            max_input_bytes: MAX_INPUT_BYTES,
            replay_window: REPLAY_WINDOW,
        })
    }
    fn get(&self, version: u32, task: &str) -> Result<Snapshot> {
        check_version(version)?;
        self.authorize(task, Operation::Read)?;
        self.controller.snapshot(task)
    }
    fn effects(&self, version: u32, task: &str) -> Result<Effects> {
        check_version(version)?;
        self.authorize(task, Operation::Read)?;
        self.controller.store.transaction(task, |slot| {
            let entry = slot.as_ref().ok_or(Error::NotFound)?;
            Ok(Effects {
                revision: entry.snapshot.revision,
                start: entry.start,
                cancellation: entry.cancel,
                inputs: entry
                    .inputs
                    .iter()
                    .map(|v| InputStatus {
                        operation_id: v.id.clone(),
                        delivery: v.delivery,
                    })
                    .collect(),
            })
        })
    }
    fn replay(&self, version: u32, task: &str, after: u64) -> Result<Replay> {
        check_version(version)?;
        self.authorize(task, Operation::Replay)?;
        self.controller.store.transaction(task, |slot| {
            let entry = slot.as_ref().ok_or(Error::NotFound)?;
            if after > entry.snapshot.revision {
                return Err(Error::StaleRevision {
                    current: entry.snapshot.revision,
                });
            }
            let oldest = entry
                .changes
                .first()
                .ok_or_else(|| storage("missing replay state"))?
                .revision;
            if after < oldest - 1 {
                return Err(Error::ReplayExpired { oldest });
            }
            Ok(Replay {
                snapshot: entry.snapshot.clone(),
                changes: entry
                    .changes
                    .iter()
                    .filter(|v| v.revision > after)
                    .cloned()
                    .collect(),
            })
        })
    }
    fn mutate(&self, request: Mutation) -> Result<Snapshot> {
        check_version(request.version)?;
        let operation = match &request.action {
            Action::Submit => Operation::Submit,
            Action::Cancel => Operation::Cancel,
            Action::Input(_) => Operation::Input,
        };
        self.authorize(&request.task, operation)?;
        if !valid_token(&request.operation_id) {
            return Err(Error::InvalidRequest);
        }
        if let Action::Input(text) = &request.action
            && (text.trim().is_empty() || text.len() > MAX_INPUT_BYTES)
        {
            return Err(Error::InvalidRequest);
        }
        let digest = crate::util::digest_bytes(&serde_json::to_vec(&request).map_err(storage)?);
        self.controller.store.transaction(&request.task, |slot| {
            let entry = slot.as_mut().ok_or(Error::NotFound)?;
            if entry.snapshot.owner != self.principal {
                return Err(Error::Denied);
            }
            if let Some(receipt) = entry
                .receipts
                .iter()
                .find(|v| v.principal == self.principal && v.id == request.operation_id)
            {
                return if receipt.digest == digest {
                    Ok(receipt.response.clone())
                } else {
                    Err(Error::Conflict)
                };
            }
            if entry.snapshot.revision != request.expected_revision {
                return Err(Error::StaleRevision {
                    current: entry.snapshot.revision,
                });
            }
            if entry.receipts.len() >= MAX_OPERATIONS {
                return Err(Error::Capacity);
            }
            if entry.snapshot.lifecycle.terminal() {
                return Err(Error::InvalidLifecycle);
            }
            let kind = match &request.action {
                Action::Submit => {
                    if entry.snapshot.lifecycle != Lifecycle::Prepared {
                        return Err(Error::InvalidLifecycle);
                    }
                    entry.snapshot.lifecycle = Lifecycle::Queued;
                    ChangeKind::Submitted
                }
                Action::Cancel => {
                    entry.snapshot.cancellation_requested = true;
                    if entry.cancel.is_none() {
                        entry.cancel = Some(Delivery::Pending);
                    }
                    ChangeKind::CancellationRequested
                }
                Action::Input(text) => {
                    if !entry.supports_input {
                        return Err(Error::UnsupportedOperation);
                    }
                    if !matches!(
                        entry.snapshot.lifecycle,
                        Lifecycle::Running | Lifecycle::WaitingForApproval
                    ) || entry.snapshot.cancellation_requested
                    {
                        return Err(Error::InvalidLifecycle);
                    }
                    if entry.inputs.iter().map(|v| v.text.len()).sum::<usize>() + text.len()
                        > 1024 * 1024
                    {
                        return Err(Error::Capacity);
                    }
                    entry.inputs.push(Input {
                        id: request.operation_id.clone(),
                        text: text.clone(),
                        delivery: Delivery::Pending,
                    });
                    ChangeKind::InputAccepted
                }
            };
            let response = entry.change(kind)?;
            entry.receipts.push(Receipt {
                principal: self.principal.clone(),
                id: request.operation_id.clone(),
                digest,
                response: response.clone(),
            });
            Ok(response)
        })
    }
}
fn bound_entry<'a>(slot: &'a mut Option<Entry>, provider: &str) -> Result<&'a mut Entry> {
    let entry = slot.as_mut().ok_or(Error::NotFound)?;
    if entry.snapshot.identity.provider != provider {
        return Err(Error::Denied);
    }
    Ok(entry)
}
fn set_delivery(entry: &mut Entry, input: Option<&str>, delivery: Delivery) -> Result<()> {
    if let Some(id) = input {
        entry
            .inputs
            .iter_mut()
            .find(|v| v.id == id)
            .ok_or(Error::NotFound)?
            .delivery = delivery;
    } else {
        entry.cancel = Some(delivery);
    }
    Ok(())
}
fn check_version(version: u32) -> Result<()> {
    if version == VERSION {
        Ok(())
    } else {
        Err(Error::UnsupportedVersion)
    }
}
fn valid_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}
fn valid_principal(value: &Principal) -> bool {
    match value {
        Principal::Operator(v) | Principal::Plugin(v) | Principal::Worker(v) => valid_token(v),
    }
}

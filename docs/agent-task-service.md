# Core agent-task service V1

`ahu::agent_task::v1::TaskService` is a provider-neutral, in-process task
coordination interface. It is separate from `ahu_host_api::v1::Host`, which
supplies terminal operations. A terminal workspace ID is not a task identity,
and terminal creation, process exit, and assignment acceptance remain different
facts. This contract adds no A2A transport, plugin storage grant, native auth
handling, or remote execution endpoint.

## Ownership and admission

Core constructs a `Controller`, runs its existing launch admission checks, and
registers a frozen `TaskRecord` with `register_admitted` **before any startup
side effect**. Registration is a trusted core integration point, not a replacement
for launch admission and not an API for accepting arbitrary caller records.
Registration freezes the repository and task IDs, execution-provider identity,
agent/harness/model/instruction identity, permissions, policy/configuration and
prompt digests. Re-registering an identical identity returns current state;
changing the owner, identity or input capability is a conflict.

The controller mints a `Session` with a principal, exact task IDs and allowed
operations. Consumers receive only the `TaskService` interface. Caller identity
and grants are not request fields. Core must establish them from its own trusted
entrypoint, and reevaluate them when minting a session after reconnect. Possession
of a task ID is insufficient. List/read/replay require an explicit task grant;
mutations additionally require the recorded owner. Worker sessions have only
list/read/replay rights, even if a caller mistakenly requests mutation grants.
Plugin principals can receive an explicit, scoped session from core; neither the
controller nor its private store is part of the plugin contract. This is an
app boundary, not isolation from arbitrary same-user code.

The ordinary registered-agent, fixed model, context-lock, executable resolution,
parent-attempt/grant, native admission and approval-widening checks stay in core's
launch path. An adapter's availability or capabilities never authorize these
operations. `ExecutionAdapter` receives a frozen identity and literal input,
not a storage handle, shell command or authorization callback.

## Operations and lifecycle

| Operation | Result and requirements |
| --- | --- |
| `capabilities` | Negotiates V1 and reports the session's operations and limits. Unsupported versions fail. |
| `list` | Bounded snapshots of exact core-granted task IDs, in ascending ID order. Requires the `List` operation grant. |
| `get` | Current snapshot, owner, frozen identity, task input support, revision and cancellation intent. |
| `effects` | Current revision and startup, cancellation and input delivery states; no input payloads. |
| `replay` | Changes strictly after a task revision and the current snapshot, read atomically. |
| `mutate(Submit)` | Queues a previously admitted `Prepared` task. It cannot choose or rewrite identity. |
| `mutate(Cancel)` | Records cancellation intent. It does not assert that a provider stopped. |
| `mutate(Input)` | Accepts bounded literal text for a running/waiting task that supports input; refuses after cancellation intent. |

Core calls `dispatch_start`, `dispatch_cancel`, or `dispatch_input` separately.
Accepted input means durably queued, not delivered or read by the model. The
core adapter decides how to use the existing native input or operator inbox
facility and must preserve its ownership rules. V1 does not write legacy inbox
files itself. `Confirmed` cancellation means the adapter established termination;
a lost acknowledgement stays unknown. Cancellation before startup can complete
without invoking the provider. Existing terminal outcomes cannot be overwritten
by late cancellation or reconciliation.

The lifecycle is `Prepared -> Queued -> Starting -> Running` with optional
`WaitingForApproval`, `OutcomeUnknown`, and terminal `Exited`, `Failed` or
`Cancelled`. A negative startup acknowledgement is `Failed`. A claimed startup
whose acknowledgement is lost is `OutcomeUnknown`; a crashed controller can
leave `Starting` with a `Claimed` effect. Neither permits replaying startup.
`Exited` means process completion, not acceptance. V1 has no implicit retry,
resume, owner transfer, provider fallback, or identity mutation. Further
execution after a terminal or indeterminate attempt requires deliberate core
recovery and, if appropriate, a newly admitted task.

## Optional tracker foundation: scoped task listing

An optional tracker adapter can consume task metadata through `TaskService`
without discovering task directories or obtaining repository-wide access.
`list(VERSION, after, limit)` enumerates only the session's frozen set of exact
task grants. The core-minted plugin read session grants `List`, `Read` and
`Replay`. A custom session must grant `List` explicitly; `Read` alone does not
authorize listing, and `List` alone does not authorize other operations.
Plugin installation, enablement, terminal grants and a task ID do not mint a
task session. The trusted core caller must select the plugin identity and scope.

The limit is between 1 and `MAX_PAGE_SIZE` (128), inclusive. Start with `None`;
pass the returned `next_after` to continue. It is an exclusive task-ID cursor,
not a replay revision. A supplied cursor must itself be an exact granted ID;
prefixes, unknown IDs and revoked IDs return `Denied` before storage access.
An empty scope returns an empty page. Missing records return `NotFound` and
unreadable records retain their storage error; neither produces a partial
successful page. Only records on the requested page are read. `next_after`
indicates remaining granted IDs, not evidence that their records exist.

For example, trusted core code can supply a synthetic adapter with two admitted
tasks. `core` below is an existing `Controller`; registration and admission
remain core responsibilities:

```rust,ignore
use ahu::agent_task::v1::{TaskService, VERSION};

let reader = core.plugin_read_session(
    "sample-tracker",
    [
        "00000000-0000-7000-8000-000000000001".to_owned(),
        "00000000-0000-7000-8000-000000000002".to_owned(),
    ].into(),
)?;
// Hand only this interface to the adapter, never the controller.
let service: &dyn TaskService = &reader;
let first = service.list(VERSION, None, 1)?;
if let Some(after) = first.next_after.as_deref() {
    let second = service.list(VERSION, Some(after), 1)?;
    assert!(second.next_after.is_none());
}
```

Memory and durable controllers read current snapshots, each coherent under its
task lock; a page is not a transaction across tasks. An `Inspection` session
remains frozen until reopened. Reconnecting requires a fresh core-authorized
session; cursors do not restore revoked grants. Consumers should negotiate
`Operation::List`; older `TaskService` implementations default to
`UnsupportedOperation` and must not advertise that operation.

The service supplies in-process task reads. The optional
[executable integration](executable-plugins.md#read-only-taskservice-invocation)
uses core-minted exact grants to collect a bounded, versioned projection and
passes it through generic plugin input. It exposes no service callback or
controller. Neither path adds a tracker client, external record mapping, credential
handling, synchronization policy, network calls or CLI commands. Existing task
execution works
without any tracker provider or terminal plugin. A future tracker integration
still needs a separately reviewed external data boundary and synchronization
policy; executable activation alone authorizes neither tracker writes nor task
mutations. A task's process exit does not establish issue acceptance
or closure.

## Safe replay, concurrency and recovery

Every mutation includes V1, task ID, operation ID, expected revision and action.
Operation IDs are scoped to the task and owning principal. Repeat the **exact**
request after a lost response. The stored original response is returned even if
the expected revision has since become stale. Reusing an operation ID for any
different request is a conflict. Authorization is checked before reading a
receipt, so a newly restricted session cannot replay a revoked operation.
A new request with a stale revision returns the current revision without writing.

Snapshots, receipts, intent and replay changes are published as one transaction.
Every successful state change increments a checked monotonic task revision.
Clients can replay the last 128 bounded coordination changes; an older cursor returns
`ReplayExpired`, and clients recover with `get`/`effects` and continue from that
revision. These changes are lifecycle metadata, not captured execution events.
Future cursors are refused. Receipt retention is independent of replay: receipts
are never evicted to make an old operation executable again.

Before invoking an effect, core durably records a one-shot claim. An execution
ownership lock remains held through the callback and acknowledgement write;
metadata requests remain available. Another controller gets `Busy` during the
callback. This prevents a concurrent cancellation from confirming termination
before startup returns. A crash releases the OS lock but retains the claim.
`Claimed` or `Unknown` effects return `OutcomeUnknown` instead of invoking the
adapter again. Core can reconcile lifecycle evidence using a revision check;
reconciliation cannot reset claims or queue execution again.

This provides **at-most-once adapter invocation**, not exactly once execution.
A crash between writing a claim and invoking the adapter can leave work that
never started. A crash after invocation can leave running work with no reply.
Core must inspect existing supervisor/native-reference evidence and reconcile;
it must never infer non-execution from a timeout. An I/O failure after atomic
publication can also lose an acknowledgement: reconnect and inspect/retry the
same operation, preserving its ID. Do not delete the coordination store or its
persistent lock files as a recovery technique.

Input is bounded to 64 KiB per request and 1 MiB per task. At most 256 mutation
receipts are retained; reaching that bound returns `Capacity` without a state
change. Read, replay, core effect dispatch and reconciliation remain available.
Records are capped at 4 MiB. These conservative bounds are explicit V1 limits,
not silent truncation or garbage collection.

## Durable local implementation and existing records

`Controller::memory` supplies the same state machine and receipts for conformance
and embedding tests. `Controller::local` derives its store from the selected
repository using `RepositoryStorage`, under the primary checkout:

```text
.ahu/state/repos/<repository-identity>/agent-tasks-v1/
  <task-id>.json
  <task-id>.lock
  <task-id>.execution.lock
```

Linked checkouts share coordination. No ambient state override, plugin path or
unrelated repository is accepted. The implementation reuses the existing state
confinement, descriptor ownership checks, task state locks and durable atomic
write helpers. New directories/files are owner-only. Reads are bounded and
refuse symlinks, multiple hard links, unsafe modes, repository/task mismatches, corrupt
records and unknown versions. Lock files persist; kernel ownership,
not an age or recorded PID, controls exclusion. Cooperative races are serialized;
this does not defend against an arbitrary hostile same-user process replacing
ancestry or altering the records and their digests.

`track_existing` attaches records found by the existing repository task discovery
and ownership checks. It never rewrites `task.json`, moves a legacy record,
imports native history, or infers permission to start an existing task again.
An existing `Starting` record becomes `OutcomeUnknown` for inspection. Further
observations must come through explicit core `reconcile` calls. Service receipts
stay in primary-owned coordination when a worktree is removed; cleanup never
erases the operation history.

## Integration status and validation

`agent_task::inspection` routes `ahu tasks` (human and JSON), `ahu task`, and
the approved MCP tools `ahu_tasks_list` and `ahu_task_get` through the same
core-minted `Session::get` authorization. Their existing JSON schemas and legacy
task IDs remain unchanged. Exact IDs, unique prefixes, handles and validated
repository index pointers resolve through the same core lookup. Human task
listing observes liveness without reconciling or rewriting task records.
The local CLI and stdio MCP inspection entrypoints retain repository read access;
JSON arguments cannot supply a principal or grants. MCP Tasks extension jobs
retain their separate caller ownership, cancellation and subscription protocol;
their inspection work uses this bridge too.

`Inspection::open` is a trusted core factory. It captures a read-only view of
ownership-validated task records and, where present, atomically published V1
state. Existing V1 owner, provider, identity, revisions, effect claims and replay
history are preserved. Conflicting identity, corrupt state and unsafe storage
fail closed. It creates no V1 directories or locks, calls no execution adapter,
and never invokes `track_existing`, registration or reconciliation. Compatibility
output continues to describe the legacy record and existing liveness evidence;
it does not pretend those observations updated durable service state.

For records without V1 state, the view contains one session-local `Reconciled`
observation at revision 1. `Starting` maps to `OutcomeUnknown`. Provider and owner
identify `ahu.legacy-core`, not a terminal host or the reading plugin. Startup
delivery is unknown and input support is false; no historical effects or native
events are inferred. Replay after 0 returns that observation, after 1 is empty,
and a future cursor is refused. **Legacy observation cursors apply only within
that immutable view; they are not reconnect cursors.** Reopening refreshes legacy
observations without migrating them. Existing V1 cursors retain their durable
meaning and bounded expiry. A view is coherent per task, not a transaction across
all tasks, and remains frozen until core opens another view.

Core can give a plugin `Inspection::plugin_read_session` or
`Controller::plugin_read_session`, selecting the plugin identity and exact task
IDs itself. Only the opaque `TaskService` is handed to the consumer. The grant
contains list/read/replay only, never prefix matching or repository discovery. IDs
lacking a grant are denied before lookup; explicitly granted missing IDs are
`NotFound`. Every submit/cancel/input request is denied, even for an owning
plugin. Core must reevaluate grants when reconnecting. This is an in-process
capability boundary, not isolation from arbitrary same-user code. The optional
`plugin_exec::task_service::TaskReadGrant` binds a controller read session to
the generic host grant's repository and plugin identity. Its invocation path
exports only selected lifecycle observations, never the full service snapshot,
a terminal protocol extension or a plugin-selected storage path.

CLI and MCP task inspection use the service's scoped `Inspection` interface.
Existing launch, cancel, approval and message commands, plus headless wait/result
attempt inspection, remain on their current core paths. This change adds no
production `ExecutionAdapter`. A future adapter integration must place
registration before startup and retain the existing launch/admission and native
approval checks; wrapping an already executed launch in `register_admitted`
would be incorrect. Consumers cannot use `Submit` to bypass that prerequisite.

`tests/agent_task_service.rs` runs the same conformance scenarios against memory
and durable state using synthetic adapters. It covers authorization, versions,
identity conflicts, duplicates, stale revisions, replay and reconnect, cancellation
intent, literal input, lost responses, provider failure, concurrency, persisted
claims and storage refusal. It starts no native provider session and reads no
native auth stores. Scoped listing tests cover pagination, exact grants, revoked
cursors, bounds, missing records and storage errors against memory and durable
stores. `tests/task_inspection.rs` additionally checks CLI/MCP response
and permission equivalence, denied and unknown tasks, immutable plugin scopes,
replay boundaries, preserved ownership and no implicit migration. Live provider
behavior, execution routing and native history replay are not claimed.

See [system architecture](knowledge/ahu-architecture.md),
[task state](knowledge/task-state.md), [task communication](knowledge/task-communication.md),
and [terminal Host V1](../crates/ahu-host-api/src/v1.rs).

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
of a task ID is insufficient. Read/replay require an explicit task grant;
mutations additionally require the recorded owner. Worker sessions have only
read/replay rights, even if a caller mistakenly requests mutation grants.
Plugin principals can receive an explicit, scoped session from core; neither the
controller nor its private store is part of the plugin contract. This is an
application boundary, not isolation from arbitrary same-user code.

The ordinary registered-agent, fixed model, context-lock, executable resolution,
parent-attempt/grant, native admission and approval-widening checks stay in core's
launch path. An adapter's availability or capabilities never authorize these
operations. `ExecutionAdapter` receives a frozen identity and literal input,
not a storage handle, shell command or authorization callback.

## Operations and lifecycle

| Operation | Result and requirements |
| --- | --- |
| `capabilities` | Negotiates V1 and reports the session's operations and limits. Unsupported versions fail. |
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
`Exited` means process completion, not acceptance. There is no implicit retry,
resume, owner transfer, provider fallback, or identity mutation in V1. Further
execution after a terminal or indeterminate attempt requires deliberate core
recovery and, if appropriate, a newly admitted task.

## Idempotency, concurrency and recovery

Every mutation includes V1, task ID, operation ID, expected revision and action.
Operation IDs are scoped to the task and owning principal. Repeat the **exact**
request after a lost response. The stored original response is returned even if
the expected revision has since become stale. Reusing an operation ID for any
different request is a conflict. Authorization is checked before reading a
receipt, so a newly restricted session cannot replay a revoked operation.
A new request with a stale revision returns the current revision without writing.

Snapshots, receipts, intent and replay changes are published as one transaction.
Every successful state change increments a checked monotonic task revision.
The last 128 bounded coordination changes are replayable; an older cursor returns
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
reconciliation cannot reset claims or requeue execution.

This provides **at-most-once adapter invocation**, not exactly-once execution.
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
refuse symlinks, hardlinks, unsafe modes, repository/task mismatches, corrupt
records and unknown versions. Locks are persistent inodes; kernel ownership,
not an age or recorded PID, controls exclusion. Cooperative races are serialized;
this does not defend against an arbitrary hostile same-user process replacing
ancestry or altering the records and their digests.

`track_existing` attaches records found by the existing repository task discovery
and ownership checks. It never rewrites `task.json`, moves a legacy record,
imports native history, or infers permission to start an existing task again.
An existing `Starting` record becomes `OutcomeUnknown` for inspection. Further
observations must come through explicit core `reconcile` calls. Service receipts
stay in primary-owned coordination when a worktree is removed; no automatic
cleanup erases idempotency history.

## Integration status and validation

V1, both storage implementations, core dispatch gates and existing-record
attachment are library facilities. Existing CLI/MCP launch, cancel and message
commands are not yet routed through this interface. There is no production
`ExecutionAdapter` in this change. A future adapter integration must place
registration before startup and retain the existing launch/admission and native
approval checks; wrapping an already executed launch in `register_admitted`
would be incorrect. Consumers cannot use `Submit` to bypass that prerequisite.

`tests/agent_task_service.rs` runs the same conformance scenarios against memory
and durable state using synthetic adapters. It covers authorization, versions,
identity conflicts, duplicates, stale revisions, replay and reconnect, cancellation
intent, literal input, lost responses, provider failure, concurrency, persisted
claims and storage refusal. It starts no native provider session and reads no
native auth stores. Live provider behavior and command routing are not claimed.

See [system architecture](knowledge/ahu-architecture.md),
[task state](knowledge/task-state.md), [task communication](knowledge/task-communication.md),
and [terminal Host V1](../crates/ahu-host-api/src/v1.rs).

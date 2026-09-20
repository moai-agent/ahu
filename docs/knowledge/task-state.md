---
type: Architecture
title: Task state
description: Interactive and headless task records, discovery and integrity boundaries.
tags: [worktrees, state]
status: draft
sources:
  - id: state
    resource: ../../src/state.rs
    title: State paths and confinement
  - id: task
    resource: ../../src/task.rs
    title: Record storage and discovery
  - id: index
    resource: ../../src/task_index.rs
    title: Cross-checkout pointer index
  - id: launch
    resource: ../../src/launch.rs
    title: Task planning and runtime checks
  - id: tests
    resource: ../../tests/task_state_lifecycle.rs
    title: Worktree state lifecycle tests
---

# Task state

Each interactive task's `task.json` and `prompt.txt` live under its own worktree at
`.ahu/state/repos/<repo-identity>/tasks/<task-id>/`. Task directories are named by
the task's UUID v7 identifier; record schema 3 writes these identifiers, while
records written by schema 2 keep their original 18-character hex identifiers and
load unchanged. Launch derives this path without an environment override, and
registers the task in the cross-checkout index once durable state exists. Removing
the worktree removes its state and its index entry; records include session
status, which does not prove task
completion.[^state][^task][^index][^launch][^tests]

Headless tasks instead store records, prompts and per-attempt results outside Git
checkouts, under the default home runtime directory or `AHU_RUNTIME_DIR`.
Discovery includes this external store. Worktree deletion retains headless
results; supervisor loss reports an interrupted attempt without automatic replay.
Process and harness outcomes remain separate from acceptance: agent reports and
same-user editable records do not prove completion. The headless implementation
in src/headless.rs defines this state store; interactive state rules below do not
relocate it through `AHU_STATE_DIR`.

Headless child grants freeze registered identities and native policies at host
submission. The broker in src/broker.rs binds requests to a live parent attempt
and dispatches the registered child with its own harness policy outside the
worker sandbox. Descendants cannot expand the grant. Failed or unjoined children
from the current attempt block parent success. Native helper joins are recorded
separately from registered task IDs; src/native.rs defines the bounded Claude
2.1.270 profile, which supplies read-only model tools to both owner and helpers;
this does not prove settings-defined hooks cannot write. A child's retained
parent-attempt binding prevents resume once that parent terminates or closes
admission. Further work needs a new registered assignment with explicit source
scope, rather than silently detaching the old task from its provenance. Same-user code is
not isolated from supervisor records, and provider-side cleanup remains unknown.

Explicit cleanup removes captured attempt artifacts after known termination,
retaining structured results, frozen inputs, native stores, branches, and worktrees.
It is not a full erasure of task content.

Interactive task discovery scans sibling task worktrees under the primary checkout's
`.worktrees/`, then compatible legacy checkout stores. Discovery then consults the
cross-checkout index for task IDs no local record holds, which makes tasks
reachable from any checkout of the repository that launched them. Worktree records
take
precedence for duplicate IDs. The primary checkout and siblings can discover
these tasks. Every scan of a managed worktree store enforces its owner's task ID,
repository identity and canonical worktree path. It is not rescanned as a legacy
store. Primary, invoking plain-checkout and explicit external legacy stores
remain readable; older child records in a parent task worktree are reported as
misplaced, without automatic acceptance or migration.[^task][^index][^tests]

Misplaced entries in managed stores produce warning notes, not task rows; files
remain untouched. Unreadable owner records and identity mismatches in legacy
stores produce unreadable rows. After legacy lookup, an unaccounted-for worktree
is reported as incomplete even if it contains stray records.[^task][^tests]

Failed launches attempt non-forced Git worktree removal. A refusal reports the
retained worktree and branch. Partial state cleanup validates the path before
attempting removal, leaves redirected paths alone and can fail. A retained
recordless worktree is visible as incomplete; listing does not delete it or
prove its contents disposable.[^launch][^tests]

The launch lock and cmux group mapping coordinate siblings in the primary
checkout's state store. Hygiene and generated architecture text use the current
checkout's store. `AHU_STATE_DIR` selects auxiliary state. For coordination and
legacy lookup, a value naming `.ahu/state` of any checkout in the same Git
repository is recognized as automatic session wiring: coordination stays in the
primary checkout, and legacy lookup uses the primary and invoking plain-checkout
stores. Managed stores retain owner checks. A subdirectory's `.ahu/state` does
not qualify as checkout-root wiring. Other values replace those coordination and legacy stores. Neither case relocates
new interactive task records or suppresses worktree discovery. The
interactive harness receives its own worktree's state root as `AHU_STATE_DIR`.[^state][^task][^launch]

State access refuses existing symlinks in default store paths and below explicit
state roots; an explicit root outside the checkout-store layout is user-selected.
Path checks do not prevent concurrent replacement. Startup checks prompt and
delivery digests, command reconstruction and repository/worktree identity. A
writer able to alter records and digests can alter both; these checks are not
authentication or OS isolation.[^state][^launch]

The state reference in docs/reference.md at the repository root details override
path semantics. [Task identity](task-identity.md) records the identifier grammar,
the index, and global resolution; [Task communication](task-communication.md)
records the operator inbox and task artifacts. [Configuration inheritance](task-configuration-inheritance.md)
explains what enters a worktree through the launch snapshot.

[^state]: State path derivation, coordination and confinement in src/state.rs.
[^task]: Record storage and discovery in src/task.rs.
[^index]: Cross-checkout pointer entries in src/task_index.rs.
[^launch]: Planning, execution and runtime checks in src/launch.rs.
[^tests]: Automatic placement, discovery, deletion and refusal cases in tests/task_state_lifecycle.rs.

---
type: Architecture
title: Task state
description: Interactive and headless task records, discovery and integrity boundaries.
tags: [worktrees, state]
status: draft
generated: { by: docs-astra/1.1.1, at: 2026-09-20T03:22:38Z }
sources:
  - id: state
    resource: ../../src/state.rs
    title: State paths and confinement
  - id: storage
    resource: ../../src/storage.rs
    title: Primary coordination and explicit legacy lookup
  - id: task
    resource: ../../src/task.rs
    title: Record storage and discovery
  - id: index
    resource: ../../src/task_index.rs
    title: Cross-checkout pointer index
  - id: launch
    resource: ../../src/launch.rs
    title: Task planning and runtime checks
  - id: headless
    resource: ../../src/headless.rs
    title: Primary-owned coordination and attempt lifecycle
  - id: commands
    resource: ../../src/commands.rs
    title: Task inspection and coordinator sessions
  - id: tests
    resource: ../../tests/task_state_lifecycle.rs
    title: Worktree state lifecycle tests
---

# Task state

Each interactive task's `task.json` and `prompt.txt` live under its own worktree at
`.ahu/state/repos/<repo-identity>/tasks/<task-id>/`. Task directories are named by
a version-7 universally unique identifier; record schema 3 writes these IDs, while
records written by schema 2 keep their original 18-character hex identifiers and
load unchanged. Launch derives this path without an environment override, and
registers the task in the cross-checkout index once durable state exists. Removing
the worktree removes its local state; `ahu remove` also removes the index
entry. Records include session status, which does not prove task completion.[^state][^task][^index][^launch][^tests]

Headless tasks keep minimal coordination under
`<primary-checkout>/.ahu/state/repos/<repo-identity>/headless/<task-id>/`.
This includes frozen prompts, grants, broker requests, bounded outcome metadata,
and native session references. Native events, stderr, final text, and helper
summaries are not copied. Native settings, skills, and histories stay native.
Worktree deletion retains primary-owned coordination; supervisor loss reports an
interrupted attempt without automatic replay. A native reference is a locator,
not permission to import history; unverified locations remain unknown. New
headless specs/results use schema 2. Schema-1 attempts remain readable in place,
but this binary refuses their resume and child dispatch. Use the original runner
or a new assignment; arbitrary legacy-worker use of a new binary is not supported.[^headless]

Headless child grants freeze registered identities and native policies at host
submission. The broker in src/broker.rs binds requests to a live parent attempt
and dispatches the registered child with its own harness policy outside the
worker sandbox. Descendants cannot expand the grant. Failed or unjoined children
from the current attempt block parent success. Native helper joins are recorded
separately from registered task IDs; src/native.rs defines the bounded Claude
2.1.270 profile, which supplies read-only model tools to both owner and helpers;
this does not prove settings-defined hooks cannot write. Resuming a registered
child or requesting resume from a worker is refused, including while its parent
is live. The retained parent-attempt binding remains
intact. Further work needs a new registered assignment with explicit source
scope, rather than silently detaching the old task from its provenance. Same-user code is
not isolated from supervisor records, and provider-side cleanup remains unknown.

Explicit cleanup removes recognized old captures if present and bounded request
entries after known termination, retaining results, frozen inputs, private broker
claims/responses, native stores, branches, and worktrees.
It is not a full erasure of task content.

Interactive task discovery scans sibling task worktrees under the primary checkout's
`.worktrees/`, then compatible legacy checkout stores. Discovery then consults the
cross-checkout index for task IDs no local record holds, which makes tasks
reachable from any checkout of the repository that launched them. Worktree
records take precedence for duplicate IDs. The primary checkout and siblings can discover
these tasks. Every scan of a managed worktree store enforces its owner's task ID,
repository identity and canonical worktree path. It is not rescanned as a legacy
store. Primary and invoking plain-checkout legacy stores remain readable. Older
child records in a parent task worktree are reported as misplaced, without
automatic acceptance or migration.[^task][^index][^tests]

Misplaced entries in managed stores produce warning notes, not task rows; files
remain untouched. Unreadable owner records and identity mismatches in legacy
stores produce unreadable rows. After legacy lookup, an unaccounted-for worktree
is reported as incomplete even if it contains stray records.[^task][^tests]

Failed launches attempt non-forced Git worktree removal. A refusal reports the
retained worktree and branch. Partial state cleanup validates the path before
attempting removal, leaves redirected paths alone and can fail. A retained
recordless worktree is visible as incomplete; listing does not delete it or
prove its contents disposable.[^launch][^tests]

The launch lock, cmux group mapping, headless coordination, and scoped task index
belong to the primary checkout’s state store. Hygiene and generated architecture text use the invoking
checkout's store. Paths come from explicit checkout and repository discovery.
ahu neither resolves state through `AHU_STATE_DIR` nor injects it into sessions.
The `--repo` prefix selects a checkout; `AHU_REPO_ROOT`, `AHU_STATE_DIR`,
`AHU_RUNTIME_DIR`, and `AHU_TASK_INDEX_DIR` do not select stores. Conventional old
external roots and explicit `legacy-lookup.json` roots are read in place without
migration.[^state][^storage][^task][^launch]

Coordinator shortcuts preserve the invoking directory and configured model.
`ahu codex` requests `--dangerously-bypass-approvals-and-sandbox`; `ahu claude`
requests `--dangerously-skip-permissions`. Each discloses its flag. These sessions
create no task or worktree; registered children keep their own manifest
permissions and required launch grants.[^commands]

Headless inspection separates recorded session state from observed supervisor
ownership. It reports attempt number and outcome, blockers, known native session
identity with provenance, artifact locations, and review commands. Metadata
reads and escaped display fields are bounded; unavailable metadata and unknown
ownership remain explicit. Session checkpoint schema 2 binds evidence to the task,
attempt, and harness; invalid checkpoints remain unavailable. Lock inspection
checks the opened file’s owner, permissions, and link count before reporting
liveness. Inspection does not scan native transcripts or infer
assignment acceptance. `wait` validates result envelopes, refusing malformed
values and results for another task or attempt, without imposing the 1 MiB
inspection limit on its full-envelope API. Internal lifecycle result reading
remains separate from inspection and its display bounds.[^headless][^commands]

Interactive cancellation uses the live run-task owner and retains work. Only
confirmed cancellation closes the recorded workspace. Unconfirmed requests and
tasks that finish on their own leave it open. Terminal outcomes persist even if
foreground restoration fails.[^commands][^launch]

State access refuses existing symlinks in checkout store paths. Path checks do
not prevent concurrent replacement. Startup checks prompt and delivery digests,
command reconstruction, and repository/worktree identity. A writer able to alter
records and digests can alter both; these checks are not authentication or OS
isolation.[^state][^launch]

The state reference in docs/reference.md at the repository root details storage
paths and explicit legacy lookup configuration. [Task identity](task-identity.md) records
the identifier grammar, the index, and scoped resolution; [Task communication](task-communication.md)
records the operator inbox and task artifacts. [Configuration inheritance](task-configuration-inheritance.md)
explains what enters a worktree through the launch snapshot.

[^state]: State path derivation, coordination and confinement in src/state.rs.
[^task]: Record storage and discovery in src/task.rs.
[^index]: Cross-checkout pointer entries in src/task_index.rs.
[^launch]: Planning, execution and runtime checks in src/launch.rs.
[^tests]: Automatic placement, discovery, deletion and refusal cases in tests/task_state_lifecycle.rs.
[^headless]: Primary coordination, attempt inspection, and lifecycle decisions in src/headless.rs.
[^commands]: Inspection output and coordinator flags in src/commands.rs.

[^storage]: Primary-owned paths and bounded legacy lookup configuration in src/storage.rs.

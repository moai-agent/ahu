---
type: Architecture
title: Task state
description: Worktree-local task records, discovery and integrity boundaries.
tags: [worktrees, state]
status: draft
sources:
  - id: state
    resource: ../../src/state.rs
    title: State paths and confinement
  - id: task
    resource: ../../src/task.rs
    title: Record storage and discovery
  - id: launch
    resource: ../../src/launch.rs
    title: Task planning and runtime checks
  - id: tests
    resource: ../../tests/task_state_lifecycle.rs
    title: Worktree state lifecycle tests
---

# Task state

Each task's `task.json` and `prompt.txt` live under its own worktree at
`.ahu/state/repos/<repo-identity>/tasks/<task-id>/`. Launch derives this path
without an environment override. Removing the worktree removes its state;
records include session status, which does not prove task completion.[^state][^launch][^tests]

Task discovery scans sibling task worktrees under the primary checkout's
`.worktrees/`, then compatible legacy checkout stores. Worktree records take
precedence for duplicate IDs. The primary checkout and siblings can discover
these tasks. Every scan of a managed worktree store enforces its owner's task ID,
repository identity and canonical worktree path. It is not rescanned as a legacy
store. Primary, invoking plain-checkout and explicit external legacy stores
remain readable; older child records in a parent task worktree are reported as
misplaced, without automatic acceptance or migration.[^task][^tests]

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
new task records or suppresses worktree discovery. The
harness receives its own worktree's state root as `AHU_STATE_DIR`.[^state][^task][^launch]

State access refuses existing symlinks in default store paths and below explicit
state roots; an explicit root outside the checkout-store layout is user-selected.
Path checks do not prevent concurrent replacement. Startup checks prompt and
delivery digests, command reconstruction and repository/worktree identity. A
writer able to alter records and digests can alter both; these checks are not
authentication or OS isolation.[^state][^launch]

The state reference in docs/reference.md at the repository root details override
path semantics. [Configuration inheritance](task-configuration-inheritance.md)
explains what enters a worktree through the launch snapshot.

[^state]: State path derivation, coordination and confinement in src/state.rs.
[^task]: Record storage and discovery in src/task.rs.
[^launch]: Planning, execution and runtime checks in src/launch.rs.
[^tests]: Automatic placement, discovery, deletion and refusal cases in tests/task_state_lifecycle.rs.

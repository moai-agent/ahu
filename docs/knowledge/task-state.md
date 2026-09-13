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
these tasks; refused task directories remain visible as unreadable.[^task][^tests]

The launch lock and cmux group mapping coordinate siblings in the primary
checkout's state store. Hygiene and generated architecture text use the current
checkout's store. `AHU_STATE_DIR` replaces auxiliary state and, when lexically
different from the default path, coordination and default legacy-store lookup.
It neither relocates new task records nor suppresses worktree discovery. The
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

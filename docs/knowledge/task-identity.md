---
type: Architecture
title: Task identity
description: Global task IDs, their grammar, the cross-checkout pointer index, and resolution semantics.
tags: [worktrees, state, security]
status: draft
sources:
  - id: task
    resource: ../../src/task.rs
    title: Task ID generation and record schema
  - id: index
    resource: ../../src/task_index.rs
    title: Cross-checkout pointer index
  - id: commands
    resource: ../../src/commands.rs
    title: Normalization and resolution
  - id: launch
    resource: ../../src/launch.rs
    title: Registration at launch and removal on rollback
  - id: headless
    resource: ../../src/headless.rs
    title: Headless registration
  - id: tests
    resource: ../../tests/task_resolution.rs
    title: Task resolution regression tests
---

# Task identity

A task ID is a hyphenated UUID v7: 36 lowercase hex digits with a 48-bit
big-endian millisecond timestamp, so IDs sort by launch time as plain strings,
followed by 74 random bits drawn from operating-system entropy. ID generation
fails closed rather than minting a guessable ID when entropy is unavailable.
The URN spelling `ahu:task:<uuid>` may appear in prompt metadata; commands
accept it, the bare hyphenated form, uppercase, or an unambiguous prefix. The
bare form is what records and stores hold. An empty ID after normalization is
refused as a usage error.[^task][^commands]

Record schema 3 writes these UUID IDs; schema 2 wrote 18-character hex IDs.
The fields are otherwise identical, so schema-2 records load unchanged and
keep their original IDs. The build reads exactly schemas 2 and 3 and refuses
schema 1, whose digest field reinterprets file bytes as delivered bytes.[^task]

The cross-checkout task index maps IDs to where their durable state lives. It
holds one small entry per task: the ID, the repository identity, the
checkout, and which store kind holds it, and nothing else. It lives outside
every Git checkout, by default
under `$HOME/.local/state/ahu/task-index` or `AHU_TASK_INDEX_DIR`, which must
name an absolute private directory outside repositories. The index is user
state, never migrated in place and never rewritten to match records it cannot
read.[^index]

Registration happens at launch, for interactive and headless tasks alike;
rollback and task removal remove the entry.
The index accepts canonical UUID IDs and absolute checkout paths
only.[^index][^launch][^headless]

Resolution consults local records first, then the index for IDs that are not
local, so a task is reachable from any checkout of the repository that
launched it. Exact IDs resolve before prefixes. Ambiguous prefixes, including
unreadable candidates, are refused with a usage error naming each match. A
pointer whose checkout or task directory has disappeared is refused as stale
rather than guessed at, and a record that describes a different task than its
index entry names is refused as hostile. An unreadable record resolves to its
disclosure error instead of a match.[^commands][^tests]

Task IDs are not capabilities. The index is a locator for the operator who
launched the task, not an authority mechanism: reading a task still requires
access to its store, and delivery to a running task is operator-only, as
[Task communication](task-communication.md) details. [Task state](task-state.md)
explains the stores these IDs address.

[^task]: ID generation, schema versions and readable-version gates in src/task.rs.
[^index]: Entry layout, index-root rules, registration and lookup in src/task_index.rs.
[^commands]: Normalization, resolution ordering and refusal wording in src/commands.rs.
[^launch]: Registration at launch and index removal on rollback in src/launch.rs.
[^headless]: Headless registration in src/headless.rs.
[^tests]: Resolution, ambiguity and stale-entry cases in tests/task_resolution.rs.
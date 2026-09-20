---
type: Architecture
title: Task identity
description: Task IDs, their grammar, the repository-scoped pointer index, and resolution semantics.
tags: [worktrees, state, security]
status: draft
sources:
  - id: task
    resource: ../../src/task.rs
    title: Task ID generation and record schema
  - id: index
    resource: ../../src/task_index.rs
    title: Repository-scoped pointer index
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

A task ID is a hyphenated version-7 universally unique identifier. Its 36
characters contain hyphens and lowercase hexadecimal digits. A 48-bit big-endian
millisecond timestamp lets IDs sort by launch time as plain strings; 74 random
bits come from operating-system entropy. ID generation
fails closed rather than minting a guessable ID when entropy is unavailable.
The typed spelling `ahu:task:<uuid>` may appear in prompt metadata; commands
accept it, the bare hyphenated form, uppercase, or an unambiguous prefix. The
bare form is what records and stores hold. An empty ID after normalization is
refused as a usage error.[^task][^commands]

Record schema 3 writes these version-7 IDs; schema 2 wrote 18-character hex IDs.
The fields are otherwise identical, so schema-2 records load unchanged and
keep their original IDs. The build reads exactly schemas 2 and 3 and refuses
schema 1, whose digest field reinterprets file bytes as delivered bytes.[^task]

The repository-scoped task index maps IDs to durable state. Each entry holds the
ID, repository identity, checkout, and store kind. New schema-2 entries live at
`<primary-checkout>/.ahu/state/task-index/`, with owner-only permissions and
repository checks. Ambient index overrides do not select this store. Compatible
old external entries remain readable in place and are never rewritten by lookup;
explicit additional roots use the primary checkout’s `legacy-lookup.json`.
Unrelated repositories do not share a global lookup scope.[^index]

Registration happens at launch, for interactive and headless tasks alike;
rollback and task removal remove the primary entry. Old external entries remain
untouched.
The index accepts canonical version-7 IDs and absolute checkout paths
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
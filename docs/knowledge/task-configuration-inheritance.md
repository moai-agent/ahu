---
type: Architecture
title: Task configuration inheritance
description: How task worktrees inherit recognized agent configuration.
tags: [worktrees, configuration]
status: draft
generated: { by: docs-opus/1.0.0, at: 2026-09-14T03:51:17Z }
sources:
  - id: snapshot
    resource: ../../src/snapshot.rs
    title: Configuration collection and materialization
  - id: launch
    resource: ../../src/launch.rs
    title: Task worktree launch pipeline
  - id: lock
    resource: ../../src/context_lock.rs
    title: Committed-context lock checks
  - id: inheritance-tests
    resource: ../../tests/worktree_inheritance.rs
    title: Worktree inheritance tests
---

# Task configuration inheritance

A task worktree starts from the parent checkout's HEAD. Before planning a launch,
ahu compares shared recognized context with committed `ahu.lock` and requires
those inputs and the lock to be tracked and clean. Recognized user-local Claude
settings are checked against a separate owner-only acceptance in host state,
scoped by user and repository. A change blocks that user's launch until
explicitly accepted with `ahu lock --update`; it does not modify the shared lock.
ahu never stages or commits project files.[^launch][^snapshot][^lock]

The snapshot recognizes `.agents`, `.claude`, `.codex`, `.agent`, `.opencode`, and `.gemini`
directories, along with supported instruction and configuration filenames,
including `GEMINI.md`, `opencode.json` and `opencode.jsonc`. It records content
digests and executable bits. User-local settings needed by a harness still
travel with the task snapshot, but their fingerprints are kept in private host
state rather than `ahu.lock`. Collection has a depth limit and skips directories
such as `.git`, `.worktrees`, and build output; it does not cover every possible
configuration location. The lock records recognized configuration it could not
scan, along with symlinks, and refuses those cases. It cannot discover all nested
or harness-managed context. Committed files in skipped directories can still
arrive through the base checkout.[^snapshot][^lock]

Configuration symlinks are recorded separately and are not followed as snapshot
entries. Materialization reconciles symlinks in the destination so configuration
writes remain inside the worktree. Tests cover dirty configuration inheritance,
source isolation, digest changes, and symlink handling.[^snapshot][^inheritance-tests]

[Named agent identity](named-agent-identity.md) describes the manifests carried
within this configuration.

[^snapshot]: Collection rules and materialization in src/snapshot.rs.
[^launch]: Planning and execution in src/launch.rs.
[^inheritance-tests]: Inheritance and containment cases in tests/worktree_inheritance.rs.
[^lock]: Admission and lockfile logic in src/context_lock.rs.

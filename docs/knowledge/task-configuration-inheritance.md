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
  - id: inheritance-tests
    resource: ../../tests/worktree_inheritance.rs
    title: Worktree inheritance tests
---

# Task configuration inheritance

A task worktree starts from the parent checkout's HEAD. Recognized agent
configuration is then materialized from a snapshot taken at submission, including
uncommitted and ignored additions, modifications, and deletions. Unrelated dirty
source files are not copied.[^launch][^snapshot]

The snapshot recognizes `.agents`, `.claude`, `.codex`, `.agent`, and `.opencode`
directories, along with supported instruction and configuration filenames,
including `opencode.json` and `opencode.jsonc`. It records content
digests and executable bits. Collection has a depth limit and skips directories
such as `.git`, `.worktrees`, and build output; the scan is not an exhaustive
inventory of every possible configuration location. Files already committed in
skipped directories can still arrive through the base checkout.[^snapshot]

Configuration symlinks are recorded separately and are not followed as snapshot
entries. Materialization reconciles symlinks in the destination so configuration
writes remain inside the worktree. Tests cover dirty configuration inheritance,
source isolation, digest changes, and symlink handling.[^snapshot][^inheritance-tests]

[Named agent identity](named-agent-identity.md) describes the manifests carried
within this configuration.

[^snapshot]: Collection rules and materialization in src/snapshot.rs.
[^launch]: Planning and execution in src/launch.rs.
[^inheritance-tests]: Inheritance and containment cases in tests/worktree_inheritance.rs.

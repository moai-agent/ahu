---
type: Architecture
title: Utility lookup
description: Git executable selection without repository bootstrap execution.
tags: [security, executables]
status: draft
sources:
  - id: selection
    resource: ../../src/selection.rs
    title: Utility resolver and PATH filtering
  - id: git
    resource: ../../src/git.rs
    title: Git command construction
  - id: tests
    resource: ../../tests/utility_resolution.rs
    title: Utility lookup regression tests
---

# Utility lookup

Git lookup skips empty and relative PATH entries. Executable
candidates must canonicalize to absolute paths outside registered repository
roots. The resolver inspects canonical candidate ancestry for `.git` entries
without executing candidate Git or following Git metadata pointers. Any marker,
uninspectable ancestor or ancestry beyond 256 directories rejects the candidate.
This excludes primary, sibling and unrelated Git working trees even before ahu
has opened them.[^selection][^tests]

Symlinked installations outside working trees remain supported; aliases into
working trees are excluded. Git uses the selected canonical path for each
invocation. Ahu does not resolve or invoke a cmux executable; session hosting
uses the selected external plugin.[^selection][^git][^tests]

Executable selection is not OS isolation and cannot prevent concurrent
replacement by a host process. The separate harness resolver excludes repository
roots ahu has opened; it does not perform this utility ancestry scan.[^selection]

The utility lookup section in docs/reference.md at the repository root details
usage constraints.

[^selection]: PATH filtering and filesystem ancestry inspection in src/selection.rs.
[^git]: Canonical utility invocation in src/git.rs.
[^tests]: Repository aliases and sibling worktrees in tests/utility_resolution.rs.

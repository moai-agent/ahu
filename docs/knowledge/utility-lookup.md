---
type: Architecture
title: Utility lookup
description: Git and default cmux executable selection without repository bootstrap execution.
tags: [security, executables]
status: draft
sources:
  - id: selection
    resource: ../../src/selection.rs
    title: Utility resolver and PATH filtering
  - id: git
    resource: ../../src/git.rs
    title: Git command construction
  - id: cmux
    resource: ../../src/cmux.rs
    title: Default and explicit cmux discovery
  - id: tests
    resource: ../../tests/utility_resolution.rs
    title: Utility lookup regression tests
---

# Utility lookup

Git and default cmux lookup skip empty and relative PATH entries. Executable
candidates must canonicalize to absolute paths outside registered repository
roots. The resolver inspects canonical candidate ancestry for `.git` entries
without executing candidate Git or following Git metadata pointers. Any marker,
uninspectable ancestor or ancestry beyond 256 directories rejects the candidate.
This excludes primary, sibling and unrelated Git working trees even before ahu
has opened them.[^selection][^tests]

Symlinked installations outside working trees remain supported; aliases into
working trees are excluded. Git uses the selected canonical path for each
invocation. A discovered default cmux client retains its canonical executable
across calls.[^selection][^git][^cmux][^tests]

`AHU_CMUX_BIN` deliberately retains ordinary command semantics, including bare
names and relative paths, without the default exclusion. The selected executable
is pinned to its canonical path before probing or execution. Executable selection
is not OS isolation and cannot prevent concurrent replacement by a host process.
The separate harness resolver excludes repository roots ahu has opened; it does
not perform this utility ancestry scan.[^selection][^cmux]

The utility lookup section in docs/reference.md at the repository root details
usage constraints.

[^selection]: PATH filtering and filesystem ancestry inspection in src/selection.rs.
[^git]: Canonical utility invocation in src/git.rs.
[^cmux]: Explicit override and default client discovery in src/cmux.rs.
[^tests]: Repository aliases, sibling worktrees and explicit overrides in tests/utility_resolution.rs.

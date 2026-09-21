---
type: Architecture
title: Task communication
description: Operator-only inbox delivery and the task artifacts ahu reads back for display.
tags: [worktrees, state, security]
status: draft
generated: { by: docs-astra/1.1.1, at: 2026-09-20T03:22:38Z }
sources:
  - id: commands
    resource: ../../src/commands.rs
    title: Inbox delivery, bounds, and artifact display
  - id: orchestration
    resource: ../../src/orchestration.rs
    title: Delegation contract for tasks
  - id: launch
    resource: ../../src/launch.rs
    title: Interactive worker environment
  - id: headless
    resource: ../../src/headless.rs
    title: Headless worker environment
  - id: tests
    resource: ../../tests/task_communication.rs
    title: Inbox and artifact regression tests
---

# Task communication

Communication between the operator and a running task travels through files
under the task directory, and delivery into them is the operator's exclusive
right. `ahu message <task-id> <text>` refuses to run inside a worker session,
whose inherited `AHU_WORKER_SESSION` marker marks it as a working agent, so a
task cannot message itself or another task; delivery belongs to the operator
or to a broker-bound child. An empty message text is refused as a usage error,
and the task id resolves through the same repository-scoped lookup every task command
uses.[^commands][^launch][^headless]

Arguments after the task reference are literal message payload; flag-like text
does not change repository, color, or output options. Typed `ahu:task:<id>`
references, exact `@name` task handles, bare IDs, and unique ID prefixes use the
same resolver. Handles resolve within the selected repository before delivery
ownership checks.[^commands]

The inbox is the `inbox` directory under the task directory. Entries are
numbered files, the next message written as the four-digit successor of the
highest present number. The inbox holds at most 100 entries and 1 MiB in
total; delivery stops with nothing written when either bound would be
exceeded. The scan before delivery fails closed: an entry ahu does not
recognize, or one that is not a regular file, stops the delivery rather than
writing next to it. Messages are written trimmed, without a trailing
newline, with owner-only permissions.[^commands]

The direction is one-way. The delegation contract instructs a task to read
its inbox entries and never rewrite, renumber, or delete them, and states
that a task id grants no delivery into any other task's directory. A task
never writes an inbox; ids locate work, they do not authorize it, as
[Task identity](task-identity.md) records.[^orchestration]

For interactive tasks and legacy headless contracts, two artifacts carry what a
task wants to say back. The task writes its final
report as `result.md` in its task directory; `ahu task` displays it under a
`result` heading, refusing rather than truncating a report larger than the
1 MiB display bound and naming one it cannot safely read. When the task
needs an operator answer first, it writes `question.md` in the same
directory, replaces that file when the question changes, and removes it
once answered; `ahu tasks` shows a `question` line for such a task, carrying
its first line capped at 60 characters, and reporting unreadable, oversize,
or empty states rather than their contents.[^orchestration][^commands]

New headless contracts keep final answers and helper output native. They do not
request a copied `result.md` in primary-owned coordination. Human `task` and
`result` output reports known session provenance, bounded outcomes, coordination
paths, and explicitly unknown native data locations. The operator question
file and inbox remain in the coordination directory. Report/capture paths apply
to legacy attempts only. Recorded outcomes and observed ownership remain
separate; unavailable metadata stays explicit. Inspection does not scan native
transcripts.[^commands][^headless]

Everything a task writes is untrusted data. ahu sanitizes artifacts for
display and never treats their contents as its own words; a report is read
as the task's claim about its work, and process success is never acceptance
of it. [Task state](task-state.md) explains the directory these files live
in.[^commands][^tests]

[^commands]: Message delivery, inbox bounds, and artifact display in src/commands.rs.
[^orchestration]: The delegation contract text in src/orchestration.rs.
[^launch]: The interactive worker environment in src/launch.rs.
[^headless]: The headless worker environment in src/headless.rs.
[^tests]: Inbox delivery and artifact cases in tests/task_communication.rs.

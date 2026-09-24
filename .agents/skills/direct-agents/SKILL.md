---
name: direct-agents
description: Coordinate repository work through registered ahu agents and task records.
---
# Direct agents

Use this skill when coordinating work inside an ahu task. Registered agents are
launched through ahu, with their configured harness, model, permissions, and
fresh worktree. Do not impersonate an agent with native harness delegation or
silently substitute another harness. The workflow is tracker- and terminal-
neutral: GitHub and cmux are supported integrations, not assumptions every
project or machine must satisfy.

## Assign work

Discover identities with `ahu agents`. Put the complete assignment in a UTF-8
file outside every repository, then launch it with:

```sh
ahu launch @agent-name --prompt-file /absolute/path/to/task.txt
```

Use `--headless --background --output json` for unattended work. Approval
widening requires the explicit `--allow-widened-approvals` flag. Use `--dry-run`
to inspect a launch before submitting it. Record each returned task id and
worktree, plus a terminal-surface reference when the selected backend provides
one.

## Inspect and accept

Use `ahu tasks`, `ahu task <id>`, `ahu wait <id>`, and `ahu result <id>` to track
assignments. A process exit is not proof of completion: read `result.md`, inspect
the diff and validation evidence, and review the actual commit before integrating.
For an interactive task, inspect its recorded terminal surface only when that
surface is available. CMUX uses `cmux read-screen --workspace <id> --scrollback`;
headless tasks have no workspace to read. Do not make a terminal transcript the
durable task record. Keep prompts, reports, and execution traces outside
repositories.

Children inherit the parent's current HEAD, not uncommitted source edits. For a
review of local changes, name the source checkout and exact revision or diff in
the assignment. Keep issue updates with the coordinator responsible for the
parent work, and do not push unless explicitly authorized.

## Long-running work and tracker records

For work that can run for a long time or fail after submission, create or link a
private durable tracker record before launching the task. An issue, task,
milestone item, or equivalent provider object may represent that record; the
provider mapping is project policy, not an ahu requirement. The tracker record
is the durable work ledger, while ahu's task state is execution evidence and may
be removed with task cleanup. Keep the record limited to acceptance criteria,
validation evidence, failure details, and disposition needed by the maintainer.

Use a provider adapter or MCP server for tracker operations. When the provider
is GitHub, issues, labels, projects, and milestones are one possible mapping;
do not bake repository names, issue-number formats, label names, project field
IDs, or milestone semantics into task prompts or public skill prose.

The tracker provider contract is intentionally small. It must be able to
resolve the private work area and visibility boundary, find or create a durable
work record, read and update lifecycle plus current-agent state, append
validation evidence, link parent/child or release context, and close the record
only after delivery checks. Provider identifiers stay in the private tracker;
ahu task IDs, worktrees, and attempts remain separate execution identifiers.
If a provider cannot implement one of these operations, report that capability
gap rather than approximating it with a public comment or a local file.

Use the provider's coordination fields, labels, tags, or status values instead
of personal assignees when the project is maintained by one maintainer. Preserve
one lifecycle state (planned, active, review, blocked, or done) and one current
agent identity, using the provider's native representation. Add a failure or
delivery marker only for an active condition, such as retry, missing evidence,
or uncommitted work. Remove stale agent and failure markers when the state
changes. The coordinator owns comments, state transitions, release/milestone
links, and closure after acceptance criteria and delivery checks are complete;
an agent does not close its record merely because its process exited.

Before any tracker write, verify the visibility and access boundary of the
record, its project, and linked objects. For GitHub, a private project does not
make a linked public repository or public issue private. Never copy private
record titles, bodies, labels, identifiers, tracker URLs, roadmap ordering, or
finding-to-fix mappings into the public repository, its knowledge base, task
prompts, result reports, or commits. If the provider cannot prove the boundary
or a write fails, record the tracking blocker in the private handoff and leave
the record open.

## Boundaries

ahu owns identity resolution, worktree creation, task records, permissions,
delivery fencing, and task lifecycle. A terminal adapter may display or focus
a task, but it does not become the source of task truth. This skill supplies the
operator workflow;
it does not grant authority or change those checks. Report unavailable agents,
denied launches, missing credentials, incomplete reports, and unjoined children
as blockers. Never replace a configured harness or model after failure.

Maintain this file at `.agents/skills/direct-agents/SKILL.md`; all supported
harnesses load the canonical `.agents/skills/` tree directly.

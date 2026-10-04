---
name: ahu-direct-agents
description: Coordinate repository work through registered ahu agents and task records.
---
# Direct agents

Use this skill when coordinating work inside an ahu task. Registered agents are
launched through ahu, with their configured harness, model, permissions, and
fresh worktree. Do not impersonate an agent with native harness delegation or
silently substitute another harness. The workflow is tracker- and terminal-
neutral. GitHub and cmux are supported integrations, not assumptions every
project or machine must satisfy.

## Assign work

Discover identities with `ahu agents`. Put the complete assignment in a UTF-8
file outside every repository, then launch it with:

```sh
ahu @agent-name --prompt-file /absolute/path/to/task.txt
```

Use `--headless --background --output json` for unattended work. Approval
widening requires the explicit `--allow-widened-approvals` flag. Use `--dry-run`
to inspect a launch before submitting it. Record each returned task id and
worktree, plus a terminal-surface reference when the selected execution mode provides
one.

Assign validation ownership as part of the task. Serialize heavyweight coverage
and timed suites across agents on a shared host; separate build directories do
not isolate CPU load. Use a distinct external `CARGO_TARGET_DIR` for each build
and require reports to name the tested commit, commands, results, and skips.

## Inspect and accept

Use `ahu tasks`, `ahu task <id>`, `ahu wait <id>`, and `ahu result <id>` to track
assignments. A process exit is not proof of completion: read the report provided
through the task's contract and repository policy, inspect the diff and validation
evidence, and review the actual commit before integrating. New headless tasks
keep final answers in native harness stores rather than a copied `result.md`.
For an interactive task, inspect its recorded terminal surface only when that
surface is available. cmux uses `cmux read-screen --workspace <id> --scrollback`;
headless tasks have no workspace to read. Do not make a terminal transcript the
durable task record. Keep prompts, reports, and execution traces outside
repositories.

Children inherit the parent's current HEAD, not uncommitted source edits. For a
review of local changes, name the source checkout and exact revision or diff in
the assignment. Keep issue updates with the coordinator responsible for the
parent work, and do not push unless explicitly authorized.

## Long-running work and tracker records

Follow this repository's work-tracking policy when one exists. A project may
require a tracker record for certain work, use records only for releases or
long-running assignments, or choose not to use an external tracker. Do not
assume GitHub, a private roadmap, or any tracker is available. When no provider
or tracking policy is configured, use the ahu task record for execution and
report that no external work item is linked; do not invent a tracker workflow.

When policy requires a durable work item, create or resolve it before launch.
After ahu returns a task ID, link that execution from the provider-side work
item using its native relation, reference field, or private activity record.
Keep ahu task IDs, handles, attempts, and branches as execution identifiers;
the provider-side work item remains the record for acceptance criteria,
validation evidence, failures, and disposition. Update the link as the task is
resumed or its commit/review is delivered, and close the work item only under
the repository's policy after acceptance and required delivery checks pass.
Use the provider's native representation; this does not require a particular
issue-to-task cardinality.

Do not put private tracker identifiers, URLs, titles, or contents into ahu
prompts, task records, result reports, public files, or commits. Tracker tools
and skills manage the link on the provider side.

Before cleanup, inventory the exact owned task IDs, worktrees, branches, and
recorded terminal workspace/group IDs. Verify terminal state, reviewed results,
and integration; preserve unrelated sessions and any task whose ownership or
remaining work is uncertain. `ahu cleanup` removes recognized headless captures
and bounded requests after termination is known; it retains the task record,
results, native sessions, branch, and worktree. Run `ahu remove` from another
checkout after the worktree is clean and its branch is merged into the primary
checkout's current HEAD (or already absent). A cherry-pick may not satisfy that
ancestry check. Interactive tasks have no headless captures, so use `ahu remove`
directly after these checks.

`ahu remove` also closes a recorded cmux workspace when its group/window
ownership can be verified; an unverifiable relationship blocks removal. Group
anchors and native harness stores are not generally removed. Separately inspect
any remaining fixture anchor against the inventory, its current members, and
session activity before closing it. Close only an owned, disposable fixture
anchor with no needed session or unrelated members. A title or apparently empty
group is not proof of ownership. Verify removal of owned task records,
worktrees, branches, and workspaces, and preservation of unrelated sessions;
report partial cleanup and retained anchors. Keep the provider-side work item
and its delivery evidence according to repository policy.

Use a provider adapter or MCP server for tracker operations. When the provider
is GitHub, issues, labels, projects, and milestones are one possible mapping;
do not bake repository names, issue-number formats, label names, project field
IDs, or milestone semantics into task prompts or public skill prose.

When tracker integration is part of repository policy, its provider contract is
intentionally small. It should resolve the work area and visibility boundary,
find or create a durable work record, read and update lifecycle plus current-
agent state, append validation evidence, link the ahu task and any parent,
child, or release context, and close the record only after delivery checks. A
project may implement this with a tracker MCP, a provider adapter, or another
approved skill/tool workflow. Provider identifiers stay in the provider; ahu
task IDs, worktrees, and attempts remain separate execution identifiers. If a
required provider operation is unavailable, report the capability gap rather
than approximating it with a public comment or local repository file.

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
as blockers. Native admission refusal is evidence of the refusal, not successful
execution. Preserve native controls and report the blocker without changing
accounts, hooks, plugins, or approval settings. Never replace a configured
harness or model after failure.

Maintain this file at `.agents/skills/ahu-direct-agents/SKILL.md`. Run `ahu setup`
to install the user-facing skills in the paths supported by detected harnesses.
Codex, OpenCode, and Antigravity use `.agents/skills/`; Claude Code uses
`.claude/skills/`. A skill file being present does not prove that a harness
loaded or used it. Explicit invocation and completion observations are separate
from evidence that the agent followed the skill. When the ahu MCP server is
connected, use `ahu_agents_list`, `ahu_tasks_list`, and `ahu_task_get` when they
fit the task. For an assignment verifying MCP access, call the actual tool and
record its result or exact failure; CLI output cannot establish MCP success.

Before delegating costly work across harnesses, call `ahu_auth_budget` when it
is available and use its active-profile rate-limit windows as one input alongside
agent compatibility, task load, and model suitability. Pass
`minimum_remaining_percent` when you need a conservative capacity filter; only
`eligible` candidates have every reported window above that threshold and
strictly above zero. This is a rate-limit filter, not a token-price comparison,
model availability check, or reservation. It reports percentages and reset
periods, not token counts or an allocation per agent. Agents on the same
provider account share that capacity. Treat unsupported, unknown, or unbound
providers as unavailable evidence, never as proof of spare capacity. Do not
switch auth profiles to route work.

For MCP and OTel compatibility assignments, report evidence separately for each
harness version, exact model, and execution mode. Require an actual MCP call
and result and observed telemetry export for a successful row, with task/attempt
and ahu revision. Keep missing telemetry, telemetry that is off, native admission refusal,
execution failure, and success distinct. Configuration or tool discovery proves
neither invocation nor export; interactive success does not validate headless
mode, and synthetic fixtures do not establish live provider compatibility.

For a task-bound operation that needs the operator to decide first, use the
`ahu_request_approval` MCP tool with a concise operation category, summary, and
optional target, then wait for its result. The operator resolves it with
`ahu approve TASK` or `ahu reject TASK`. This is an explicit cooperative
checkpoint; it does not intercept other shell or file operations.

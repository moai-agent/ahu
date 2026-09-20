---
name: direct-agents
description: Coordinate repository work through registered ahu agents and task records.
---
# Direct agents

Use this skill when coordinating work inside an ahu task. Registered agents are
launched through ahu, with their configured harness, model, permissions, and
fresh worktree. Do not impersonate an agent with native harness delegation or
silently substitute another harness.

## Assign work

Discover identities with `ahu agents`. Put the complete assignment in a UTF-8
file outside every repository, then launch it with:

```sh
ahu launch @agent-name --prompt-file /absolute/path/to/task.txt
```

Use `--headless --background --output json` for unattended work. Approval
widening requires the explicit `--allow-widened-approvals` flag. Use `--dry-run`
to inspect a launch before submitting it. Record each returned task id, worktree,
and cmux workspace.

## Inspect and accept

Use `ahu tasks`, `ahu task <id>`, `ahu wait <id>`, and `ahu result <id>` to track
assignments. A process exit is not proof of completion: read `result.md`, inspect
the diff and validation evidence, and review the actual commit before integrating.
For interactive tasks, use `cmux read-screen --workspace <id> --scrollback`.
Keep prompts, reports, and execution traces outside repositories.

Children inherit the parent's current HEAD, not uncommitted source edits. For a
review of local changes, name the source checkout and exact revision or diff in
the assignment. Keep issue updates with the coordinator responsible for the
parent work, and do not push unless explicitly authorized.

## Boundaries

ahu owns identity resolution, worktree creation, task records, permissions,
delivery fencing, and task lifecycle. This skill supplies the operator workflow;
it does not grant authority or change those checks. Report unavailable agents,
denied launches, missing credentials, incomplete reports, and unjoined children
as blockers. Never replace a configured harness or model after failure.

Maintain this file at `.agents/skills/direct-agents/SKILL.md`; all supported
harnesses load the canonical `.agents/skills/` tree directly.

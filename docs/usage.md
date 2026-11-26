# Task workflow and features

## Everyday use

ahu and its maintained executable plugins use Rust; Python is only a development
tooling dependency. See the [runtime inventory](rust-runtime.md) and
[Rust harness plugin setup](harness-plugins.md) for package installation and
the native behavior owned by each independently versioned plugin.

From any Git checkout:

```sh
ahu doctor
ahu agents
```

Install and activate the desired [harness plugins](harness-plugins.md), then
run `ahu setup` in each project. It uses installed capability metadata and asks you
to choose models for their ahu development agents, installs the bundled skills, adds
project MCP entries, and writes `.agents/ahu/config.md` and `.agents/ahu/lock.md`.
Review and commit the generated files so future ahu commands can use the same
project setup. ahu does not stage, commit, or push them.

Delegate work to a registered agent with a quoted prompt; tasks run headless:

```sh
ahu @dev-astra 'Review configuration validation.' --work-mode
ahu agents
```

For a scriptable preview in this repository:

```sh
ahu agents
ahu @dev-astra 'Review configuration validation.' \
  --dry-run --allow-widened-approvals
```

To submit an assignment, write it to a UTF-8 file:

```sh
printf '%s\n' 'Review configuration validation and report findings.' > assignment.txt
ahu @dev-astra --prompt-file assignment.txt \
  --title "Configuration review" --summary "Check validation and report findings" \
  --work-mode
```

The direct form accepts `--prompt` and `--prompt-file`.
Each task receives a short handle derived from its displayed title, such as
`@configuration-review`. Use `--name review` to choose one. Task commands accept
the handle: `ahu task @review` or `ahu cancel @review`.
Names belong to the repository and remain reserved after removal; task IDs still work.
Most registered agents declare `permissions = "auto"` (see each manifest's
`permissions` field), so their launches require the explicit approval-widening
flag, even for previews. The flag controls the child's requested settings; it
grants no extra access to the caller.

For unattended execution, use an initialized project and the same registered agent:

```sh
ahu @dev-astra --headless --prompt-file assignment.txt \
  --allow-widened-approvals --output json
ahu @dev-astra --headless --background --prompt-file assignment.txt \
  --allow-widened-approvals --output json
```

The first command waits in the foreground; the second returns after supervisor
startup. Each launches a separate task. Headless coordination lives in the primary checkout’s private state; native
histories stay in the harness’s own stores. See [headless execution](reference.md#headless-execution) for
supported CLI versions, result collection, resume, cancellation, and limits.
Headless mode covers Claude Code, Codex, Antigravity, OpenCode, and Kiro CLI.
An agent on a harness without a headless adapter is refused rather than run on
another harness. `--work-mode` requests each harness's automatic/no-ask mode and
continues when ahu cannot verify native configuration compatibility, recording
those differences in the task result. It does not make a Git worktree a sandbox
or guarantee that harness-native hooks, plugins, or authentication will not
block execution.
Headless child launches need host grants (`--allow-child` or
`--allow-child-widened`). Claude Code 2.1.270 also supports bounded native helpers
for entirely read-only assignments; use a separate registered reviewer for that
mode when the coordinator needs to edit or run commands.

Inspect work from the primary checkout or a sibling task worktree. Outside it,
prefix the command with `ahu --repo /path/to/project`. Task commands accept
exact `@name` handles, `ahu:task:<id>`, bare IDs, and unique ID prefixes:

```sh
ahu tasks
task_id=abc123  # replace with a full task ID from ahu tasks
ahu task "$task_id"
ahu result "$task_id"
```

Reviewing the changes is plain Git. `ahu task` prints the task's branch,
worktree, and launch base; run Git against those directly:

```sh
base_commit=COMMIT  # replace with the launch base printed by ahu task
git -C .ahu/worktrees/"$task_id" diff "$base_commit"
git -C .ahu/worktrees/"$task_id" status
```

Worktrees make task changes reviewable; Git does not sandbox an agent's
filesystem or processes. Follow the shared [agent Git workflow](agent-git-workflow.md)
for worktree boundaries, coordinator integration, release branches, and the
limits of harness compatibility evidence.

Headless inspection shows the attempt outcome, observed ownership, blockers,
known native session reference, and artifact paths. A process exit does not
prove completion, so `task` and `result` report the root outcome and helper
lifecycle coverage on separate lines: unknown helper coverage does not mean the
root result is missing. A `supervisor_error` attempt is never resumed
automatically; it prints the recorded failure phase and the recovery steps to
follow. Review the changes and findings. Tasks keep their branches,
worktrees, and records after the harness exits.

## Typed decisions and agent evals

Give agents a shared decision tool, then measure whether it helps their work.
[TypeSafe Jev through ahu MCP](typed-decisions.md) returns validated choices,
scores, and probability estimates. A batch can apply one rubric to up to 20 items
without asking the agent to repeat the schema for each item.

[ahu Evals](evaluations.md) compares registered agents using Markdown cases,
deterministic checks, optional agent judges, and OTel tool observations. Reports
show answer quality, failures, time, native token usage, and separate decision
service usage. Use that evidence to improve instructions and skills; delegation
is a tradeoff to measure, not a promise of savings. Read the
[live comparison findings](decision-eval-findings.md) for the measured
improvements and limits.

## Registered agents

Run `ahu agents` to see this checkout's registered agents, their pinned harness
and model, source manifest, and any detected configuration drift. The list is
loaded from the repository each time, so this README does not duplicate a
potentially stale agent catalog. [Manifests](../.agents/ahu/agents/) are OKF
Markdown documents that pair each agent's identity with its instructions.
Named launches use their manifest's pair independently of automatic project
selection. This repository's automatic choice is Antigravity /
`gemini-3.1-pro-high` (from `harness_preferences` and `model_rankings` in
`.agents/ahu/config.md`); it does not select a named agent.

Automatic selection walks the configured harness order, takes the first harness
with an available adapter and a nonempty project model ranking, and selects that
ranking's first model. Local prerequisites are checked afterward. A missing
harness produces an error, never a different selection or catalog fallback.

## Requirements discovery

Use [discover-requirements](../.agents/skills/discover-requirements/SKILL.md) when a
request needs clearer outcomes, scope, or acceptance criteria. Ask to use
`discover-requirements` with your idea; it gathers facts and proposes choices in
your preferred channel, keeping discovery records private. Specified tasks proceed
without a mandatory interview. The canonical repository skill tree is
`.agents/skills/`, which OpenCode and Codex discover natively; see
[skill verification](skill-verification.md) for per-harness evidence.

## What travels with a task

Each task starts at the invoking checkout's HEAD on `ahu/<agent>/<task-id>`, in
`.ahu/worktrees/` under the primary checkout. Recognized agent configuration is copied
from the invoking checkout, including uncommitted and ignored files and local
deletions. Unrelated dirty source files stay behind. Task records and
prompts and headless coordination belong to the primary checkout’s ignored
`.ahu/state/`. Task-local state belongs to the task checkout’s `.ahu/`.
The repository task index survives worktree deletion until task removal. ahu does not
copy native event streams, stderr, final answers, or helper summaries there. See
[state and compatibility](reference.md#state-and-compatibility) for discovery,
legacy records, overrides, and integrity limits.

ahu delivers its contract, available execution facts, agent instructions, and
assignment in nonce-bearing sections with raw bodies. This is prompt text,
not an enforced system prompt. The harness controls the running session,
including model changes, approvals, and context loading. ahu reports visible
settings and gaps without claiming to know everything the harness loads.
Ordinary launches do not alter native hooks or settings.
Private tracker material stays out of public files and reports; remote pushes
require explicit user permission.

## More commands and reference

| Command | Purpose |
| --- | --- |
| `ahu setup` | Detect harnesses, choose models, configure MCP, install skills, register per-harness development agents, and refresh `.agents/ahu/lock.md` |
| `ahu @agent [prompt]` | Launch a registered agent directly, or open the launcher with it selected when no prompt is supplied |
| `ahu doctor` | Concise readiness summary; add `--verbose` for component diagnostics |
| `ahu auth readiness --harness codex --output json` | Check native identity and local project binding without a model request |
| `ahu auth budget --output json` / MCP `ahu_auth_budget` | View the active profile's verified provider rate-limit windows; unsupported budgets remain unknown |
| `ahu approve TASK` / `ahu reject TASK` | Resolve a task's explicit MCP approval checkpoint after inspecting it with `ahu task TASK` |
| `ahu lock` | Check that recognized agent context and `.agents/ahu/lock.md` are committed and current |
| `ahu lock --update` | Fingerprint current recognized context into `.agents/ahu/lock.md` for review and commit |
| `ahu agents` | List registered agents and detected drift |
| `ahu tasks` | List the 20 most recent tasks; use `--limit N` or `--all` to change the range |
| `ahu help [COMMAND]` | Compact command list or focused help; use `ahu help all` for the full option reference |
| `ahu onboard` | Read-only preview of native definitions available for registration |
| `ahu knowledge lint` | Validate and lint configured OKF bundles with installed `okf` |
| `ahu explain` | Built-in architecture overview |
| `ahu agy` | Open a coordinating Antigravity CLI session in the current terminal |
| `ahu claude` | Open a coordinating Claude session in the current terminal |
| `ahu codex` | Open a coordinating Codex session in the current terminal |
| `ahu opencode` | Open a coordinating OpenCode session in the current terminal |
| `ahu kiro` | Open a coordinating Kiro V3 session in the current terminal |
| `ahu mcp serve` | Serve repository-scoped agent, task, and typed-decision tools over stdio MCP |
| `ahu eval run --suite PATH --agent @name [--records PATH]` | Run cases and save bounded records in primary ignored `.ahu/evals/` by default; raw artifacts stay under repository-local `.ahu/` |
| `ahu eval report [--records PATH]` | Compare answer quality, tool behavior, failures, time, and observed usage |

Use the bundled `ahu-agent-context-critic` skill with `ahu eval run` and
`ahu eval report` to investigate context changes against measured outcomes.
The skill treats repository files as declared context, not proof that a
harness loaded them. Shared project inputs are covered by committed `.agents/ahu/lock.md`;
recognized private local settings are accepted in owner-only per-user ahu state.
Other harness, managed, and provider context remains outside that coverage.
The eval cases, OTel observations, and private run artifacts should stay outside
the candidate checkout when they contain information the candidate should not
see. `ahu help` lists commands, `ahu help COMMAND` shows focused options, and
`ahu help all` prints the full reference.

Coordinator shortcuts are aliases supplied by installed plugins. `ahu harness ID`
selects an exact installed descriptor. They use the project's top-ranked exact
model and request automatic approval through the plugin's native mapping. Missing
rankings, unsupported permission modes and missing plugins are errors; ahu does
not silently choose a different model or weaken the requested approval mode.

Codex, Claude Code, Antigravity and Kiro packages provide direct automatic modes.
The reviewed OpenCode mini version has no unattended interactive override, so its
automatic direct shortcut is refused; its headless work mode supports automatic
approval. Native configuration can still deny tools, and automatic mode is not
filesystem confinement. See [harness plugins](harness-plugins.md) for evidence
and version boundaries.

Aliases accept no extra native flags. Coordinator sessions run in the current terminal. When the `cmux` CLI is installed,
the Claude Code and Codex shortcuts start through cmux's own wrappers, and the
OpenCode, Kiro and Antigravity shortcuts first run cmux's hook installer; see
[the cmux notes](reference.md#cmux).
See [coordinator sessions](reference.md#coordinator-sessions) for details.

The MCP integration is local and repository-scoped. `ahu mcp serve` provides
agent and task inspection over stdio, and can expose the optional
`ahu_typed_decide` tool for TypeSafe Jev over HTTPS or a configured local
decision service. `ahu setup`
configures a project MCP entry for each detected harness, installs bundled
skills in harness-supported paths, creates a `dev-<harness>` ahu agent using a
model you choose, and refreshes `.agents/ahu/lock.md`. Review and commit the generated
project files before launching. Claude Code asks before using project MCP
servers, and Codex must trust the repository before loading project configuration. See the
[typed-decision experiment](typed-decisions.md) for its model-neutral
service contract and limitations.

External issue tracking is optional and remains policy in skills and tracker
tools, not in ahu's task CLI. The bundled `ahu-direct-agents` skill lets a project
choose whether to use tracker records and, when configured, link those records
to ahu task IDs on the provider side.
The shared [task service](agent-task-service.md#optional-tracker-foundation-for-scoped-task-listing)
also provides scoped, paginated task reads as an in-process foundation for future
optional adapters. It does not activate a tracker or grant external access.

See the [CLI and context reference](reference.md) for registration, JSON
contracts, prompt transport, hooks, state, and delegation boundaries.


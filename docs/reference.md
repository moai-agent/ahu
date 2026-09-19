# CLI and context reference

[Back to README](../README.md). Run `ahu help` for option syntax and
`ahu explain` for the built-in architecture overview.

## Scriptable launch previews

A launch prompt can come from an inline argument, a UTF-8 file, or piped stdin:

```sh
ahu launch @offsec-astra --prompt 'Review the subprocess argument handling.' --dry-run --allow-widened-approvals
printf '%s\n' 'Review the subprocess argument handling.' > assignment.txt
ahu launch @offsec-astra --prompt-file assignment.txt --dry-run --allow-widened-approvals
printf '%s\n' 'Review the subprocess argument handling.' | ahu launch @offsec-astra --dry-run --allow-widened-approvals
ahu launch @offsec-astra --prompt 'Review the subprocess argument handling.' --dry-run --allow-widened-approvals --output json
```

`--prompt` and `--prompt-file` are mutually exclusive. An explicit source takes
precedence over stdin, which is read only when neither flag is given and stdin
is not a terminal. Empty or whitespace-only prompts are rejected. Accepted
prompt text retains its original bytes, including leading and trailing newlines.

`--dry-run --output json` writes one JSON object to stdout and human-readable
preview information to stderr. Previewing requires Git, valid project settings,
and the selected harness, but no cmux session. It launches no task. Actual
interactive execution requires cmux; `--headless` uses a separate supervisor.
Manifests that widen approvals require the
explicit `--allow-widened-approvals` flag for both previews and execution, as in
the preceding examples. Interactive launch JSON requires `--dry-run`; headless
launches also support JSON execution results.

The interactive preview has `schema_version: 1` and these fields:

| Field | Meaning |
| --- | --- |
| `agent` | Name, version, description, resolved source path, and full identity digest. |
| `title`, `summary` | Plain sidebar display text, explicit or derived from the assignment. |
| `harness`, `model`, `selection_basis` | The exact selected pair and why it was selected. |
| `policy_digest`, `catalog_version` | The configuration and compatibility catalog used. |
| `permissions` | The manifest's approval mode. |
| `argv` | Command and arguments, with the delivered prompt redacted. |
| `prompt_digest`, `prompt_bytes` | SHA-256 and UTF-8 byte length of the original task prompt. |
| `enforcement` | Model enforcement status, gaps, and applied controls. |
| `warnings` | Hook, approval, wrapper, and other launch disclosures; ordinary in-session model switching is not a reliability warning. |
| `executed` | Always `false` for a preview. |

JSON strings preserve the actual metadata through JSON escaping; terminal
display escaping is not applied to them. Consumers should check the schema
version and tolerate additional fields. Removing a field or changing its
meaning requires a schema version bump.

Commands return these exit codes:

| Code | Meaning |
| --- | --- |
| `0` | Success. |
| `1` | Cancelled by the user. |
| `2` | Invalid command usage. |
| `3` | Unknown or unregistered agent. |
| `4` | Missing prerequisite, such as Git repository, harness, or cmux. |
| `5` | Run failure, including invalid persisted configuration. |

## Headless execution

`launch --headless` runs the selected harness in batch mode without cmux, a PTY,
or screen scraping. `--background` detaches the supervisor after startup
acknowledgement; without it, the caller supervises in the foreground. Use the
foreground form under CI or a host service that manages process lifetime.
A dry run performs preflight checks and prints the redacted command, frozen
identity, capabilities, gaps, timeout, and external runtime path without launching.
Known cmux wrappers are refused; use the actual harness executable on `PATH`.

The admitted CLI profiles are Codex 0.154.0, Claude Code 2.1.269/2.1.270,
Antigravity CLI 1.2.2, and OpenCode 1.18.29/1.18.31. Other versions fail before
worktree creation, with no fallback harness or model. OpenCode's batch form is
`opencode run --format json`; its permission mapping is the interactive one, so
a manifest declaring `permissions = "accept-edits"` is refused here too. See
[OpenCode with Ollama-hosted models](#opencode-with-ollama-hosted-models) for the
provider setup an OpenCode agent needs. Profile admission describes the adapter's argument
surface, not successful authentication, provider availability, or full native
helper lifecycle validation.

```sh
ahu launch @dev-astra --headless --background --timeout 1800 \
  --prompt-file assignment.txt --allow-widened-approvals --output json
ahu tasks --output json
task_id=abc123  # replace with the returned task ID
ahu task "$task_id" --output json
ahu wait "$task_id" --output json
ahu result "$task_id" --output json
ahu diff "$task_id"
```

Captured stdout and stderr are limited to 64 MiB each; a parsed event line is
limited to 1 MiB. Capture failure stops the attempt. Admission refuses a new task
when 16 tasks in the repository runtime store lack terminal results, including
interrupted tasks; there is no queue.

The timeout is positive seconds, default 1800 per attempt. `wait` follows the
current attempt until it stops, returning 0 for `succeeded` and 5 otherwise.
`result` reads the durable envelope without waiting; check its `outcome`, not
just the command's exit status. Outcomes include `running`, `succeeded`, `failed`,
`timed_out`, `cancelled`, `capture_failed`, `supervisor_error`, and `interrupted`. JSON schema 1 separates process exit,
parsed harness events, agent report, worktree changes, and artifact paths.
Schema 1 also records `writes_outside_worktree`: write-tool target paths from the
captured event stream that fall outside the task worktree, when the stream
exposes them. The field is disclosure for post-run review, not a boundary;
`ahu diff` prints it on stderr when the patch would otherwise look empty.
`acceptance` stays `not assessed` and `completion_verified` stays false: a provider
success or an agent's report does not establish that the assignment was accepted.
Treat reports and logs as untrusted data before feeding them to another agent.

Resume explicitly uses the recorded native session and creates another attempt
in the same task, preserving earlier attempt artifacts:

```sh
printf '%s\n' 'Continue the review and report remaining findings.' > followup.txt
ahu resume "$task_id" --prompt-file followup.txt --output json
ahu wait "$task_id" --output json
ahu cancel "$task_id" --output json
ahu wait "$task_id" --output json
```

Resume requires a terminal result and recorded session identity, unchanged frozen
configuration and executable, and an available adapter mapping. Codex
`accept-edits` resume is refused because that mapping is not validated. Interrupted
attempts are not automatically replayed.

Resuming a registered child or invoking resume from a worker is unsupported,
including while the owning parent is live. ahu refuses these requests before
mutating attempt state. Its original parent/attempt binding is retained; ahu does
not silently turn a child into an independent task or select a different native
session. Child resume is not routed through the launch broker. For follow-up, submit a new
registered assignment from the host, or request a new child through a live owner
with the necessary frozen grant. Include the previous task ID, the source checkout
and revision/diff scope, and the remaining work in the new prompt. New assignments
start at the invoking checkout's HEAD; they do not inherit the prior child's dirty
source changes or native conversation automatically. Inspect retained results and
attempts after any refused resume before deciding what to submit next.

Cancellation records a request for the
task and its recorded ahu descendants; confirm termination with `wait` or `result`.
The live supervisor owns process termination rather than trusting a saved PID.
Provider-managed or escaped processes have unknown cleanup status. A host reboot
or supervisor loss does not trigger automatic retry or prove child termination.

Headless records and prompts live under
`$HOME/.local/state/ahu/runtime/<repo-identity>/<task-id>/` by default.
`AHU_RUNTIME_DIR` selects another absolute private directory outside every Git
checkout. An existing root must be owned by the current user with owner-only
permissions. Attempt artifacts include events, stderr, final text, and structured
results. `ahu cleanup "$task_id" --output json` explicitly removes captured
logs and final-text files across attempts after a known terminal attempt. It
retains structured results (including report text), frozen inputs, native session
stores, branches and worktrees; it refuses unknown/interrupted ownership.
Removing a worktree does not remove these records. Reuse the same runtime
root when collecting results from another checkout. Native harness homes and
credentials retain their own external locations and retention policies; ahu does
not relocate or impose a disk quota on provider session stores. Do not place
execution traces in a repository, even ignored directories. Runtime records and
their digests remain editable by the same user: they are integrity checks, not
authenticated evidence or an OS security boundary.

Unattended approval mappings never grant extra authority merely to avoid a prompt.
Claude denies unanswered permission requests; Codex uses explicit batch sandbox
and approval flags; Antigravity preserves the manifest mapping (`auto` requests
`--dangerously-skip-permissions`). `auto` and
`accept-edits` still require `--allow-widened-approvals`, including dry runs.
Inspect the exact preview and denial evidence; exit zero alone is insufficient.

### Registered children and host grants

A headless worker can request only registered children granted when its root task
was submitted from the host. Repeat `--allow-child @name` for ordinary approval
profiles or `--allow-child-widened @name` for manifests declaring `auto` or
`accept-edits`. Granting the parent's own `--allow-widened-approvals` does not
also grant children. The grant freezes each child's identity, permission mode,
hook inventory, and native-helper policy; descendants cannot expand it.

For example, after registering a shell-capable `@coordinator` and a `@reviewer`
with ordinary approval settings:

```sh
ahu launch @coordinator --headless --background --allow-child @reviewer \
  --prompt-file assignment.txt --output json
```

If either manifest requests wider approvals, supply the corresponding parent
`--allow-widened-approvals` or child `--allow-child-widened @reviewer` flag.
Inside the owning task, request the child through the launching executable:

```sh
"$AHU_BIN" launch @reviewer --headless --background \
  --prompt 'Read the assigned source and report findings.' --output json
```

The child request still needs `--allow-widened-approvals` when its manifest
requires it. ahu sends a bounded request to the owning supervisor's broker; the
supervisor starts the real registered child outside the worker sandbox with
that child's configured harness, model, sandbox, and approval mapping. It does
not run a replacement harness inside the parent sandbox. Codex workspace-write
receives the task's request directory as a narrow additional write root;
read-only Codex broker transport is refused.

Requests bind to a live parent attempt. Invalid, replayed, stale, or cancelled
requests are refused; configuration changes also refuse dispatch. Request and
dispatch capture are limited to 2 MiB, dispatch to 30 seconds. A lost
acknowledgement does not justify replay: inspect recorded child tasks first.
Admission closes when the parent process exits. Failed or unjoined registered
children from its current attempt prevent parent success. The result's
`ahu_children` records their task IDs and outcomes. Limits are eight child levels,
128 assignments per root grant, and 16 active/interrupted assignments per
repository runtime store. No global token cap or hidden task queue is promised.
The broker is not isolation against hostile code running as the same OS user.

### Bounded native helpers

Native helpers belong to the owning harness attempt; they cannot replace a
registered specialist, another harness, or independently supervised work.
`--native-helpers` overrides the agent manifest's top-level `native_helpers`,
then project `[execution].native_helpers`; the default is `disabled`. Child
requests must retain the policy frozen in their host grant.

| Harness CLI | Headless launch | Bounded native helpers |
| --- | --- | --- |
| Claude Code 2.1.269 | Admitted | Refused |
| Claude Code 2.1.270 | Admitted | Read-only profile |
| Codex 0.154.0 | Admitted | Refused: incomplete helper identity/join event visibility |
| Antigravity CLI 1.2.2 | Admitted | Refused: unvalidated native profile |
| OpenCode 1.18.29/1.18.31 | Admitted | Refused: no validated native tool switch |

The Claude bounded profile restricts the **entire attempt, including the owner**,
to the model tools `Read`, `Grep`, `Glob`, and the parent's `Task` delegation tool.
Through those tools it cannot edit,
run shell commands, builds or tests, create native worktrees/teams, or shell-launch
registered ahu children. Use it for read-only review or investigation:

```sh
ahu launch @reviewer --headless --native-helpers bounded \
  --prompt 'Read the relevant source, use and await a helper, and report findings.' \
  --output json
```

This example requires a registered Claude-backed reviewer and CLI 2.1.270.
Apply the usual approval-widening flag if its manifest requires it. For a mixed
workflow, register that reviewer's manifest with `native_helpers = "bounded"`
and grant it to a shell-capable coordinator at host submission. The coordinator
launches the separate registered review task and collects its result; the reviewer
uses native helpers internally. Keep writing assignments in `disabled` mode.
ahu does not infer from prompt text whether an assignment requires writes.

The integrated bounded profile pins helpers to the owner's exact manifest model,
one concurrent helper, depth one, and a USD 5 budget per attempt. MCP tools and
slash commands are excluded. Repository settings and deny rules remain
discoverable; hook execution as a helper constraint is not established. The
`ahu-reader` role is requested and actual roles are recorded, but no role
allowlist or total helper-count cap is enforced. A role change cannot widen the
profile's tool ceiling or model pin. The ceiling does not establish that
settings-defined hooks cannot spawn processes or write files; review those
settings separately before relying on a read-only execution environment.

`native_completeness` records joins, unjoined helpers, violations, and unknowns.
A parent's final message is insufficient: bounded attempts require known,
successful helper completion. Provider-side cancellation and child usage
accounting remain unknown. The `disabled` mode withholds Claude's `Task` tool and
sets Codex `agents.enabled=false`; Antigravity has no validated native
control for turning them off. ahu's headless path does not invoke cmux, but
arbitrary hooks, native configuration and shell commands still require a vetted
environment. Neither
these controls nor local records establish a universal sandbox or independently
verified assignment acceptance.

## Knowledge checks

`ahu knowledge lint` runs installed `okf validate` followed by `okf lint` for
each configured bundle, combining and deduplicating their findings. It requires
Git, valid project configuration, and `okf` on PATH outside the repository;
it does not need cmux or a provider session. The check reads bundles without
fetching content, generating indexes, or rewriting files.

Add the optional section to `.agents/ahu/config.toml`, preserving existing policy:

```toml
[knowledge]
bundles = ["docs/knowledge"]
fail_on_warnings = false
```

Paths are repository-relative directories. Absolute paths, empty segments,
`.` and `..` segments, backslashes, and control or direction-changing characters
are rejected. Bundle paths and contents must not be symlinks; contents must be
regular files or directories. Omitting the section defaults to no bundles and
`fail_on_warnings = false`.

```sh
ahu knowledge lint
ahu knowledge lint --output json
```

Errors fail the check. Warnings fail it only when `fail_on_warnings` is true.
A passing check exits `0`; missing prerequisites, including no configured bundles
or missing `okf`, exit `4`. Invalid configuration, unusable bundle paths, validator
failures, and findings that fail the configured policy exit `5`. Each configured
bundle must contain at least one concept Markdown file: an empty directory, a
directory containing only non-Markdown files, or one containing only `index.md`
and/or `log.md` fails with exit `5`.

With `--output json`, stdout contains the completed report, and stderr carries
human-readable diagnostics. Its `schema_version` is `1`; fields include `command`,
`okf`, `fail_on_warnings`, total `errors` and `warnings`, `passed`, and `bundles`.
Each bundle has its configured `path`, counts, and `findings`; findings carry
`severity`, `rule`, `concept_id`, and `message`. A failure before report completion
can produce diagnostics without a JSON report.

The validator integration uses the OKF 0.5 JSON report contract; the standalone
bundle checks have been validated with `okf` 0.5.0. To check the bundle directly:

```sh
okf validate docs/knowledge
okf lint docs/knowledge
```

The maintained bundle is [docs/knowledge](knowledge/index.md). README and this
reference remain outside it. Format checks do not establish that source claims
are correct or that a human has reviewed them.

The installed validator executes with the caller's privileges; ahu does not
sandbox it. Tree inspection refuses symlinks and special files before invoking
OKF, but cannot prevent concurrent changes after inspection. Reports are collected
before the 16 MiB parsing limit is checked, so this is not a subprocess output
or memory limit. See [validator boundaries](knowledge/knowledge-validation.md)
for source provenance.

## Sidebar text

`--title` and `--summary` set plain display metadata without changing the
assignment. Titles are limited to 60 characters and summaries to 160; common
Markdown formatting is converted to plain text. With `--title` alone, the summary
uses the title too. Without either flag, text is derived from the assignment.
The identity pill shows agent and model. cmux controls native notification
previews and directory/branch rows through its own preferences.

## Terminal styling

Use `--color=auto`, `--color=always`, or `--color=never` with any command.
`always` and `never` override environment detection. Under `auto` (the default),
styling is turned off when `NO_COLOR` is set to any value, `TERM=dumb`, or stdout
is not a terminal. Redirected output is therefore plain by default. JSON stdout
never contains styling, even with `--color=always`.

Agent identity, harness/model, warnings, enforcement gaps, drift, and hints have
distinct styles. Labels and layout retain their meaning with color turned off.
Repository-controlled strings are escaped before styling so they cannot inject
terminal controls. The composer remains line-oriented: color does not change
the `.` sentinel, `.cancel`, or the confirmation code required for submission.

## Registering an agent

`ahu` launches an agent only when `.agents/ahu/agents/<name>.md` registers it.
A manifest is an OKF Markdown document whose YAML frontmatter declares
`okf_version: 0.2` and `type: ahu:agent`, followed by a body. The body is the agent's
instructions when no `source_format`/`source_path` pair selects a native
definition, and a short pointer for human readers when one does. Definitions
found elsewhere are onboarding candidates, never implicit
registrations—a skill is not an agent, and `AGENTS.md` is not an agent
registry.

`onboard` offers registration for Claude Code and OpenCode Markdown
definitions—`.claude/agents/<name>.md` and `.opencode/agent/<name>.md`, each
one file per agent with YAML frontmatter. A declared `model` is checked against
the catalog rows for that definition's own harness, so an OpenCode agent naming a Claude
model is a blocker rather than a re-targeted launch. It lists
Codex TOML and Antigravity native definitions with blockers even though both
harness adapters exist; this native-onboarding path cannot register them.
Explicit manifests can use plain Markdown with any supported harness, OpenCode
included; `antigravity-agent` also reads a Markdown body after frontmatter. An explicit
`codex-agent` source is delivered verbatim as text; its TOML fields are not parsed
as native model or instruction metadata. Plain Markdown makes the delivered
instructions explicit.

For the registration example below, first create the matching definition:

```sh
mkdir -p .claude/agents
printf '%s\n' 'Help implement repository tasks.' > .claude/agents/chris.md
```

A manifest references that definition in place:

```markdown
---
okf_version: 0.2
type: ahu:agent
title: chris
description: "Helps implement repository tasks"
status: stable
tags: [agents]
harness: claude-code
model: claude-opus-5
permissions: prompt
version: 1.2.0
source_format: claude-agent
source_path: .claude/agents/chris.md

---

Instructions live in the native definition at `.claude/agents/chris.md`,
referenced in place and never edited.
```

Use the CLI to preview, register, or remove this example:

```sh
ahu onboard
ahu onboard --register chris --model claude-opus-5 --agent-version 1.2.0
ahu onboard --remove chris
```

Registration and removal ask for confirmation; neither edits the native file.

`harness` and `model` are required, explicit values. If the native definition
has parsed frontmatter declaring a nonempty model other than `inherit`,
the two must agree; `ahu` does not rewrite either file or
pick one silently. A model identifier is written exactly as its harness expects
it: OpenCode's `-m` takes `provider/model`, so an OpenCode manifest pins the
provider prefix too, as in
[OpenCode with Ollama-hosted models](#opencode-with-ollama-hosted-models).

Every named agent needs a semantic version. `ahu` does not manage releases for
you, but it reports when a version label has stopped matching its inputs:
if `chris@1.2.0` launches with different instructions or a different repository
configuration than the last `chris@1.2.0` launch, that drift is reported as a
pending behavior change for the next version bump.

## OpenCode with Ollama-hosted models

This section walks one combination end to end: the OpenCode CLI driving GLM
through a local Ollama endpoint. ahu installs none of it, writes no OpenCode
configuration, and holds no provider credentials.

### Inference runs on Ollama's hosted service

`glm-5.3:cloud` is an Ollama **cloud** model. Registering an agent on it selects
a remote inference provider: the prompts, repository file contents, and tool
output that reaches the model leave the machine and are processed by Ollama's hosted
service. The Ollama process on the machine is the endpoint and router; it is not
where inference happens. Decide whether this repository's contents may leave the
machine before registering the agent, not after.

Check it yourself. `ollama list` shows a cloud model with an empty size column,
because there is no local weight file—on 2026-09-13 `glm-5.3:cloud` listed
digest `8477dab3e25b` and no size. A model whose weights are on disk shows a size.

A `:cloud` tag is also a moving alias: the digest behind the name can change
without the name changing. ahu records `ollama/glm-5.3:cloud` as a moving alias
rather than a pinned build, so a task record names the identifier that was
requested, not the build that answered.

### Check the prerequisites

```sh
opencode --version
ollama --version
ollama list
ollama show glm-5.3:cloud
curl -sS http://localhost:11434/v1/models
```

`ollama show` prints the model's `context length`. The `curl` request lists what
the OpenAI-compatible endpoint serves without running inference; a connection
error there means Ollama is not running or is not listening on that address.

More than one OpenCode installation can sit on `PATH` at once—a package-manager
copy and the installer's copy under your home directory, often at different
versions. ahu resolves `opencode` on the submitting shell's `PATH` and reports the
resolved executable path in the launch preview. To see which one that is:

```sh
command -v opencode
opencode --version
```

`type -a opencode` (bash or zsh) lists every match in `PATH` order. Compare the
first match against the path in the preview: ahu runs the installation its own
resolution picks and reports it, and it does not go looking for a different one.

### Configure the Ollama provider in OpenCode

OpenCode ships no Ollama provider. Until one is configured, `opencode models`
lists only `opencode/*` entries and `ollama/glm-5.3:cloud` does not resolve. The
provider is your own OpenCode configuration, never something ahu writes:

```json
{
  "$schema": "https://opencode.ai/config.json",
  "provider": {
    "ollama": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "Ollama",
      "options": { "baseURL": "http://localhost:11434/v1" },
      "models": {
        "glm-5.3:cloud": {
          "name": "GLM 5.3 (Ollama cloud)",
          "limit": { "context": 65536, "output": 8192 }
        }
      }
    }
  }
}
```

Put that in the project's `opencode.json` or in the global OpenCode
configuration; `ollama/glm-5.3:cloud` then appears in `opencode models`. This
repository carries exactly that block in its own committed `opencode.json`, so a
checkout of ahu needs only a running Ollama with the tag pulled. A global
configuration is the better home when the endpoint or account is personal to one
machine, since a committed one applies to everyone who clones the repository.
OpenCode's `-m` takes `provider/model`, so the `ollama/` prefix is part of the
identifier a manifest pins.

OpenCode 1.18.30 validates that block on startup and refuses a model entry
missing either half of `limit`: with `limit.context` alone, `opencode models`
printed `Missing key provider.ollama.models.glm-5.3:cloud.limit.output` and
listed no `ollama/*` model at all. Ollama's OpenCode integration documentation
requires a context length of 64k or higher; `ollama show <model>` reports the
model's own context length, and `limit.context` is what OpenCode asks for. Check
both before assigning long work.

### Configuration precedence, and what travels into a task

OpenCode merges configuration in its documented order, later sources overriding
earlier ones for conflicting keys: remote `.well-known/opencode`, the global
`~/.config/opencode/opencode.json(c)`, `OPENCODE_CONFIG`, the project's
`opencode.json(c)`, `.opencode/` directories, `OPENCODE_CONFIG_CONTENT`, managed
configuration files, and macOS MDM preferences. Rules come from `AGENTS.md`, with
`CLAUDE.md` as a fallback when there is no `AGENTS.md`; the first match in each
category wins. Skills are discovered on demand, including under
`.opencode/skills/`, `.claude/skills/`, and `.agents/skills/`.

ahu's configuration snapshot carries the invoking checkout's `.opencode/`,
`opencode.json`, `opencode.jsonc`, `AGENTS.md`, `CLAUDE.md`, `.agents/`, and
`.claude/` into the task worktree, as described in
[what a task gets](#what-a-task-gets). The global, `OPENCODE_CONFIG`, managed, and
MDM sources belong to the host: they are not snapshotted, so a task can resolve
configuration ahu never saw and does not report.

An `opencode.json` can name `plugin` modules that OpenCode installs and runs at
startup. That is executable configuration travelling with the task, on the same
footing as hooks: ahu copies it and reports what it can see, decides nothing about
it, and provides no OS isolation. Review a repository's OpenCode configuration
before launching an agent in it.

### Register an OpenCode agent

`harness = "opencode"` accepts `permissions = "prompt"` and `permissions = "auto"`;
`accept-edits` is refused. On this harness `prompt` is not an approval gate—see
[delegation and approval boundaries](#delegation-and-approval-boundaries) for the
mapping and the reason.

Write the manifest as `.agents/ahu/agents/glm-reviewer.md`, instructions in the
body:

```markdown
---
okf_version: 0.2
type: ahu:agent
title: glm-reviewer
description: Reviews assigned changes and reports findings
status: stable
tags: [agents]
harness: opencode
model: ollama/glm-5.3:cloud
permissions: prompt
version: 1.0.0

---

Review the assigned changes and report findings with file and line references.
```

Preview before launching anything:

```sh
ahu agents
ahu launch @glm-reviewer --prompt 'Review the configuration validation and report findings.' --dry-run
```

The preview's `argv` shows the command ahu builds:
`opencode --model ollama/glm-5.3:cloud --prompt <PROMPT>`, with `--auto` added
only for `permissions = "auto"`. The prompt is one argument and never passes
through a shell; see [prompts are data](#prompts-are-data). Then submit the real
assignment:

```sh
printf '%s\n' 'Review configuration validation and report findings.' > assignment.txt
ahu launch @glm-reviewer --prompt-file assignment.txt
```

This manifest declares `prompt`, so no approval-widening flag is needed. A
manifest declaring `auto` requires `--allow-widened-approvals` on previews and
launches alike. Interactive launches need cmux. The same agent runs unattended
with `--headless`, which builds `opencode run --format json` instead and needs
no cmux; see [headless execution](#headless-execution).

`auto` is the widest of the three and OpenCode has no sandbox to narrow it:
unlike a Codex launch, which ahu gives `--sandbox workspace-write`, an OpenCode
session's file tools act on whatever absolute path the model names. A live
1.18.30 run under `--auto` wrote its file into the parent checkout rather than
the task worktree it was launched in. Headless attempts disclose such paths
when the captured event stream exposes them: the result envelope records
`writes_outside_worktree`, and an empty `ahu diff` points to it on stderr. The
worktree is where the session starts, not a boundary the harness is held to;
the launch preview says so.

For a coordinating OpenCode session in the current terminal, rather than an
agent in a task worktree:

```sh
ahu opencode
```

That session inherits the terminal and the working directory, and ahu passes it
no arguments at all: OpenCode's permission actions are its own configuration's
to decide, and the only flag that would change them widens them.

### Troubleshooting

ahu's own preflight covers Git, project configuration, the manifest, and the
`opencode` executable on `PATH`. Everything past launch—endpoint reachability,
provider resolution, model availability, authentication, and context limits—is
between OpenCode and Ollama.

| Symptom | Your check | What ahu does |
| --- | --- | --- |
| `opencode` not found; exit `4` | `command -v opencode` | Reports the missing prerequisite and stops before creating a worktree. |
| The session cannot reach the endpoint | `curl -sS http://localhost:11434/v1/models` | Does not probe the endpoint. The failure surfaces in the session; the task, branch, and worktree remain. |
| `ollama/glm-5.3:cloud` does not resolve; `opencode models` lists only `opencode/*` | The `provider` block in `opencode.json` or the global configuration | Does not write or repair OpenCode configuration. |
| The tag is absent from `ollama list` | `ollama list`, then pull the tag yourself | Does not pull or create models. |
| Hosted-model authentication fails | Ollama's own sign-in for cloud models | Holds no provider credentials and cannot authenticate for you. |
| Context below 64k | `ollama show <model>`; `limit.context` in the provider entry | Does not set or raise context limits. |
| Manifest declares `accept-edits` | The manifest's `permissions` value | Refuses the manifest with an error and offers no substitute mode, interactively and headless alike. Declare `prompt` or `auto`. |
| A headless launch reports no terminal event | The captured events under the task's artifacts | Scores a run only on OpenCode's own terminal step, never on its exit status: a refused tool call ends the run with exit 0. |

In every one of these cases ahu reports and stops. It never silently selects
another model, provider, or harness, and a failed session does not remove the
task's branch, worktree, or record.

## What a task gets

Inspect and review a task from the checkout that launched it:

```sh
task_id=abc123  # replace with a task ID from ahu tasks
ahu task "$task_id" --output json
ahu diff "$task_id"
```

Both commands accept a full task ID or an unambiguous prefix. `ahu focus`
uses the newest record matching its prefix without checking ambiguity; use
a full ID when focusing a task. Inspection reads
the saved record without contacting cmux or updating it. JSON schema version 1
includes `task_id`, `agent`, `harness`, `model`, `branch`, `base_commit`,
`worktree`, `worktree_exists`, `record_path`, `cmux_workspace_id`, and
`cmux_window_id`. `session_state` is the recorded `starting`, `running`,
`exited`, or `failed` state; `state_source` is `record` and
`completion_verified` is always `false`. Missing optional values are JSON null.
JSON remains unstyled even with `--color=always` and omits prompt text and titles.
Consumers should tolerate additional fields and check `schema_version`.

The interactive task list labels states such as `session running` and `session exited`.
A running session may be awaiting input; a process exit does not verify success.

`ahu diff` compares the launch base to the current task worktree, including
committed, staged, and unstaged tracked changes, including inherited agent
configuration. Untracked files are listed on stderr and are not included in the
patch; ignored files are omitted. Git external diff helpers and text conversion
are turned off. Terminal output escapes control characters; redirected stdout
preserves the patch bytes. If the patch is empty and the attempt's result
envelope records write-tool paths outside the worktree, the empty output is
qualified on stderr with those paths; see `ahu result`. The command fails if
the checkout is missing or belongs to another repository. Neither command
stages, commits, or applies changes.

- **A fresh branch and worktree.** `ahu/<agent>/<task-id>`, based on the HEAD of
  the checkout you launched from, under `.worktrees/` in the primary checkout.
  Repeated and concurrent launches always produce distinct tasks.
- **Recognized repository agent configuration.** Recognized `.agents/`, `.claude/`,
  `.codex/`, `.agent/`, and `.opencode/` directories, plus `CLAUDE.md`,
  `CLAUDE.local.md`, `AGENTS.md`, `AGENTS.override.md`, `.mcp.json`,
  `opencode.json`, and `opencode.jsonc` files, are
  copied at their native paths, including uncommitted and Git-ignored files, and
  configuration you have deleted locally stays deleted. Unrelated dirty source
  files stay in your original checkout. Some of this is executable
  configuration—hooks, and OpenCode `plugin` modules—so it travels with the
  same trust consequences as the rest of the repository.
- **The configured harness and model at launch.** The preview records the
  executable found on the submitting shell's `PATH`. For interactive startup, `run-task`
  resolves the same harness name on the workspace's `PATH`; cmux wrappers can
  differ between surfaces. ahu reports a changed path and rejects relative or
  repository-local executables. ahu does not substitute a model or pass permission
  flags beyond those requested
  by the manifest. The harness controls model changes during the session.
- **A frozen record.** The agent version, both digests of its source file, the
  configuration snapshot digest, the policy digest, the base commit, branch,
  worktree, and cmux ids are written to local task metadata. Editing an agent
  later changes the next launch; a running task keeps what it started with.

### Two digests, each named

A source file and the text ahu delivers from it are not the same bytes when the
format has YAML frontmatter, so `ahu` records and shows both, always labelled:

- **file digest**—SHA-256 of the complete file at `source.path`, exactly as it
  is on disk, frontmatter included. This is the one to compare against the
  repository.
- **instructions digest**—SHA-256 of exactly the text `ahu` puts in the prompt:
  the same file with its frontmatter stripped, byte-for-byte identical to what
  lands inside the `<<<ahu-agent-...>>>` fence.

For a format with no frontmatter the two cover the same bytes and come out equal.
Both are folded into the agent's identity digest, so a change to either is
drift—and drift says which one moved, because an edit to frontmatter alone
changes the file without changing anything the model was given.

Exiting the harness keeps the worktree, the branch, and the task record. A
process exit is not evidence that the task succeeded, and `ahu` never deletes
your work for you.

## Prompts are data

A task prompt can contain anything—`$(...)`, backticks, pipes, newlines. `ahu`
never puts a prompt into a shell command. The cmux startup command contains only
`ahu`'s own executable path and task directory, both shell-quoted; the prompt is
written to a file and handed to the harness as a single argument.
The prompt file is written owner-only. The task record stores its digest and
a placeholder rather than the full prompt; sidebar titles and summaries can
still contain excerpts derived from the assignment.

Repository content is also treated as untrusted on the way *out*. Hook commands,
agent descriptions, native frontmatter keys, and file names are all rendered with
control characters escaped, so a repository cannot use terminal escape sequences
to repaint or erase the disclosures in the launch preview.

Configuration is materialized into a task worktree without following symlinks.
Configuration symlinks are omitted from the snapshot and reported as gaps;
materialization removes configuration symlinks inherited from the base commit.
Unsafe path components are refused. The scan skips build, dependency, state,
and worktree directories and stops below its depth limit. Files already committed
under skipped paths still arrive through Git; local edits there are not copied.

## Hooks

Hooks run on harness lifecycle events and can affect tool calls or context.
ahu inventories Claude Code hook settings and reports Codex, Antigravity, and
OpenCode hook coverage as unknown. It does not add, edit, remove, or turn off hooks.

Only a hook's program is shown, never its arguments, and only its digest is
stored in the task record—hook commands routinely carry tokens, and an
inventory must not leak a credential merely to describe a hook.

For Claude Code, `ahu inventory` lists hooks ahu can read from
`.claude/settings.json`, `.claude/settings.local.json`, `~/.claude/settings.json`,
and managed settings, with event, matcher, scope, and digest. `ahu doctor` shows
this hook section only when the project harness preferences include Claude Code;
it omits it for this repository's Codex-only preferences. Codex, Antigravity, and
OpenCode launch previews explicitly report unknown hook coverage. For scanned hooks,
the launch preview says what each one means for the task:

- **Recognized repository hook configuration travels into the task worktree**,
  with executable bits preserved. The harness decides whether and when hooks run. The preview
  names them and counts how
  many inherited configuration files are executable.
- **Hooks outside project policy raise a warning.** A hook in
  `settings.local.json` travels but is typically Git-ignored, so it may never have
  been shared or reviewed. A hook in a home directory or machine policy does not
  travel at all and can differ for every teammate. ahu prints the specific reason
  per hook rather than one blanket claim.
- **A hook change is drift.** The hook digest spans every scope ahu can read,
  including ones the repository snapshot cannot see, so a teammate's personal hook
  changing between two `chris@1.2.0` launches is reported rather than hidden under
  the same version label.

What ahu cannot see: hooks injected by the cmux Claude wrapper, and hooks
contributed by plugins. Both are reported as gaps rather than omitted. A settings
file that exists but cannot be parsed is reported as *unknown* hooks, never as
*no* hooks.

## Context inventory and hygiene

`ahu inventory` lists what can influence an agent: its fixed identity, the
repository instructions, skills, hooks, memory sources, MCP configuration,
personal configuration from your home directory, managed policy, and the task
prompt.
Sources are marked `loaded`, `available`, `disabled`, `opaque`, or `absent`, and
the report ends with what `ahu` cannot see. It is never labelled
complete—`available` means the harness can discover a source, not that its
contents reached the model.

On an agent's first load, and then on the project's cadence
(`context_hygiene.review_interval_days`, a project-wide setting), `ahu` reviews
memory and skills that may be influencing the agent and shows a concrete cleanup
proposal. It deletes nothing, disables nothing, and stages nothing. Where a
source is shared with other agents or projects, it says so rather than presenting
a global deletion as a local one.

## State and compatibility

Headless tasks use the external runtime store described in the preceding section. `tasks`, `task`,
and `diff` include those records alongside interactive and legacy records.
`focus` is for interactive cmux sessions. The following worktree-local layout
and `AHU_STATE_DIR` rules describe interactive tasks and compatible legacy state;
`AHU_RUNTIME_DIR` independently selects the headless store.

Each interactive task stores `task.json` and `prompt.txt` under
`<task-worktree>/.ahu/state/repos/<repo-identity>/tasks/<task-id>/`. Session status
is part of `task.json`. ahu derives this location from the worktree it creates;
ordinary invocations need no manual `AHU_STATE_DIR` export. Removing the worktree
removes its state. Task checkouts are siblings under the primary checkout's `.worktrees/`,
including nested launches. State and worktree directories ignore themselves.

`tasks`, `task`, `diff`, and `focus` discover task records through those worktrees
from the primary checkout or a sibling. Each managed worktree store accepts only
its owner's task ID, matching repository identity and canonical worktree path.
This rule applies on every scan, including from inside that worktree; its store
is not scanned again as an unrestricted legacy store.

Compatible legacy records remain readable in the primary checkout, an invoking
plain checkout, or an explicitly selected external store. A managed worktree's
accepted record takes precedence for a duplicate ID. Older nested child records
kept in a parent task worktree are reported as misplaced for inspection, not
accepted or migrated automatically. Persisted schema compatibility is unchanged.

`ahu tasks` reports misplaced entries in managed stores as warning notes, without
adding task rows or changing the files. Invalid task/repository identity in a
legacy store produces an unreadable row. Unreadable or refused owner records
remain visible for inspection. After checking legacy stores, a worktree with no
record accounting for its task is reported as incomplete; a stray record cannot
hide it. Such a worktree may still be preparing or may remain from a failed
launch. Missing worktrees have no worktree-local record to list; independent
legacy records are not automatically deleted.

On launch failure, rollback uses ordinary Git worktree removal, without force.
If removal fails, the error names the retained worktree and branch for inspection.
ahu attempts to discard partial task state only after validating its path; it
leaves redirected paths alone. Cleanup can itself fail, so inspect the reported
paths. A retained worktree without a record appears as incomplete in `ahu tasks`.
Listing does not remove it or establish that its contents are safe to delete.

Only the launch lock and cmux group mapping need shared coordination state, under
`<primary-checkout>/.ahu/state/repos/<repo-identity>/`. Hygiene timestamps and
generated architecture documents use the invoking checkout's `.ahu/state/`.

`AHU_STATE_DIR` overrides that auxiliary store. The code uses the supplied path
as-is, without requiring an absolute path or migrating records; use a consistent
absolute path for tools that need an override. A value naming `.ahu/state` of
any checkout that Git identifies as belonging to the same repository is treated
as automatic session wiring for coordination and legacy lookup. This retains
primary-checkout coordination and normal legacy lookup: the primary store and
an invoking plain checkout's store, with managed worktrees checked only as owned
stores. The value must name a checkout root's `.ahu/state`; a store under a
subdirectory is a separate explicit selection. This holds even when the variable
names a sibling's store. Other values
select their own coordination and legacy store instead of those default stores.
In either case, discovery still scans task worktrees, and interactive task records and
prompts still go inside their own worktree. The launched harness receives
`AHU_STATE_DIR` set to its own worktree's `.ahu/state`, replacing any inherited
override. Harness configuration and credentials retain their native handling.

Default state paths refuse existing symlinks at `.ahu`, `state`, and descendant
directories and files; state files must be regular files. For a user-selected
override outside the checkout-store layout, the root and its ancestors are the
user's selection; descendants remain checked. Files are created owner-only on
Unix and replaced through temporary files. These checks refuse static path
redirection; they do not prevent a concurrent host process from replacing paths
between inspection and use.

At startup, ahu checks prompt and delivered-instruction digests, reconstructs the
launch command, and checks repository and worktree identity, including which task
worktree owns a local record. Records and their digests are mutable local files,
not authenticated evidence against a writer able to change both. Neither these
checks nor Git worktrees provide OS isolation or protection against concurrent
hostile host processes.

### Utility lookup

Git and default cmux lookup skip empty and relative `PATH` entries. Candidates
must be executable, canonicalize to an absolute path, and be outside registered
repository roots and Git working trees. The resolver inspects each canonical
candidate's parent ancestry for `.git` with filesystem metadata, without running
candidate Git or following `.git` pointers. Any `.git` entry (including a file or
symlink), inspection error other than absence, or ancestry beyond 256 directories
rejects the candidate. This also excludes executables in unopened sibling or
unrelated working trees. Symlinked installations outside working trees remain
usable; aliases resolving into a working tree do not.

The selected canonical Git path is used for that Git invocation; default cmux
keeps its canonical path across calls on the discovered client. `AHU_CMUX_BIN`
is a deliberate user selection and retains normal command semantics: a bare name
uses command lookup, and relative or absolute paths are accepted without the
default working-tree exclusion. These are executable selection checks, not a
sandbox, and they do not prevent later replacement by a host process.

The catalog in `src/catalog.rs` records, per adapter, the CLI version its
behavior was verified against and the enforcement gaps ahu discloses for it.
These are recorded compatibility baselines, not claims that newer versions or
account entitlements were tested. Where more than one installation of a harness
is on `PATH`, ahu runs and reports the one its own resolution picks; it does not
search for a version that matches the catalog. An entry may name more than one
verified version, comma-separated, and the prerequisite check accepts any of
them. That exists because a harness can replace its own binary in place between
launches: OpenCode did so during this adapter's verification, moving from
1.18.29 to 1.18.30 with no user action, and a single-version entry would have
warned every user on the newer build that it was unverified when it was not.
Project configuration pins catalog `2026-09-13`; a mismatch is an error.

Persisted task records use schema 2; launch-preview and task-inspection JSON use
schema 1. Incompatible persisted records are refused and listed as unreadable;
ahu does not reinterpret their digests or delete their worktrees.

## Delegation and approval boundaries

Outside ahu, delegation belongs to the current harness. Registered assignments
inside ahu use registered ahu agents running their configured
harnesses and models in separate worktrees. Interactive tasks use cmux workspaces;
headless descendants inherit the headless state store and external runtime root.
Headless child grants and bounded native helpers follow the preceding policies.
The supplied contract requires
`ahu agents`, a complete UTF-8 assignment file, and `ahu launch @name
--prompt-file assignment.txt`. `AHU_BIN` points to the launching ahu executable.

Specify the absolute source checkout and revision or diff scope when assigning
a review of uncommitted work: children start at HEAD and do not copy dirty source
files. Specify where findings belong and read them before reporting completion.

ahu sends its delegation contract, the agent instructions, and the task prompt
in that order as one prompt argument. Its sections have a per-launch nonce in
their fences. These delimiters identify supplied text; they do not enforce
authority. No adapter passes an agent-selection or system-prompt flag, and ahu
cannot stop the harness or its shell tools from starting other processes.

For interactive launches, `permissions = "prompt"` passes no approval flag. `accept-edits` and `auto`
request adapter-specific flags; `ahu launch` requires
`--allow-widened-approvals` for either, including dry runs. Existing harness
settings still affect approvals. The flag grants no additional access to the
calling session.

`prompt` therefore means *ahu imposes nothing*, not *the session will ask*. What
happens next is the harness's own default, and the harnesses do not agree:

| Manifest `permissions` | OpenCode | Effect |
| --- | --- | --- |
| `prompt` | no flag passed | OpenCode's own permission configuration decides. Its defaults allow most tools outright, so an OpenCode agent can edit files and run commands without asking. The built-in `build` agent's only `deny` rules are `question`, `plan_enter` and `plan_exit`; reads of `*.env` and `*.env.*` default to `ask`, not `deny`, and so do `doom_loop` and `external_directory`. |
| `auto` | `--auto` | Approves every permission that is not explicitly denied. An `ask` rule is not a denial, so `--auto` approves it silently: with OpenCode's defaults that includes reading `*.env` and `*.env.*`, and reaching outside the project directory. If you need those refused rather than merely prompted, write a `deny` rule in your own OpenCode `permission` configuration; ahu does not add one. Still requires `--allow-widened-approvals`. |
| `accept-edits` | rejected | OpenCode has no accept-edits flag and its permission actions are static configuration, so ahu refuses the manifest instead of approximating the mode. No fallback to `prompt` or `auto` exists. |

Readers arriving from Claude Code or Codex should not carry over the assumption
that `prompt` gates edits and commands: on OpenCode it does not. Express finer
rules in OpenCode's own `permission` configuration, whose actions are `ask`,
`allow`, and `deny` per tool. No permission flag on any harness gives OS-level
isolation.

ahu passes no `--agent` flag on OpenCode. A wrong agent name does not fail there:
OpenCode reports that the agent was not found and continues with its default
agent, so the flag cannot confirm that an identity was applied. `--agent` would
also override the agent's own model and permissions, contradicting the exact
model the manifest pins. As on every other harness, the agent's instructions and
ahu's delegation contract travel as prompt text—delivery, not enforcement.

`ahu codex` opens Codex in the current Git checkout with `--sandbox
workspace-write --ask-for-approval on-request` and Codex's configured model. It
creates no task or worktree and needs neither project configuration nor cmux.
Start it from a terminal; an existing outer sandbox still applies.

`ahu claude` opens Claude in the current Git checkout using its configured model
and permission behavior. It passes no model or permission overrides and accepts
no additional arguments. Like `ahu codex`, it preserves the invoking directory
and terminal, sets `AHU_BIN` and checkout-local `AHU_STATE_DIR`, and creates no
task, worktree, or cmux session.

`ahu opencode` does the same for OpenCode, and passes no arguments either.
Unlike `ahu codex`, it names no sandbox or approval mode: OpenCode's permission
actions are static configuration, and its one permission flag, `--auto`,
approves everything not explicitly denied without prompting. A coordinating session that
passed it would widen the user's own boundary on their behalf, which is the
opposite of what the adapter does when an agent asks. `--pure` is not passed
either, so the user's own plugins load exactly as they do outside ahu.

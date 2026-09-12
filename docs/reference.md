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
execution still requires cmux. Manifests that widen approvals require the
explicit `--allow-widened-approvals` flag for both previews and execution, as in
the examples above. JSON output is supported only with
`--dry-run`.

The JSON object has `schema_version: 1` and these fields:

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
styling is disabled when `NO_COLOR` is set to any value, `TERM=dumb`, or stdout
is not a terminal. Redirected output is therefore plain by default. JSON stdout
never contains styling, even with `--color=always`.

Agent identity, harness/model, warnings, enforcement gaps, drift, and hints have
distinct styles. Labels and layout retain their meaning with color disabled.
Repository-controlled strings are escaped before styling so they cannot inject
terminal controls. The composer remains line-oriented: color does not change
the `.` sentinel, `.cancel`, or the confirmation code required for submission.

## Registering an agent

`ahu` launches an agent only when `.agents/ahu/agents/<name>.toml` registers it.
Definitions found elsewhere are onboarding candidates, never implicit
registrations — a skill is not an agent, and `AGENTS.md` is not an agent
registry.



`onboard` offers registration for Claude Code Markdown definitions. It lists
Codex TOML and Antigravity native definitions with blockers even though both
harness adapters exist; this native-onboarding path cannot register them.
Explicit manifests can use plain Markdown with any supported harness;
`antigravity-agent` also reads a Markdown body after frontmatter. An explicit
`codex-agent` source is delivered verbatim as text; its TOML fields are not parsed
as native model or instruction metadata. Plain Markdown makes the delivered
instructions explicit.

For the registration example below, first create the matching definition:

```sh
mkdir -p .claude/agents
printf '%s\n' 'Help implement repository tasks.' > .claude/agents/chris.md
```

A manifest references that definition in place:

```toml
schema_version = 1
name = "chris"
version = "1.2.0"
description = "Helps implement repository tasks"
harness = "claude-code"
model = "claude-opus-5"

[source]
format = "claude-agent"
path = ".claude/agents/chris.md"
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
the two must agree; `ahu` will not rewrite either file or
pick one silently.

Every named agent needs a semantic version. `ahu` does not manage releases for
you, but it will tell you when a version label has stopped matching its inputs:
if `chris@1.2.0` launches with different instructions or a different repository
configuration than the last `chris@1.2.0` launch, that drift is reported as a
pending behavior change for the next version bump.

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

The task list labels states as `session running`, `session exited`, and so on.
A running session may be awaiting input; a process exit does not verify success.

`ahu diff` compares the launch base to the current task worktree, including
committed, staged, and unstaged tracked changes, including inherited agent
configuration. Untracked files are listed on stderr and are not included in the
patch; ignored files are omitted. Git external diff helpers and text conversion
are disabled. Terminal output escapes control characters; redirected stdout
preserves the patch bytes. The command fails if the checkout is missing or belongs
to another repository. Neither command stages, commits, or applies changes.

- **A fresh branch and worktree.** `ahu/<agent>/<task-id>`, based on the HEAD of
  the checkout you launched from, under `.worktrees/` in the primary checkout.
  Repeated and concurrent launches always produce distinct tasks.
- **Recognized repository agent configuration.** Recognized `.agents/`, `.claude/`,
  `.codex/`, and `.agent/` directories, plus `CLAUDE.md`, `CLAUDE.local.md`,
  `AGENTS.md`, `AGENTS.override.md`, and `.mcp.json` files, are
  copied at their native paths, including uncommitted and Git-ignored files, and
  configuration you have deleted locally stays deleted. Unrelated dirty source
  files stay in your original checkout.
- **The configured harness and model at launch.** The preview records the
  executable found on the submitting shell's `PATH`. At startup, `run-task`
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

- **file digest** — SHA-256 of the complete file at `source.path`, exactly as it
  is on disk, frontmatter included. This is the one to compare against the
  repository.
- **instructions digest** — SHA-256 of exactly the text `ahu` puts in the prompt:
  the same file with its frontmatter stripped, byte-for-byte identical to what
  lands inside the `<<<ahu-agent-...>>>` fence.

For a format with no frontmatter the two cover the same bytes and come out equal.
Both are folded into the agent's identity digest, so a change to either is drift —
and drift says which one moved, because an edit to frontmatter alone changes the
file without changing anything the model was given.

Exiting the harness keeps the worktree, the branch, and the task record. A
process exit is not evidence that the task succeeded, and `ahu` never deletes
your work for you.

## Prompts are data

A task prompt can contain anything — `$(...)`, backticks, pipes, newlines. `ahu`
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
ahu inventories Claude Code hook settings and reports Codex and Antigravity
hook coverage as unknown. It does not add, edit, remove, or disable hooks.

Only a hook's program is shown, never its arguments, and only its digest is
stored in the task record — hook commands routinely carry tokens, and an
inventory must not leak a credential merely to describe a hook.

For Claude Code, `ahu inventory` lists hooks ahu can read from
`.claude/settings.json`, `.claude/settings.local.json`, `~/.claude/settings.json`,
and managed settings, with event, matcher, scope, and digest. `ahu doctor` shows
this hook section only when the project harness preferences include Claude Code;
it omits it for this repository's Codex-only preferences. Codex and Antigravity
launch previews explicitly report unknown hook coverage. For scanned hooks,
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
the report ends with what `ahu` cannot see. It is never labelled complete —
`available` means the harness can discover a source, not that its contents
reached the model.

On an agent's first load, and then on the project's cadence
(`context_hygiene.review_interval_days`, a project-wide setting), `ahu` reviews
memory and skills that may be influencing the agent and shows a concrete cleanup
proposal. It deletes nothing, disables nothing, and stages nothing. Where a
source is shared with other agents or projects, it says so rather than presenting
a global deletion as a local one.

## State and compatibility

Task records and hygiene timestamps default to `.ahu/state/` in the invoking
checkout. Each linked worktree has its own store. Run `ahu tasks`, `ahu task`,
`ahu diff`, and `ahu focus` from the checkout that launched the task. The shared
launch lock and cmux group mapping live in the primary checkout's `.ahu/state/`.
All task checkouts are siblings under its `.worktrees/`, including child tasks.
Both state and worktree directories contain their own ignore files.

`AHU_STATE_DIR` explicitly overrides the state store and coordination location.
Harness configuration and credentials retain their native handling.

The catalog in `src/catalog.rs` records adapter verification against Claude Code
2.1.269, Codex 0.154.0, and Antigravity CLI 1.2.2. These are recorded compatibility
baselines, not claims that newer versions or account entitlements were tested.
Project configuration pins catalog `2026-09-12`; a mismatch is an error.

Persisted task records use schema 2; launch-preview and task-inspection JSON use
schema 1. Incompatible persisted records are refused and listed as unreadable;
ahu does not reinterpret their digests or delete their worktrees.

## Delegation and approval boundaries

Outside ahu, delegation belongs to the current harness. Inside an ahu task,
sub-agents and fan out mean registered ahu agents running their configured
harnesses and models in separate cmux workspaces. The supplied contract requires
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

`permissions = "prompt"` passes no approval flag. `accept-edits` and `auto`
request adapter-specific flags; `ahu launch` requires
`--allow-widened-approvals` for either, including dry runs. Existing harness
settings still affect approvals. The flag grants no additional access to the
calling session.

`ahu codex` opens Codex in the current Git checkout with `--sandbox
workspace-write --ask-for-approval on-request` and Codex's configured model. It
creates no task or worktree and needs neither project configuration nor cmux.
Start it from a terminal; an existing outer sandbox still applies.

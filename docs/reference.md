# CLI and context reference

[Back to README](../README.md). Run `ahu help` for option syntax and
`ahu explain` for the built-in architecture overview.

## Repository and task selection

`ahu --repo /path/to/project <command>` selects the checkout before parsing the
command; `--repo=/path/to/project` is equivalent. Command paths are relative to
that checkout. Without this prefix, ahu uses the current directory. Ambient
`AHU_REPO_ROOT`, `AHU_STATE_DIR`, `AHU_RUNTIME_DIR`, and `AHU_TASK_INDEX_DIR` do
not select storage. See [state and compatibility](#state-and-compatibility) for
explicit lookup of old stores.

## Installing and updating ahu safely

On macOS, use `cargo install` to update an installed ahu binary. Cargo replaces
the destination with a fresh file. For a locally built release, remove the
destination before copying the artifact:

```sh
rm -f "${CARGO_HOME:-$HOME/.cargo}/bin/ahu"
cp target/release/ahu "${CARGO_HOME:-$HOME/.cargo}/bin/ahu"
```

Do not copy a new build over an installed path while an ahu process may still
be running. In-place replacement can leave later executions killed by macOS
code-signature validation, even though the file appears valid on disk. The
running process keeps its old file open; new invocations need the fresh file.
An overwritten path that exits with status 137 and produces no output should be
replaced using one of the procedures described earlier before investigating another cause.

Task commands accept exact `@name` handles, `ahu:task:<id>`, bare IDs, uppercase,
and unambiguous ID prefixes. References to other resource kinds are refused.
`ahu launch @name` selects a registered agent; `ahu task @name` selects a task.

### Task handles

New tasks receive a short name derived from up to four words of their displayed
title. Repeated titles add a numeric suffix: `@parser-cleanup`, then
`@parser-cleanup-2`. `--name parser-cleanup` or `--name @parser-cleanup` chooses an
explicit name instead; an existing reservation rejects the launch.

Names start with a letter and contain at most 48 letters (`a-z`), digits, or
single hyphens. They are case-insensitive and displayed in lowercase. A handle must match
in full; `@parser` is not a prefix match for `@parser-cleanup`.

Handles are immutable and scoped to the repository. Linked checkouts share them;
use `ahu --repo <checkout>` for another repository. All task controls accept them,
including `task`, `focus`, `diff`, `message`, `wait`, `result`, `cancel`, `resume`,
`cleanup`, and `remove`. Resume retains the same task identity and handle.

Reservations remain after removal or a failed launch. They are not recycled or
renamed, so an old command cannot target a later task. A removed task's handle
still names its original task ID and reports that its task is unavailable. Two
owner-only bindings under the primary checkout's coordination store must agree
before resolution; incomplete, corrupt or redirected bindings are refused.
These checks do not isolate processes running as the same OS user.

Existing tasks keep their canonical references and are not renamed automatically.
Human output shows the handle alongside canonical identity. JSON retains
`task_id` and `task_ref`; `task_handle` is an additive field and is null when no
verified handle is available. Dry-run previews show `task_handle_candidate` and
`task_handle_reserved: false`; they create no reservation. The actual generated
name can acquire a suffix at submission if another task claimed it meanwhile.
Registered agent names and task handles may share text; the command supplies
their distinct meaning. Names locate tasks and do not grant authority.

The `@` sigil is interpreted by command context: agent-selection positions such
as `ahu launch @dev-astra` select a registered agent, while task-reference
positions such as `ahu task @storage-cleanup` select an immutable task handle.
The canonical task reference is `ahu:task:<uuid>`. Provider-owned native session
IDs remain labeled locators in inspection output; they do not become ahu task
references or grant permission to retrieve provider history.

## MCP integration

`ahu mcp serve` starts a local newline-delimited JSON request/response server on stdin and
stdout. Its first tools are read-only and repository-scoped:
`ahu_agents_list`, `ahu_tasks_list`, and `ahu_task_get`. The server reports
canonical task IDs and verified `@name` handles while using ahu's existing
repository ownership and task-resolution rules. Protocol handles describe
inspection operations and do not replace ahu launch records or identities.

`ahu mcp setup` materializes the skill bundle shipped with this ahu build into
`.agents/skills/`. OpenCode and Codex discover that canonical tree natively;
Claude Code does not discover it; its documented skill locations are `.claude/skills/` and
harness-managed paths, and `ahu mcp setup` does not duplicate the bundle into
`.claude/skills/`. Operators who want Claude Code to load a canonical skill can
copy or symlink that skill's directory into `.claude/skills/` and commit it, as
an ordinary project file. See [skill verification](skill-verification.md) for
per-harness probe protocols and recorded evidence. The files are ordinary
project files for review and commit. Setup refuses to overwrite a changed file,
so local skill edits cannot be silently replaced. This is the temporary
delivery path while installed harnesses lack verified native Skills-over-MCP
support; the bundle can move to an independent repository later without
changing the MCP task boundary.

Setup is additive and ordered. It refuses to overwrite a changed existing file;
if a later bundled skill differs, earlier skills may already have been written
before the command reports the refusal. Setup never removes skills that are no
longer bundled, so removal remains an explicit repository edit.

The canonical tree follows a strict contract so every harness can load it: one
directory per skill directly under `.agents/skills/`, named after the skill,
holding exactly one `SKILL.md`. The frontmatter carries only the portable keys
`name` (equal to the directory name) and `description` (a one-line summary).
Skill names use lowercase letters, digits, and hyphens. `scripts/check-skills.py`
verifies the tree shape, the frontmatter, unique names, and that the compiled
bundle in `src/mcp.rs` carries the same set of skills as the tree; CI runs it on
every change.

When the same skill name appears in more than one discovered location, each
harness resolves the collision by its own discovery precedence, which ahu
neither controls nor emulates: the harness picks its winner, and ahu reports
what it finds rather than overriding that choice. The repository contract keeps
collisions rare instead: `.agents/skills/` is the canonical location, `ahu mcp
setup` never duplicates skills into harness-owned locations such as
`.claude/skills/`, and any copy there is an ordinary file the operator can
review and remove. `ahu inventory` lists skill sources, including duplicates,
so a same-named skill outside the canonical tree stays visible.

The modern path targets the [2026-07-28 MCP specification](https://modelcontextprotocol.io/specification/2026-07-28)
and its [Tasks extension](https://tasks.extensions.modelcontextprotocol.io/specification/2026-07-28/tasks).
Modern stdio requests do not use `initialize`: every request carries
`io.modelcontextprotocol/protocolVersion: "2026-07-28"` and an object-valued
`io.modelcontextprotocol/clientCapabilities` in `params._meta`. `server/discover`
returns `resultType: "complete"`, supported versions, capabilities, cache hints,
and server identity under `_meta["io.modelcontextprotocol/serverInfo"]`.

Declare `io.modelcontextprotocol/tasks: {}` inside
`params._meta["io.modelcontextprotocol/clientCapabilities"].extensions` on every
Tasks request. Inspection calls return a persisted `working` handle before
execution. `tasks/get` returns the current state and its final tool result or
JSON-RPC error. A tool execution failure is a completed tool result with
`isError: true`; `failed` and `error` are reserved for protocol/execution
infrastructure failures. `tasks/update` answers outstanding input requests, and
`tasks/cancel` durably cancels an active inspection. Cancellation is idempotent;
completed results remain completed. Neither terminal protocol status nor an
inspection result accepts, merges, or approves harness work. No MCP tool
launches or changes harness permissions.

The stdio binding is newline-delimited UTF-8 JSON-RPC: each line is one request,
notification, or response, and stdout contains no other bytes. Diagnostics go
to stderr. Frames are limited to 1 MiB, including the newline; an oversized
frame receives `-32600` and is discarded through its newline so later frames can
still be processed. A dual-era client may probe `server/discover` and fall back to the
legacy handshake when the probe is not understood. Malformed JSON receives
`-32700`; invalid envelopes (including batches, missing/wrong `jsonrpc`, missing
or non-string methods, response-shaped messages, and invalid IDs) receive
`-32600` with a null ID. Request IDs must be strings or integers, not null,
true/false values, arrays, objects, or fractional numbers. This server sends no requests
to clients and does not accept response envelopes. Method parameters, when
present, must be objects (`-32602` otherwise).

An envelope with no ID is a notification, regardless of its method name. Valid
notification envelopes receive no response, even with unknown methods, invalid
parameters, or absent modern metadata. Supported inbound notifications are
advisory no-ops: notifications never select a protocol mode, queue inspections,
cancel Tasks, or change subscriptions. Use requests with IDs for those operations.
A `notifications/*` method sent with an ID receives `-32601`.

Unknown methods receive `-32601`. Unknown tools, missing/non-string tool names,
and invalid tool arguments receive `-32602` in both synchronous and Tasks paths,
before any handle is created. Arguments must be objects matching the advertised
schema: list tools accept no keys, `ahu_task_get` requires `task`, and only the
experimental adapter permits its omission. Selectors must be nonempty strings
of at most 256 UTF-8 bytes; arguments are limited to 8 KiB. Failures while
executing a valid inspection (such as a missing repository task) return text
content with `isError: true`, not a top-level JSON-RPC error. Modern synchronous
results, including tool errors and every `tools/list` variant, carry
`resultType: "complete"`; queued calls carry `resultType: "task"`, and their
stored final tool results carry `resultType: "complete"`.

Modern requests require version/capability metadata on every request (`-32022`
when absent or unsupported). A validated modern request locks out `initialize`
(`-32601`). A failed metadata/discovery-parameter check does not select a mode.
Legacy initialization locks out `server/discover` (`-32601`) and Tasks methods
(`-32021`). Subsequent modern metadata on a legacy connection is ignored: it
cannot opt into modern result shapes, asynchronous calls, or experimental tools.

The stdio host supplies `AHU_MCP_CALLER` as a stable authenticated principal for
each caller; without it, the effective local OS user is the principal. The host
must choose this value, retain it across reconnects, and use separate processes
for separate callers. Client metadata cannot set or override it. This is a
local transport boundary, not remote authentication: clients with the same OS
account and direct filesystem/process access already share that account's
trust. Handles are bound to this principal, repository identity, and checkout.
Old handles without ownership metadata are refused rather than reassigned.

The queue under the private repository coordination store (`mcp/tasks`) retains
ownership, operation arguments, status, timestamps, cancellation, and final
result/error. Atomic writes sync files and their directory before acknowledgement.
TTL is persisted as `null` (unlimited); there is no automatic retention cleanup.
On reconnect, a modern Tasks request starts recovery of that caller's queued
inspections in the same checkout. Workers use OS locks to avoid duplicate
execution and recheck cancellation before publishing results. A process crash
can replay an interrupted read-only inspection. Work pauses while no server
for that caller is running; this transport does not install a daemon.

For stdio notifications, send `subscriptions/listen` with
`notifications.taskIds` (up to 64 authorized IDs) and the Tasks capability.
The server emits `notifications/subscriptions/acknowledged` followed by
`notifications/tasks` snapshots when subscribed state changes, including changes
from another connection. Each listen replaces this connection's subscriptions;
reconnects require a new listen. Every subscription notification carries the
originating `subscriptions/listen` request ID in
`_meta["io.modelcontextprotocol/subscriptionId"]`. Notifications may coalesce
intermediate states; `tasks/get` remains authoritative. Task payloads are never
broadcast to other callers.

The optional experimental adapter is enabled by the host with
`AHU_MCP_TASKS_ADAPTER=inspection-v1`. It exposes `ahu_task_inspect` to modern
clients. A provided `task` selector behaves like `ahu_task_get`; omission requests
selection through `input_required`. When omitting the selector, the creating
client must also advertise `elicitation.form: {}`. Reply through
`tasks/update.inputResponses` with
`{"task-selection":{"action":"accept","content":{"task":"@reviewer"}}}`
and both capabilities. `decline` or `cancel` cancels the inspection. Updates
are limited to 8 KiB, selectors to 256 bytes, and the response can only fill that
pending selector. Unknown or already answered input keys are ignored; identity,
permissions, tool, and repository fields cannot be updated.

`initialize` selects the isolated `2025-11-25` (or an older requested handshake
revision) legacy inspection path for the connection. It always returns ordinary
synchronous tool results and refuses Tasks methods even if later requests
include modern capabilities. `tasks/list` and `tasks/result` are not
implemented.

### Long-running task records

Long-running or failure-prone work should have a durable record in the project's
private tracker before an ahu task is launched. An issue, task, milestone item,
or equivalent provider object records acceptance criteria, evidence, failures,
and disposition; ahu task state records execution details and is not the roadmap
record. Use the provider's native labels, tags, or fields for coordination: one
lifecycle state (planned, active, review, blocked, or done) and one marker for
the registered agent handling the current attempt. Do not use assignees for this
single-maintainer
workflow.

The coordinator owns comments, label changes, and closure. A task process
exiting successfully is not sufficient to close a record: inspect its report,
diff, validation, and delivery state first. Keep private tracker content out of
this public repository and its knowledge base. Before writing, verify the
provider's record, project, and linked-object visibility; a private project does
not necessarily make a linked public record private.

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
| `cmux_integration` | Shared local component evidence, activation/isolation uncertainty, and headless admission. |
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
identity, capabilities, gaps, timeout, and coordination path without launching.
Known cmux wrappers are refused; use the actual harness executable on `PATH`.

The admitted CLI profiles are Codex 0.154.0/0.155.1, Claude Code 2.1.269/2.1.270,
Antigravity CLI 1.2.2, and OpenCode 1.18.29/1.18.30/1.18.31/1.18.32. Other versions fail before
worktree creation, with no fallback harness or model. OpenCode's batch form is
`opencode run --format json`; its permission mapping is the interactive one, so
a manifest declaring `permissions = "accept-edits"` is refused here too. See
[OpenCode with Ollama-hosted models](#opencode-with-ollama-hosted-models) for the
provider setup an OpenCode agent needs. Profile admission describes the adapter's argument
surface, not successful authentication, provider availability, or full native
helper lifecycle validation. Codex 0.155.1 admission covers the batch launch
and recorded-session resume surfaces checked by compatibility probes; bounded
native helpers remain refused.

`ahu cmux status` (also `--output json`) checks installed CLI versions and
reports the same isolation profile used by launch previews. Inspection alone
without a version observation reports component compatibility only. An admitted
version and inspected components do not replace executable, approval, model, or
other launch checks.

| Harness | Required native isolation evidence |
| --- | --- |
| Codex | Absent sources or exact reviewed hooks with disable guards. Plugin state, cloud authentication/configuration, and managed sources remain unresolved. |
| OpenCode | Absent sources or the reviewed guarded Session plugin. Feed is unsafe; authentication/account stores, declared modules, and substitutions remain unresolved. |
| Claude Code | Direct executable avoids the cmux wrapper. Independent hooks, enabled plugins, and managed settings require separate evidence. |
| Antigravity | Absent inspected hooks and an exact reviewed CLI version. Custom hooks, extensions, and overrides remain unverified. |

Unknown or unsafe integrations have no operator bypass, including with
`--allow-widened-approvals`. Use interactive cmux execution while resolving native
sources with their owner; it retains the normal approval and launch checks.
ahu does not remove credentials, rewrite native settings, or treat interactive
availability as headless isolation. CLI version drift requires compatibility
validation before the reviewed version list changes. Evidence is bounded local
inspection; live delivery, provider availability, and sandbox behavior are not implied.

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

Stream evaluation is limited to 64 MiB per stdout/stderr stream and 1 MiB per
parsed event line. Exceeding a bound stops the attempt. These streams are parsed
in memory, not retained as ahu logs. Admission refuses a new task
when 16 tasks in the repository runtime store lack terminal results, including
interrupted tasks; there is no queue.

The timeout is positive seconds, default 1800 per attempt. `wait` follows the
current attempt until it stops, returning 0 for `succeeded` and 5 otherwise.
`result` reads the durable envelope without waiting; check its `outcome`, not
just the command's exit status. Outcomes include `running`, `succeeded`, `failed`,
`timed_out`, `cancelled`, `capture_failed`, `supervisor_error`, and `interrupted`. New result envelopes use schema 2 and separate process exit, bounded harness
outcome metadata, native references, and worktree identity. Native transcript,
final-answer, stderr, and helper-summary text is not copied into the envelope.
Schema 2 also records `writes_outside_worktree`: write-tool target paths from the
evaluated event stream that fall outside the task worktree, when the stream
exposes them. The field is disclosure for post-run review, not a boundary;
`ahu diff` prints it on stderr when the patch would otherwise look empty.
`acceptance` stays `not assessed` and `completion_verified` stays false: a provider
success or an agent's report does not establish that the assignment was accepted.
Treat native reports and same-user editable metadata as untrusted data.

Human-readable `tasks` identifies headless or cmux execution and summarizes the
headless attempt number and outcome. `task` and `result` add observed supervisor
ownership, recorded blockers, known native session identity with its source, artifact
locations, and commands for review. Recorded session state and the current
`owner.lock` observation are separate: `live`, `stale`, or `unknown` ownership
is not a measure of progress. A native session reference comes from the recorded
result or resume target; unknown references and native data locations stay
explicitly unknown. Inspection does not scan native transcripts.

Inspection reads bounded metadata and escapes and truncates human display fields.
It shows at most eight blockers, directing readers to result JSON for more.
Missing, malformed, unsupported, unsafe, or oversize inspection metadata is
reported as unavailable. `wait` validates result envelopes and refuses malformed
values or results belonging to another task or attempt. New coordination results have a 1 MiB persistence bound. The full-envelope
reader remains separate from the inspection limit. Internal lifecycle result reading
remains separate from inspection and its display bounds. Native locations are references, not proof that files exist. Legacy capture
paths may be displayed when inspecting an old schema-1 attempt. JSON retains structured values for review.

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

Headless records, frozen prompts, grants, broker requests, and bounded results live
under `<primary-checkout>/.ahu/state/repos/<repo-identity>/headless/<task-id>/`.
This owner-only coordination store survives deletion of a task worktree. ahu
records native session IDs with provenance; native data locations remain unknown
when unverified. Native settings, skills, histories, credentials, and retention
remain under each harness’s control. ahu does not copy event streams, stderr,
final answers, or helper summaries into its state.

`ahu cleanup "$task_id" --output json` requires a known terminal attempt and
removes recognized captured files if present and bounded broker request entries.
It retains results, frozen inputs, private broker claims/responses, native stores,
branches, and worktrees. It refuses unknown/interrupted ownership. Discovery
reads compatible old stores in place; it does not migrate them. New-binary resume
and child dispatch from legacy attempts are refused; use their original runner
or submit a new assignment. Do not assume any legacy worker can use a new binary.

Each new task has a version-7 universally unique identifier. The selected repository’s pointer index lives at
`<primary-checkout>/.ahu/state/task-index/`; it contains task ID, repository
identity, checkout, and store kind. Lookup checks local records before scoped
pointers, exact IDs before unique prefixes, and refuses stale or inconsistent
pointers. Primary and sibling worktrees share this scope; unrelated repositories
do not provide global task access.

`ahu message "$task_id" "text"` is the operator's delivery channel into a
task's inbox. It writes a numbered message file under the task's private
directory, capped at 100 entries and 1 MiB in total; delivery works from any
checkout of the selected repository through the same resolution. Working agents cannot deliver inbox
messages: ahu refuses when `AHU_WORKER_SESSION` is set, because delivery
belongs to the operator or to a broker-bound child, never to the task itself.
After the task reference, message arguments are literal payload: text such as
`--repo elsewhere`, `--color always`, and `--output json` does not change command
options. Messages are trimmed before writing; empty text is refused.
Tasks read inbox messages but never rewrite, renumber, or delete them, and a
task ID grants no delivery into any other task's directory.

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
repository coordination store. No global token cap or hidden task queue is promised.
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
| Codex 0.154.0/0.155.1 | Admitted | Refused: incomplete helper identity/join event visibility |
| Antigravity CLI 1.2.2 | Admitted | Refused: unvalidated native profile |
| OpenCode 1.18.29/1.18.30/1.18.31/1.18.32 | Admitted | Refused: no validated native tool switch |

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
All listed profiles must also pass native integration/isolation admission.
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

## Local OpenTelemetry

For local numeric headless attempt metrics without an exporter, set
`local_metrics = true` in `[telemetry]` and leave `enabled = false`.
Both options default to false and operate independently. Headless attempt results
then include a `metrics` object with `schema_version = 1`, six normalized
`ahu.tokens.*` fields under `values`, and
`token_aggregation = "maximum-reported-per-field"`. Each value has
`kind = "observed"` with an unsigned integer `value`, or
`kind = "unavailable"` without a value. Zero is an observation, not missing data.
No estimated values are produced; missing totals are never inferred.
Reported maxima are not additive task totals or billing measurements.
This option controls the new projection; it does not change existing
`harness.usage` collection or retention.

The containing result's existing task ID and attempt identify the observation.
The metrics object accepts no issue references, free text, paths, account data,
or arbitrary attributes. Any private mapping must be maintained separately
outside the repository; no tracker integration or mapping store is provided.
This object is not sent to OTLP or child environments. The complete task result
still contains existing coordination metadata and is not a safe export format.
Interactive sessions and attempts that stop before result persistence do not
produce this object. Resume produces a separate attempt, not a merged total;
metrics follow existing result retention and cleanup behavior.

### Private association boundary (library only)

`telemetry::private::PrivateMapping` provides an in-memory schema and numeric
summary primitive for private host adapters. It is not connected to
launch, resume, CLI, MCP, child environments, or exporters. No mapping store or
tracker client is installed. The existing checkout-local state store is not a
suitable privacy boundary for this association.

The bounded JSON input (at most 64 KiB) requires exactly `schema_version = 1`,
`record_key`, `repo_identity`, and `tasks`. The opaque record key is 1–256 ASCII
letters, digits, underscores, or hyphens; URLs and free text are unsupported.
The repository key is the existing machine-local 16-character lowercase hex
identity. Membership is an explicit list of 1–256 distinct canonical task UUID values;
paths, handles, and legacy task IDs are unsupported. Unknown or duplicate fields,
unsupported versions, and invalid values fail with a fixed error that includes
no submitted content. The mapping has no serialization or debug representation;
its key is accessible only through an explicit library method. Validation does
not establish tracker visibility, ownership, or authorization.

With `local_metrics` enabled, `summarize` accepts at most 4096 supplied numeric
observations, each scoped to that repository, a listed task, and a positive
attempt number. A retry submitted as a new task or a registered child requires
explicit membership; a task resume uses its existing task and new attempt. Identical
duplicates count once, conflicting duplicates fail without a partial result.
Per-field output reports the maximum observed value and counts of observed and
unavailable attempts. A null maximum means no observation; zero remains observed.
Missing totals are never inferred and values are never summed: resumed sessions
may repeat cumulative usage, and parent usage may overlap child usage. These are
coverage statistics over supplied observations, not complete task totals or
billing. Absent results, disabled collection, and undiscovered attempts are not
invented as observations. The caller must validate task ownership and extract
only opted-in numeric projections; this primitive does not read result envelopes
or prove completeness. No provider calls are involved.

The privacy requirements for any durable adapter include an explicitly
host-owned location outside every checkout and configuration snapshot, verified
tracker project and backing-record visibility, and owner-only,
symlink-resistant, atomic storage with locking and conflict handling. Keep mapping
keys out of task records, worktree names, prompts, MCP responses, shared configuration,
OTEL attributes, and diagnostics. Bind through validated repository/task identity
rather than caller-supplied paths; repository moves require explicit rebinding.
Require explicit removal and retention independent of task cleanup, bounded reads,
crash recovery, and failures that cannot affect launch or exporter outcomes.
No migration, automatic ancestry inheritance, cancellation behavior, filesystem
confinement guarantee, or same-user process isolation is added by this primitive.

### Local trace export

Trace export is disabled unless the project opts in. When enabled, ahu exports its
own launch and harness lifecycle traces to the project-configured local OTLP
collector and injects the same endpoint only into harness processes started by
ahu. It never changes the invoking shell or harness sessions started directly.
Only traces are enabled in this initial integration; inherited OTLP headers,
signal-specific endpoints, and log/metric exporters are cleared for the child.
Exporter construction or delivery failure does not fail the assignment.

```toml
[telemetry]
enabled = true
endpoint = "http://127.0.0.1:4318"
```

The exporter accepts only the local OTLP/HTTP endpoint on port
4318. Configure an upstream OpenTelemetry Collector to receive the endpoint and
write or route telemetry as needed. ahu adds normalized `ahu.*` attributes for
its version, agent and manifest version, harness and installed harness version,
requested model, provider-resolved model when the event stream reports one,
task, outcome, elapsed milliseconds, and any token usage the
harness event stream actually reports (`input`, `output`, `cached`,
`cache_write`, `reasoning`, and `total`). Missing usage remains absent; ahu never
estimates it. ahu does not add prompts, transcripts, credentials, or private
issue content to its span attributes. Inherited `OTEL_RESOURCE_ATTRIBUTES` are retained for
children, and the exporter SDK can read ambient OpenTelemetry configuration.
Keep sensitive data out of that configuration; these settings are not a
redaction boundary.

Harness event streams are not identical. Codex, Claude Code, Antigravity, and
OpenCode use different event names and terminal records, so ahu normalizes
terminal status and the common usage keys where they are present. Provider
native OTEL spans, if a harness emits them, remain harness-owned and may use
different semantic conventions. Interactive sessions generally provide timing
and process status; headless sessions additionally provide the structured event
usage fields that ahu can normalize.

Headless results also distinguish the skill catalog copied into the task
worktree from observed skill invocations. A catalog entry records only its
name, source path, and content digest. A skill invocation is recorded only when
the harness emits a recognizable skill/tool event; mentioning a skill in text,
having a skill on disk, or having an unrecognized event does not count as use.
Invocation records are bounded and do not retain skill contents, prompts, or
tool arguments.

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

Each resolved registration also has an immutable repository-scoped reference in
the form `ahu:agent:<uuid>`. The UUID identifies the registered name, version,
harness, model, source digest, and delivered-instructions digest together. A
reference is valid only in its repository's primary coordination store. When
the registration or either source changes, resolution fails as stale instead of
silently selecting a new target; native harness session identifiers remain
provider-owned and are not agent references.

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
`.opencode/skills/`, `.claude/skills/`, and `.agents/skills/`. The repository's
canonical skill tree and collision rules are described in
[MCP integration](#mcp-integration).

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
session's file tools act on whatever absolute path the model names. Headless attempts disclose reported targets outside the task worktree
when the evaluated event stream exposes them: the result envelope records
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
| A headless launch reports no terminal event | The bounded result metadata and harness-owned session history | Scores a run only on OpenCode's own terminal step, never on its exit status: a refused tool call ends the run with exit 0. |

In every one of these cases ahu reports and stops. It never silently selects
another model, provider, or harness, and a failed session does not remove the
task's branch, worktree, or record.

## What a task gets

Inspect and review a task from any checkout of the repository that launched it:

```sh
task_id=abc123  # replace with a task ID from ahu tasks
ahu task "$task_id" --output json
ahu diff "$task_id"
```

Both commands accept a full task ID or an unambiguous prefix. `ahu focus`
uses the newest record matching its prefix without checking ambiguity; use
a full ID when focusing a task. Inspection reads the saved record and headless
attempt metadata without updating state. Interactive inspection may query cmux
for observed ownership; headless inspection probes `owner.lock`. JSON schema
version 1 includes `task_id`, `agent`, `harness`, `model`, `branch`, `base_commit`,
`worktree`, `worktree_exists`, `record_path`, `cmux_workspace_id`, and
`cmux_window_id`. `session_state` is the recorded `starting`, `running`,
`exited`, or `failed` state; `state_source` is `record` and
`completion_verified` is always `false`. Headless records also include an
`attempt` projection with outcome, ownership observation, blockers, session
provenance, and review paths. Missing optional values are JSON null.
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
  lands inside the `agent` section, between `<ahu-agent-NONCE>` and
  `</ahu-agent-NONCE>` in delivery layout 3.

For a format with no frontmatter the two cover the same bytes and come out equal.
Both are folded into the agent's identity digest, so a change to either is
drift—and drift says which one moved, because an edit to frontmatter alone
changes the file without changing the delivered agent instructions.

The complete delivery has a separate integrity digest. Its layout, nonce, and
digest alone are not agent-version drift inputs. Drift compares agent identity,
source and instruction digests, repository configuration, project policy, and
known hooks.

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
ahu inventories Claude Code hook settings; general hook coverage for the other
harnesses remains incomplete. The separate cmux integration inspection recognizes
exact reviewed native components. Ordinary launch and inspection do not install
or edit hooks. The explicit [cmux installer](#cmux-native-integration) delegates
changes to the native command.

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

## cmux native integration

`ahu cmux status [--output json]` inspects bounded local configuration and native
artifact fingerprints without installing hooks or contacting a provider. It
reports CLI availability/version separately from socket reachability,
registration separately from activation, and isolation separately from live
conformance. `ahu doctor` gives compact summaries and an initial refusal reason;
the status command retains full provenance. Launch disclosures use the same
inspection. Verified absence is reported as missing. Unreadable, redirected,
unsupported, or indirect evidence stays unknown; a filename or matching fragment
of a command does not prove a reviewed component.

The native installer mapping is reviewed only for cmux
`0.64.22 (102) [ddd4a01bc]`. Other builds remain unknown. Preview the exact
operation and affected paths before explicit installation:

```sh
ahu cmux status --output json
ahu cmux install --harness codex --dry-run
```

Without `--dry-run`, ahu runs only the fixed native arguments shown below,
with inherited terminal streams, native confirmations, and native exit status.
It adds no `--yes`, shell evaluation, project scope, or bulk installation.
Native cmux owns its scripts, hooks, and activation/trust settings. Installation
success does not establish activation or live delivery; ahu inspects again afterward,
even after a failed native operation. Custom native configuration overrides make
the default-scope installer unavailable until separately reviewed.

| Harness ID | Native operation and scope | Inspected integration and headless admission |
| --- | --- | --- |
| `codex` | `cmux hooks codex install`; default user hooks, configuration, and cmux hook scripts | Exact reviewed commands check the hook-off variable and absent surface. Configuration activation is reported separately. Unknown commands or scopes refuse headless admission. |
| `claude-code` | No installer arguments; use cmux Settings > Automation | The reviewed bundled wrapper is recognized, but invocation/injection is unobserved. Headless requires the direct harness executable; separately configured hooks need their own evidence. |
| `opencode` | `cmux hooks opencode install`; default user Session and Feed plugins | The reviewed Session bytes check the hook-off variable and surface. Reviewed Feed bytes lack both and can use a fallback socket: installed Feed refuses headless coexistence. Reinstalling the same bytes does not fix isolation. |
| `antigravity` | `cmux hooks antigravity install`; default user Gemini hooks configuration | Configured commands have no fingerprint verifier and remain unknown, refusing headless admission. Custom formats and extension scopes remain unverified; CLI version eligibility is separate. |

Headless admission is checked before execution and rechecked by the supervisor.
The child environment removes inherited `CMUX_*` routing variables and restores
only a reviewed process-local control when the evidence requires one. Current
verified guards use `CMUX_CODEX_HOOKS_DISABLED` or
`CMUX_OPENCODE_HOOKS_DISABLED`. Claude’s reviewed wrapper requires a direct
executable; registered Claude and Antigravity hook commands remain unknown.
A variable name alone is not proof of isolation. Unsafe or unknown components
refuse admission. ahu leaves native homes and settings in place. Arbitrary shell
commands, hooks, remote plugins, and concurrent same-user changes remain outside
these controls. A successful static inspection does not launch or validate a
native session.

Native authentication stores, account databases, managed preferences, and plugin
sources can select additional executable configuration. ahu checks their presence
without reading credentials or fetching remote context. Present or unresolved
opaque sources refuse headless admission, even for a legitimate setup. Inspect
the reported source with `ahu cmux status`; interactive execution remains available.

### Conformance evidence

The local investigation baseline is cmux 0.64.22 build ddd4a01bc, Codex 0.155.1,
Claude Code 2.1.281, OpenCode 1.18.32, and Antigravity CLI 1.2.9. Installed versions
are observations, not additions to the admitted [headless profiles](#headless-execution).
In particular, Claude 2.1.281 and Antigravity 1.2.9 do not replace pinned profiles.

| Evidence class | What it establishes | What remains unverified |
| --- | --- | --- |
| Native artifact/source inspection | Exact command/plugin fingerprints, declared settings, guards, and installer arguments | Effective trust, actual hook activation, native event delivery, and unknown builds |
| Synthetic fixtures | Parser, refusal, environment filtering, and fixed-argv behavior under controlled inputs | Actual native harness behavior |
| Real PTY tests | Terminal ownership, stdin/signal handling, restoration, and cancellation with test processes | Four-harness terminal interface/session conformance |
| Live cmux plumbing tests | Group/workspace operations and focus behavior with harmless startup processes | Native hooks or four-harness provider sessions |
| Scoped native Codex hook no-op probes | Reviewed hook behavior under the probed hook-off/absent-surface conditions | A live Codex conversation or equivalence with the other harnesses |

The status report deliberately labels its evidence `locally_inspected;
live_untested`. Do not infer four-harness live conformance from installation,
static inspection, PTY coverage, or cmux plumbing.

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

Headless tasks use primary-owned coordination; interactive tasks use their own
worktree-local records. `tasks`, `task`, and `diff` include compatible legacy
records as well. `focus` is for interactive cmux sessions.

Each interactive task stores `task.json` and `prompt.txt` under
`<task-worktree>/.ahu/state/repos/<repo-identity>/tasks/<task-id>/`. Session status
is part of `task.json`. ahu derives this location from the worktree it creates;
state discovery uses explicit checkout paths. Removing the worktree removes
its state. Task checkouts are siblings under the primary checkout's `.worktrees/`,
including nested launches. State and worktree directories ignore themselves.

`tasks`, `task`, `diff`, and `focus` discover task records through those worktrees
from the primary checkout or a sibling. Each managed worktree store accepts only
its owner's task ID, matching repository identity and canonical worktree path.
This rule applies on every scan, including from inside that worktree; its store
is not scanned again as an unrestricted legacy store.

Compatible legacy records remain readable in the primary checkout and an
invoking plain checkout. A managed worktree's accepted record takes precedence
for a duplicate ID. Older nested child records
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

The primary checkout owns the launch lock, cmux group mapping, headless
coordination, and scoped task index. Hygiene timestamps and generated architecture
documents use the invoking checkout’s `.ahu/state/`. All locations derive from
explicit repository discovery, not ambient storage selectors. Ownership requires
positive Git verification, with filesystem checks before reusing process-local
evidence. Normal primary and linked checkouts are supported. Separate Git-directory
layouts are refused when Git cannot identify a verifiable primary checkout.

Execution admission, reconciliation, and cancellation stay within their owning
storage domain. Malformed records in unrelated legacy stores cannot fail a new
execution. Session checkpoint schema 2 binds native session evidence to task,
attempt, and harness. Invalid checkpoints remain unavailable; invalid ownership
metadata produces unknown `liveness`. Opened metadata files must have a single hard
link, the current user as owner, and safe permissions. New coordination records
and ownership locks require owner-only permissions.

Conventional old stores under `$HOME/.local/state/ahu/runtime` and
`$HOME/.local/state/ahu/task-index` remain lookup sources. Additional old roots
can be listed in `<primary-checkout>/.ahu/state/legacy-lookup.json`:

```json
{
  "schema_version": 1,
  "runtime_roots": ["/private/ahu-archive/runtime"],
  "index_roots": ["/private/ahu-archive/task-index"]
}
```

This file must be an owner-only regular file owned by the current user, without
hard links, at most 64 KiB. Unknown fields, unsupported schemas, or more than 32
combined roots are refused. Roots must be absolute, contain no `..`, remain
outside Git checkouts, and pass path checks. Missing roots are not created by
lookup. Old index entries remain read-only; new registrations and removals affect
the primary index. Lookup neither relocates old data nor enables unsupported
legacy execution. Explicit lifecycle commands are separate from read-only discovery.

Checkout state paths refuse existing symlinks at `.ahu`, `state`, and descendant
directories and files; state files must be regular files. Files are created
owner-only on Unix and replaced through temporary files. These checks refuse
static path redirection; they do not prevent a concurrent host process from
replacing paths between inspection and use.

At startup, ahu checks prompt and delivered-instruction digests, reconstructs the
launch command, and checks repository and worktree identity, including which task
worktree owns a local record. Records and their digests are mutable local files,
not authenticated evidence against a writer able to change both. Neither these
checks nor Git worktrees provide OS isolation or protection against concurrent
hostile host processes.

Interactive execution gives the child process group terminal foreground ownership
and restores the caller’s foreground afterward, including error paths. The
run-task owner holds a lock and supervises its child; cancellation uses an owned
process, not an unverified saved PID. A request before startup is honored.
Cancellation retains the task record, worktree, and branch. Only confirmed
cancellation closes the recorded cmux workspace. An already-terminal task, an
unconfirmed request, or a task that finishes on its own leaves the workspace open.
A workspace-close failure is reported separately from confirmed process
termination. A terminal outcome is recorded even if foreground restoration fails.

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

The selected canonical Git path is used for that Git invocation; cmux
keeps its selected canonical path across calls on the discovered client. `AHU_CMUX_BIN`
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
them. Headless admission uses its own explicit version profiles.
Project configuration pins catalog `2026-09-13`; a mismatch is an error.

Persisted task records use schema 3, whose IDs are hyphenated `UUID v7` values;
records written with schema 2 remain readable unchanged, keeping their 18-character
hex IDs, and older schemas are refused. Launch-preview and task-inspection JSON use
schema 1. Incompatible persisted records are refused and listed as unreadable;
ahu does not reinterpret their digests or delete their worktrees.

## Delegation and approval boundaries

Outside ahu, delegation belongs to the current harness. Registered assignments
inside ahu use registered ahu agents running their configured
harnesses and models in separate worktrees. Interactive tasks use cmux workspaces;
headless descendants inherit the execution mode and primary-owned coordination scope.
Headless child grants and bounded native helpers follow the preceding policies.
The supplied contract requires
`ahu agents`, a complete UTF-8 assignment file, and `ahu launch @name
--prompt-file assignment.txt`. `AHU_BIN` points to the launching ahu executable.

Specify the absolute source checkout and revision or diff scope when assigning
a review of uncommitted work: children start at HEAD and do not copy dirty source
files. Specify where findings belong and read them before reporting completion.

### Delivery layout

New deliveries use typed layout 3, ordered as `contract`, optional `metadata`,
optional `state`, optional `agent`, and `request`. Each section uses XML-shaped
opening and closing tags carrying the same per-launch nonce, such as
`<ahu-request-NONCE>` and `</ahu-request-NONCE>`. Bodies are raw text, not escaped
XML. The renderer preserves their bytes, including leading and trailing
newlines; a body without a final newline touches its closing tag. It refuses a
nonce collision in any rendered body.

Metadata carries the task ID, agent, harness, model, and permission mode. Available
state records root and parent task references, attempt number, a known native
session reference, and frozen child grants. These are minimal frozen execution
facts and references. They do not import native histories or transcript summaries.
The contract, agent instructions, and requester assignment retain distinct
sections, but all travel as prompt text in one argument. No adapter passes an
agent-selection or system-prompt flag; tags do not enforce authority.

Layout 3 keeps headless final reports in the native response, with no duplicate
`result.md` in coordination. Layout 2 retains its frozen contract bytes for
compatible inspection/replay; that does not admit legacy headless execution.

A record without `delivery.layout_version` replays layout 1 byte-for-byte,
including its bracket fences and unfenced request. Replay retains digest checks
and verifies available frozen composition against execution facts. Unknown
layouts are refused, requiring a new submission; ahu does not silently rewrite
saved prompts. Delivery layout and digest alone do not trigger agent-version
drift. The two source digests and configuration inputs remain distinct.

### Registered agent permissions

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

### Coordinator sessions

`ahu codex` opens Codex in the invoking checkout with
`--dangerously-bypass-approvals-and-sandbox`. `ahu claude` opens Claude Code with
`--dangerously-skip-permissions`. Each prints its flag before starting and keeps
the harness's configured model. These shortcuts explicitly request the harness
permission bypass; any outer sandbox still applies.

Coordinator shortcuts accept no additional arguments, preserve the invoking
directory and terminal, and set `AHU_BIN`. Inside cmux they place the invoking
workspace in the repository's group before starting the harness, reusing the
same group across linked checkouts and moving the workspace to its saved window
when needed. Grouping errors prevent harness startup. Outside cmux they run in
the current terminal. They create no task or worktree and need no project
configuration. ahu does not inject `AHU_STATE_DIR`;
subsequent commands discover checkout and primary coordination paths explicitly.
Registered child agents retain their own manifest permissions, harness, and
model. A coordinator's bypass does not alter a child's mapping or replace the
child's required approval-widening flag or headless host grant.

`ahu agy` opens Antigravity with `--dangerously-skip-permissions`, its unattended
mode, while retaining the configured model. `ahu opencode` opens OpenCode
without arguments, retaining native model and permission settings. OpenCode's
plugins retain their native loading behavior; ahu passes neither `--auto` nor
`--pure` here.

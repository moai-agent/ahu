# CLI and context reference

[Back to README](../README.md). Run `ahu help` for the concise command list,
`ahu help COMMAND` for focused options, or `ahu help all` for the full option
reference. The [MCP protocol details](mcp-protocol.md) describe tool contracts
and transport. Run `ahu explain` for the built-in architecture overview.

## Repository and task selection

`ahu --repo /path/to/project <command>` selects the checkout before parsing the
command; `--repo=/path/to/project` is equivalent. Command paths are relative to
that checkout. Without this prefix, ahu uses the current directory. Ambient
`AHU_REPO_ROOT`, `AHU_STATE_DIR`, `AHU_RUNTIME_DIR`, and `AHU_TASK_INDEX_DIR` do
not select storage. See [state and compatibility](#state-and-compatibility) for
explicit lookup of old stores.

Options such as `--prompt` and `--records` consume the next argument literally,
even if it begins with `--`. This permits flag-like prompts and filenames; shell quoting groups
arguments but does not distinguish an option from its value. Supply each value
before the next option. For example, `--prompt '--color=always'` is prompt text.

## CLI entry points

Run `ahu --help` for the concise public command list. `ahu doctor` gives a
concise readiness summary; `ahu doctor --verbose` shows component-level
details. It checks
repository and harness readiness, the committed context lock, bundled skill
bytes, telemetry configuration, registered-agent configuration drift, and account
binding readiness for each registered harness/model pair. Account checks display
status only, without principal values. Mismatched or missing bindings in an active
profile are launch blockers; unsupported or unavailable identity checks remain
explicitly uncertain. A general prerequisite check does not prove every execution
mode is available. For a
configured local telemetry endpoint, doctor reports TCP reachability only; it
does not verify OTLP delivery. `ahu agents` displays the registered agents, pinned
harnesses and models, manifest paths, and detected drift in a table. `ahu tasks`
lists the 20 most recent tasks by default. Use `--limit N` for a bounded count,
`--all` for the full history, or `--output json` for the complete machine-readable
list. It prints a task table laid out for the terminal; see [the task
list](#the-task-list).

To launch work directly, use a registered agent name followed by a quoted
positional prompt. With no prompt, `ahu @name` opens the interactive launcher
with that agent preselected. You can also pass a prompt file or the usual launch
options, including headless options:

```sh
ahu @reviewer 'Review the configuration validation.' --dry-run
ahu @reviewer --prompt-file assignment.txt --dry-run
ahu @reviewer --headless --prompt-file assignment.txt --output json
ahu @reviewer
```

The canonical `ahu @name` form accepts `--prompt`, `--prompt-file`, or piped stdin.
Do not combine a positional prompt with either prompt flag. Direct prompts are
passed as one literal
argument; quote multi-word prompts in the shell. `ahu lock` checks that the
recognized repository context matches committed `.agents/ahu/lock.md`; `ahu lock --update`
refreshes it for review. Every changed context input and the lock must be
tracked, clean, and committed before an agent can run. The bundled
`ahu-agent-context-critic` skill uses evaluations to test context hypotheses rather than
claiming that ahu can enumerate everything a harness loaded.

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
`ahu @name` selects a registered agent; `ahu task @name` selects a task.

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
including `task`, `focus`, `message`, `wait`, `result`, `cancel`, `approve`, `reject`, `resume`,
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
as `ahu @dev-astra` select a registered agent, while task-reference
positions such as `ahu task @storage-cleanup` select an immutable task handle.
The canonical task reference is `ahu:task:<uuid>`. Provider-owned native session
IDs remain labeled locators in inspection output; they do not become ahu task
references or grant permission to retrieve provider history.

## MCP integration

`ahu mcp serve` starts a local newline-delimited JSON request/response server on stdin and
stdout. Its repository inspection tools are read-only and scoped to the current checkout:
`ahu_agents_list`, `ahu_tasks_list`, and `ahu_task_get`. The read-only
`ahu_auth_budget` tool reports supported provider rate-limit windows for the
active auth profile, without returning identity values. The server reports
canonical task IDs and verified `@name` handles while using ahu's existing
repository ownership and task-resolution rules. Protocol handles describe
inspection operations and do not replace ahu task records or identities.

Agents may pass `minimum_remaining_percent` to `ahu_auth_budget` to filter its
registered-agent candidates against supported rate-limit windows. A candidate
is eligible only when every reported window meets the threshold; unknown
provider capacity stays unknown. This helps route work away from a constrained
known account, but does not compare token prices, confirm model availability,
or reserve shared capacity.

Tool-only clients can discover verified bundled skills with `ahu_skills_list`
and retrieve a manifest plus Markdown with `ahu_skills_get`. Both are read-only
and preserve the [local Skills privacy and provenance contract](mcp-protocol.md#tool-only-skill-compatibility).
Retrieval does not establish harness loading or execution.

The advisory `ahu_skills_suggest` MCP tool accepts a task and a lexical or
decision policy. It suggests committed skills without loading them. Decision
mode sends task text and skill names/descriptions to the configured provider.
`ahu eval run --skill-selection none|lexical|decision` compares the same policies
before candidate launch; see [the evaluation guide](evaluations.md).

The optional `ahu_typed_decide` tool uses TypeSafe Jev over HTTPS by default,
reading `TYPESAFE_API_KEY` from the MCP process
environment or the primary checkout's ignored `.env`. Setting
`AHU_DECISION_URL` explicitly selects a credential-free loopback decision
service instead. Decision calls are not persisted as ahu tasks; the tool
telemetry excludes their payloads and credentials. Jev receives the state and
questions, so review TypeSafe's data policy before sending sensitive content.
See [typed decisions](typed-decisions.md) for the provider mapping and secret
handling details.

`ahu_request_approval` is an explicit checkpoint for a task-bound operation.
It waits for an operator to run `ahu approve TASK` or `ahu reject TASK`; the
task is shown as `waiting-for-approval` while pending. Rejection or the
30-minute timeout requests cancellation. Approve and reject require an operator
invocation outside ahu task or worker context. Same-user code can remove these
environment markers or edit state; this is a cooperative check. This tool does not intercept
arbitrary shell operations, and harness write events remain reported evidence,
not OS-level filesystem containment. Approval summaries and targets are kept
in the task's owner-only state directory.

`ahu setup` is the only setup command. It detects installed supported harnesses,
asks for a model per detected harness, creates an ahu `dev-<harness>` agent for
each, installs bundled skills in project locations, and adds a local `ahu mcp
serve` entry to each harness's project configuration. Setup uses a harness's
available-model list when that command is supported, intersected with ahu's
reviewed catalog. Otherwise it offers catalog models and says account access
is not checked. Codex, OpenCode, and Antigravity use `.agents/skills/`; Claude
Code uses `.claude/skills/`. The
canonical bundle remains `.agents/skills/`; for Claude, setup creates ordinary
project-file copies. Setup checks the local MCP process with an
initialize/tools-list handshake, validates existing files before writing, and
refuses conflicting skill, agent, or MCP entries. It refreshes `.agents/ahu/lock.md` after
creating the project context. Review and commit the files and lock before
launching; setup does not stage, commit, or push them.

Antigravity setup retains `.agents/mcp_config.json` for project/print-mode
use and registers a native user-wide entry for interactive sessions at
`~/.gemini/config/mcp_config.json`. The global entry uses an absolute ahu
executable with `mcp serve` and inherits each session's working directory, so
it does not pin every session to one repository. Setup preserves other native
servers and settings and refuses conflicting ahu entries, malformed nonempty JSON,
unsafe paths, or inputs changed during planning. Empty or whitespace-only native
files are treated as an empty object while preserving their original bytes for
the change guard. It writes atomically with owner-only
permissions. Stop other native configuration writers while running setup.
Native user settings belong to private context acceptance, not the shared lock.
The CLI's `agy mcp list` lists user-wide entries; omission of a project server
does not prove print-mode discovery failed. The setup handshake verifies the
ahu server itself, not authenticated interactive client discovery.

Before task submission and execution, ahu checks OpenCode's exact selected
model against `opencode models` in the applicable checkout. A missing
model, failed listing, or timeout refuses the run without selecting another
model. The [native listing](https://opencode.ai/docs/cli/#models) establishes
configured availability, not account entitlement or successful inference.
Headless probes follow isolation admission. Native listing commands may migrate
configuration; if inspection changes recognized project context, ahu refuses the
launch until you review, accept, and commit that change.

Codex auto preflight loads native configuration with the same local-tool
approval overrides using `codex mcp list`, without connecting to MCP servers;
all output is discarded. An untrusted project's `.codex/config.toml` is ignored,
which can leave tool overrides without an ahu transport. The diagnostic asks
the operator to review project trust and native MCP configuration; ahu does
not grant trust, promote a project command into CLI overrides, or drop approval
gates. Trusted project-only transports are accepted; global registration is
not required. Codex's [configuration reference](https://developers.openai.com/codex/config-reference)
describes project trust and the separate per-tool MCP approval settings.

Harness CLI versions float to the version installed on each machine by default;
ahu does not install or update harnesses. To require an exact version for a
project, add a pin to `.agents/ahu/config.md`:

```yaml
ahu:
  # Keep the other existing ahu settings.
  harness_version_pins:
    codex: "0.160.0"
    claude-code: "2.1.288"
```

Omit a harness from this table to follow its installed version. Pins are shared
project policy and are covered by `.agents/ahu/lock.md`; each task records the version it
actually used. The catalog's verified versions describe where behavior has been
checked, rather than an allowlist. Floating permits newer CLI versions with a readable semantic version,
but does not claim that every upstream change is compatible. ahu checks
headless isolation against the installed version before execution and resume;
unknown native behavior or an invocation that no longer works can still prevent
the task from starting. Use a pin when a project needs repeatable CLI behavior.

The MCP entry uses the `ahu` executable from `PATH`. Claude Code asks for
approval before using a project MCP server. Codex reads project MCP settings
only when the repository is trusted. Harness-level confirmation is still
needed to establish that the client connected and exposed its tools. Skill
presence does not prove that a particular run loaded the skill. See
[skill verification](skill-verification.md) for per-harness discovery evidence.

For Codex agents declaring `permissions = auto`, ahu approves its local agent
and task lookup tools and `ahu_request_approval` for that launch. The latter
only records a bounded checkpoint and pauses; it cannot perform the requested
action. Provider-backed `ahu_typed_decide` and `ahu_skills_suggest` calls stay
gated because their inputs may leave the machine. Before release, verify
each supported harness in both interactive and headless mode with a real ahu
tool call, and confirm an `ahu.mcp.tool.call` span reached the configured local
OTLP receiver. An initialize/tools-list handshake alone is not proof of use;
approval denials can happen before ahu receives a call.

Approval-free execution does not grant unrestricted filesystem access. Codex
agents retain the native `workspace-write` sandbox; protected context directories
and shared worktree Git metadata can remain read-only. When a task authorizes
an edit or commit but the native boundary refuses it, the agent can provide a
patch outside the repository for the authorized coordinator to review, apply,
validate, and commit. ahu does not widen that boundary automatically.

The canonical tree follows a strict contract: one directory per skill directly
under `.agents/skills/`, named after the skill, holding exactly one `SKILL.md`.
The frontmatter carries `name` (equal to the directory name) and `description`
(a one-line summary). `scripts/check-skills.py` verifies the tree shape,
frontmatter, unique names, and that the compiled user-facing skill bundle in
`src/mcp.rs` matches the source files; CI runs it on every change.

When the same skill name appears in more than one discovered location, each
harness resolves the collision according to its own discovery precedence. ahu
does not control that choice. Claude's `.claude/skills/` copies are ordinary
project files created by setup and are fingerprinted by `.agents/ahu/lock.md`. Skill
presence does not prove that a run loaded it; user, managed, and provider
context remains outside the repository lock.

See the [MCP protocol reference](mcp-protocol.md) for protocol negotiation,
tool validation, task delivery, and transport details.

### Long-running task records

Follow the repository's tracking policy for long-running or failure-prone work.
It may require an issue, task, milestone item, or another provider record; it
may choose to use ahu's task record alone. ahu records execution state, not
acceptance criteria or roadmap status, and it has no built-in issue-tracker
integration. The bundled `ahu-direct-agents` skill describes how to keep tracker
work optional or link a provider-side work item to an ahu task ID without
assuming GitHub or a private roadmap.

When a provider record is used, its skills and tools define lifecycle fields,
comments, visibility checks, and closure. A task process exiting successfully
is not sufficient to close a record: inspect its report, diff, validation, and
delivery state first. Keep private tracker content out of this public
repository and its knowledge base. Before writing, verify the provider's record,
project, and linked-object visibility; a private project does not necessarily
make a linked public record private.

## Scriptable launch previews

A launch prompt can come from an inline argument, a UTF-8 file, or piped stdin:

```sh
ahu @offsec-astra 'Review the subprocess argument handling.' --dry-run --allow-widened-approvals
printf '%s\n' 'Review the subprocess argument handling.' > assignment.txt
ahu @offsec-astra --prompt-file assignment.txt --dry-run --allow-widened-approvals
printf '%s\n' 'Review the subprocess argument handling.' | ahu @offsec-astra --dry-run --allow-widened-approvals
ahu @offsec-astra 'Review the subprocess argument handling.' --dry-run --allow-widened-approvals --output json
```

The positional prompt, `--prompt`, and `--prompt-file` are mutually exclusive.
An explicit source takes precedence over stdin, which is read only when neither
flag is given and stdin is not a terminal. Empty or whitespace-only prompts are
rejected. Accepted prompt text retains its original bytes, including leading
and trailing newlines.

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

## Harness compatibility by execution mode

All five harnesses have interactive and headless adapters. The
[catalog](../src/catalog.rs), [interactive adapters](../src/harness/mod.rs),
and [batch command builder](../src/headless.rs) describe different evidence
and controls for those modes. The catalog's recorded versions are:

| Harness | Interactive evidence | Headless evidence | Limits and refusals |
| --- | --- | --- | --- |
| Codex | 0.154.0, 0.157.1, 0.160.0 | 0.154.0, 0.155.1, 0.157.1, 0.160.0 | Fresh runs and native resume have separate argument mappings. Headless `accept-edits` resume is refused. Versions 0.157.1 and 0.160.0 require native metadata inspection. |
| Claude Code | 2.1.269, 2.1.283, 2.1.288 | 2.1.269, 2.1.270, 2.1.283 | Hook-control profiles also exist for 2.1.288, 2.1.289, and 2.1.302; these do not extend authenticated headless evidence. Bounded native helpers require 2.1.270. |
| Antigravity CLI | 1.2.2, 1.2.16, 1.4.2 | 1.2.2, 1.2.16 | The 1.4.2 record covers help, model discovery, and interactive startup; headless behavior on that version remains unverified. Unresolved hooks/extensions refuse headless admission. |
| OpenCode | 1.18.29, 1.18.30, 1.18.31, 1.18.32, 1.18.34, 2.0.32 | 1.18.29, 1.18.30, 1.18.31, 1.18.32 | The 2.0.32 record covers `mini` help and interactive startup with `prompt` permissions. Interactive `auto` is refused on that interface; `accept-edits` is refused in both modes. Headless 2.0.32 remains unverified. |
| Kiro CLI V3 | 2.28.0 | 2.28.0, 2.29.0 | The 2.29.0 catalog entry covers headless and native configuration, not interactive verification. Headless configuration inspection refuses versions other than these two. `accept-edits` and bounded native helpers are refused. |

These records are scoped observations, not proof of every permission mode,
tool, skill, model entitlement, or integration. In particular, interactive
startup does not establish task completion. A raw harness probe does not
establish admission through ahu. See the [skill verification notes](skill-verification.md)
for separate discovery and invocation evidence, and the
[conformance evidence](#conformance-evidence) for inspection and fixture limits.
Configuration alone establishes neither a live MCP call nor OTel export;
record actual tool outcomes and collector observations separately for the
exact version, model, and execution mode tested.

### Local invocation and completion evidence

The 2026-10-19 documentation audit checked installed CLI versions/help and the
current adapters. Three ahu-assigned Codex audit tasks executed local shell and
file tools and produced changes, but were stopped before returning terminal
reports; this is not evidence of successful task completion. Catalog observations
from other versions remain separate. Help confirms option declarations, not
admission, authentication, tool execution, or task acceptance. "Not observed"
means this audit did not establish the result, not that the capability is absent.

| Harness and mode | Locally checked invocation | Task/tool completion evidence | Approval/autonomy mapping | OTel export evidence |
| --- | --- | --- | --- | --- |
| Claude Code interactive | 2.1.302 `claude --help`; no session launched | No current-version task or tool completion observed | `prompt` adds no approval flag; `accept-edits` uses `--permission-mode acceptEdits`; `auto` uses `--permission-mode auto` | Not observed |
| Claude Code headless | 2.1.302 help advertises print, stream JSON, and permission controls; synthetic invocation profile only | No authenticated 2.1.302 completion observed; the catalog records prompt completion/usage on 2.1.283 | Same permission modes, plus `--permission-prompts none`; non-managed hooks turned off by the exact-version profile | Not observed |
| Codex interactive | 0.163.0 help and three ahu audit task sessions with `gpt-6-astra` | Instructions were received and local shell/file tools ran, but the sessions were stopped before a terminal task result | `auto` passes `--ask-for-approval never --sandbox workspace-write`; `accept-edits` uses `--approve-for-me`; `prompt` retains native policy | Collector receipt not inspected |
| Codex headless | 0.163.0 `codex exec --help`; no batch session launched | No 0.163.0 batch completion observed; 0.160.0 has recorded live MCP decision/answer completion | `approval_policy="never"`; `prompt` uses read-only, `auto` workspace-write; fresh `accept-edits` uses `--approve-for-me`, resume refuses it | Not observed on 0.163.0; catalog records collector export on 0.160.0 |
| Antigravity interactive | 1.4.2 `agy --help`; catalog separately records startup on 1.4.2 | Startup evidence does not establish a completed task or successful tool call | `prompt` retains native policy; `accept-edits` uses `--mode accept-edits`; `auto` uses `--dangerously-skip-permissions` | Not observed |
| Antigravity headless | 1.4.2 help advertises `--print`, stream JSON, and print timeout; no batch session launched | 1.4.2 task/tool completion remains unverified | Same permission flags; unresolved hooks/extensions can refuse admission | Not observed |
| OpenCode interactive | Coordinator inspected installed 2.0.32 `mini --help` and its task-directory routine; no session launched | No fresh completion observed; prior 2.0.32 startup uses `prompt` | 2.x `mini` refuses `auto`; both modes refuse `accept-edits`; `prompt` retains native policy | Not observed |
| OpenCode headless | Coordinator inspected installed 2.0.32 `run --help`; argument and working-directory selection covered by fixtures | No fresh batch completion observed; catalog batch evidence remains on 1.x | `auto` adds `--auto`; 2.x uses `--standalone`, with task directory pinned through the working directory and `PWD`; native-state admission remains separate | Not observed |
| Kiro interactive | 2.32.0 version and `chat --v3 --help`; no session launched | No 2.32.0 task/tool completion observed; catalog interactive evidence remains on 2.28.0 | `prompt` retains native policy; `auto` adds `--trust-all-tools`; `accept-edits` refused | Not observed |
| Kiro headless | 2.32.0 help advertises batch flags, but this checkout's configuration inspection admits only 2.28.0/2.29.0 | No 2.32.0 task/tool completion observed; help does not override the version refusal | `--no-interactive --output-format stream-json --require-mcp-startup`; same permission mapping, subject to admission | Not observed; native Kiro export is separate from ahu/MCP attribution |

These mappings describe ahu's arguments, not the effective permissions of every
tool. Native settings still apply. Manifest-requested widening requires ahu's
explicit opt-in; it does not resolve headless isolation refusals. A successful
shell command does not prove MCP access, and neither proves collector receipt.
The [release telemetry guide](otel-release-report.md) describes separate
task/attempt-attributed export checks. Adapter and loopback transport fixtures
exercise synthetic events, not authenticated provider completion.

Readiness and budget observations are local and time-sensitive, so this public
reference does not publish account identities, binding states, or capacity
figures. Use `ahu auth readiness` and `ahu auth budget --output json` for current
secret-free status. Unsupported capacity signals remain unknown; these checks
do not establish model entitlement or live task success. No native account,
configuration, or approval settings were changed to obtain the observations in
this audit.

Git worktrees provide a branch and working directory, not OS confinement.
Approval settings and headless native-configuration checks are separate from
filesystem, process, and network restrictions. Follow the
[agent Git workflow](agent-git-workflow.md) for worktree limits and release
integration: task commits go through coordinator review into the release
branch before the validated candidate is merged to `main`.

## Headless execution

`ahu @agent --headless` runs the selected harness in batch mode without cmux, a PTY,
or screen scraping. `--background` detaches the supervisor after startup
acknowledgement; without it, the caller supervises in the foreground. Use the
foreground form under CI or a host service that manages process lifetime.
A dry run performs preflight checks and prints the redacted command, frozen
identity, capabilities, gaps, timeout, and coordination path without launching.
Known cmux wrappers are refused; use the actual harness executable on `PATH`.

Installed CLI versions float by default; `[harness_version_pins]` can require
an exact version. The [recorded batch versions](#harness-compatibility-by-execution-mode)
are evidence records, not an allowlist. A supported adapter and a readable semantic version are
required; version-specific isolation controls and native-helper policies remain
separate checks. Unknown native integrations can still prevent launch or resume.
No failure selects a fallback harness or model.

Interactive compatibility is tracked separately: ahu inspects the installed
OpenCode CLI and uses `opencode mini` on 2.0.32 to keep the session persistent
with the exact model and prompt. That version is not included among the
validated OpenCode batch versions above.

OpenCode's batch form uses `opencode run --format json --model <provider/model>`.
The 1.x adapter adds `--dir <task-worktree>`. On 2.x it omits the removed `--dir`
option and adds `--standalone` to request a private server. OpenCode task launches
set both the process working directory and `PWD` to the task worktree because
2.x prefers `PWD` in both `mini` and `run`. This argument mapping does not establish
live 2.x batch compatibility. Batch `auto` adds `--auto`; `permissions = "accept-edits"` is
refused. These checks are separate from interactive `mini` admission. See
[OpenCode with Ollama-hosted models](#opencode-with-ollama-hosted-models) for
provider setup. Recorded compatibility does not establish authentication,
provider availability, or full native-helper lifecycle validation.

### Trusting native OpenCode state

OpenCode can keep provider credentials and account-managed configuration in
native state outside the project. ahu does not read those credentials or account
database contents. By default, an opaque OpenCode account/authentication store
blocks headless admission because it may select remote configuration, including
executable plugins. A project that intentionally relies on that native setup can
make the trust decision explicit in `.agents/ahu/config.md`:

```yaml
ahu:
  headless:
    trust_opencode_native_state: true
```

This applies only to headless OpenCode tasks. ahu records the trust in the
frozen task controls and discloses that the store may activate remote
configuration and plugins. ahu does not inspect, disable, or constrain them;
review that native state through OpenCode. The setting does not bypass other
unresolved integration findings, approval policy, or harness checks.

Codex 0.157.1 and 0.160.0 use strict configuration validation, enable inspected hooks, turn off
optional plugins and remote plugin loading, and clear the legacy notify command
for this invocation. Before launch and resume, ahu asks a separate native metadata
process for effective hooks and managed requirements without starting a model
turn. Inspection has a 30-second deadline and bounded output. ahu refuses
admission for warnings, managed requirements, or hooks outside reviewed profiles. Authentication remains
native. This inspection cannot prevent configuration changes between inspection
and execution. A trusted project and explicit tool approvals are still required
for unattended MCP calls.

Claude Code 2.1.283, 2.1.288, 2.1.289 and 2.1.302 receive
`--settings '{"disableAllHooks":true}'` for headless launch and resume. This
invocation setting disables non-managed hooks without replacing native settings,
account selection, skills, or direct MCP configuration. Parsed hooks can be
isolated this way; malformed settings, enabled plugins, and unresolved managed
sources still refuse admission. Managed hooks retain their native precedence.
See Claude's [hook controls](https://code.claude.com/docs/en/hooks#disable-or-remove-hooks).

The 0.160.0 Codex and 2.1.288 Claude invocation controls were checked with local
synthetic configuration and model transport fixtures. Those checks do not extend
Claude's authenticated headless verification records. Codex 0.160.0 also
completed live headless decision calls and answer generation with observed MCP
telemetry during the 0.8.0 evaluation pilots.

The 2.1.289 Claude invocation profile was separately checked against the installed
native help, vendor hook controls, and a loopback model fixture: user, project
and local SessionStart, UserPromptSubmit and Stop hooks ran in the control case
and were suppressed by the invocation setting. The exact requested model and a
direct user MCP server remained available. These synthetic checks do not extend
the authenticated headless verification records.

The 2.1.302 profile is based on installed CLI help advertising `--settings`,
the vendor's per-run hook control documentation, and synthetic admission and
launch/resume argument tests. No authenticated headless task or native hook
execution was observed for this version; its live verification records remain
unchanged. The profile applies only to that exact version, with the same
managed-source and plugin refusals.

OpenCode's `--pure` flag disables external plugins, but does not establish the
contents of authenticated remote configuration. Native configuration can also
select providers and models; replacing a configuration or data home, hiding an account
database, or stripping authentication plugins can change the selected identity.
ahu does not apply those workarounds. Resolve the reported source with its native
owner or use interactive execution; see OpenCode's
[configuration precedence](https://opencode.ai/docs/config/#precedence-order).
Claude's `--bare` similarly changes authentication resolution and is not used.
Antigravity has no reviewed invocation override for unresolved hooks/extensions.

`ahu cmux status` (also `--output json`) checks installed CLI versions and
reports the same isolation profile used by launch previews. Inspection alone
without a version observation reports component compatibility only. An admitted
version and inspected components do not replace executable, approval, model, or
other launch checks.

| Harness | Required native isolation evidence |
| --- | --- |
| Codex | Absent sources or exact reviewed hooks guarded against permission expansion. Versions 0.157.1 and 0.160.0 additionally require native effective metadata inspection with no warnings or managed requirements, and turn off optional plugins for the invocation. Other versions retain unresolved plugin, cloud, and managed-source checks. |
| OpenCode | Absent sources or the reviewed guarded Session plugin. Feed is unsafe; declared modules and substitutions remain unresolved. Opaque authentication/account stores refuse admission unless the project explicitly trusts native state as described above. |
| Claude Code | Direct executable avoids the cmux wrapper. Versions 2.1.283, 2.1.288, 2.1.289 and 2.1.302 turn off parsed non-managed hooks for the invocation. Enabled plugins, malformed settings, and unresolved managed sources still refuse admission. |
| Antigravity | Absent inspected hooks. Custom hooks, extensions, and overrides remain unverified. |
| Kiro | V3 2.28.0/2.29.0 and the inspected subset of native agents and CLI preferences. Unknown formats/fields, nonempty hooks, extensions, Powers, workflows, cloud configuration, and environment indirection are refused. See [Kiro CLI](#kiro-cli). |

Apart from the explicit OpenCode native-state trust setting, unknown or unsafe
integrations have no operator bypass, including with
`--allow-widened-approvals`. Interactive execution uses the current terminal or
an explicitly selected session host and retains normal approval and launch checks.
ahu does not remove credentials, rewrite native settings, or treat interactive
availability as headless isolation. New compatibility claims require validation
before the catalog records a version as verified. Evidence is bounded local
inspection; live delivery, provider availability, and sandbox behavior are not implied.

```sh
ahu @dev-astra --headless --background --timeout 1800 \
  --prompt-file assignment.txt --allow-widened-approvals --output json
ahu tasks --output json
task_id=abc123  # replace with the returned task ID
ahu task "$task_id" --output json
ahu wait "$task_id" --output json
ahu result "$task_id" --output json
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
`timed_out`, `cancelled`, `boundary_violation`, `capture_failed`, `supervisor_error`, and `interrupted`. New result envelopes use schema 2 and separate process exit, bounded harness
outcome metadata, native references, and worktree identity. Native transcript,
final-answer, stderr, and helper-summary text is not copied into the envelope.
Schema 2 also records `writes_outside_worktree`: absolute target paths reported
by recognized write tools in the harness's stdout events that resolve outside
the task worktree. The supervisor checks these observations while the process
runs. On detection it requests cancellation of the task and its recorded ahu
descendants, stops the process group, and records `boundary_violation`.
`ahu task` and `ahu result` disclose the paths for review.

This is detection after an event arrives, not filesystem isolation or a
pre-write approval gate. A reported target is not proof a write succeeded;
the write may already have occurred. Relative paths, arbitrary shell writes,
unrecognized tools, and missing events can escape detection. The final stream
drain is checked before accepting a successful result; observed outside targets
also cancel registered descendants. Review the reported paths and native evidence.
`acceptance` stays `not assessed` and `completion_verified` stays false: a provider
success or an agent's report does not establish that the assignment was accepted.
Treat native reports and same-user editable metadata as untrusted data.

Headless result views separate `root_completion` from `native_completeness`.
The former reports the root harness terminal event and process exit; the latter
reports observed helper lifecycle coverage. A missing helper terminal event on
a harness whose helper stream is unsupported leaves helper completion unknown;
it does not mean the root result was missing or prove that no helpers ran.
For `supervisor_error`, ahu shows the bounded failure phase/code and recommends
preserving the attempt, reviewing its worktree diff, and submitting a new
registered assignment only after checking whether native work ran. It does not
automatically resume or clean an uncertain attempt.

Human-readable `tasks` summarizes the headless attempt number and outcome on
its own line under each row, so a headless task is identifiable even in a
terminal too narrow for the `MODE` column. `task` and `result` add observed supervisor
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
configuration and executable, and an available adapter mapping. When local auth
profiles are configured, it also rechecks the active account and the task's
original account/profile pin; see [account bindings](#local-harness-account-bindings).
Codex
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
ahu @coordinator --headless --background --allow-child @reviewer \
  --prompt-file assignment.txt --output json
```

If either manifest requests wider approvals, supply the corresponding parent
`--allow-widened-approvals` or child `--allow-child-widened @reviewer` flag.
Inside the owning task, request the child through the launching executable:

```sh
"$AHU_BIN" @reviewer --headless --background \
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
| Claude Code 2.1.283 | Admitted; invocation turns hooks off | Refused: no validated bounded-helper profile |
| Codex 0.154.0/0.155.1/0.157.1 | Admitted | Refused: incomplete helper identity/join event visibility |
| Antigravity CLI 1.2.2 | Admitted | Refused: unvalidated native profile |
| OpenCode 1.18.29/1.18.30/1.18.31/1.18.32 | Admitted | Refused: no validated native tool switch |

The Claude bounded profile restricts the **entire attempt, including the owner**,
to the model tools `Read`, `Grep`, `Glob`, and the parent's `Task` delegation tool.
Through those tools it cannot edit,
run shell commands, builds or tests, create native worktrees/teams, or shell-launch
registered ahu children. Use it for read-only review or investigation:

```sh
ahu @reviewer --headless --native-helpers bounded \
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

Add the optional section to `.agents/ahu/config.md`, preserving existing policy:

```yaml
ahu:
  # Keep the other existing ahu settings.
  knowledge:
    bundles: ["docs/knowledge"]
    fail_on_warnings: false
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

## Local traces

For local numeric headless attempt metrics without an exporter, set
`local_metrics: true` in `ahu.telemetry` and leave `enabled: false`.
Both options default to false and operate independently. Headless attempt results
then include a `metrics` object with `schema_version = 2`, six normalized
`ahu.tokens.*` fields under `values`, and
`token_aggregation = "maximum-reported-per-field"`. Each value has
`kind = "observed"` with an unsigned integer `value`, or
`kind = "unavailable"` without a value. Zero is an observation, not missing data.
Schema version 2 also includes `ahu.cost.harness_reported_usd`, as
`kind = "observed_float"` when the harness reported a finite non-negative amount,
or `kind = "unavailable"` otherwise. The opt-in projection also records
`elapsed_ms` as an observed run duration or unavailable. The USD field is a
harness-reported amount, not a final provider bill. ahu does not derive prices
from token counts. Token
maxima are not additive task totals; the USD field follows the harness's reported
session or step accounting described below.
This option controls the new projection; it does not change existing
`harness.usage` collection or retention.

The containing result's existing task ID and attempt identify the observation.
The metrics object accepts no issue references, free text, paths, account data,
or arbitrary attributes. This object is not sent to child environments. The
complete task result still contains existing coordination metadata and is not
a safe export format. Interactive sessions do not produce this headless
projection.

Associate headless task attempts with a private work item using an opaque local
key:

```sh
ahu telemetry link --key record-a4 --task TASK
ahu telemetry report --key record-a4
ahu telemetry report --key record-a4 --output json
ahu telemetry unlink --key record-a4 --task TASK
ahu telemetry unlink --key record-a4
```

The key and explicit task UUID membership live in an owner-only host state
directory (`$XDG_STATE_HOME/ahu/private-measurements/` or
`$HOME/.local/state/ahu/private-measurements/`), outside checkouts, task
records, and project configuration. A key is limited to ASCII letters, digits,
underscores, and hyphens; ahu does not contact the tracker or validate its
visibility. Keep it opaque and use a private tracker record only after checking
the record and project visibility yourself. The command does not write to the
tracker.

Links survive task retries and resumes because attempts retain their task UUID.
Child tasks have separate UUIDs and must be linked explicitly. A Git merge does
not change task identity. `ahu remove` deletes task evidence but does not remove
the private mapping; reports mark missing task records. Remove links with
`ahu telemetry unlink` when the tracker item is deleted or when its retention
period ends. `ahu cleanup` preserves task results and does not change links.

Reports require `telemetry.local_metrics = true` for the current project and
include persisted headless attempt results. They group by agent identity,
harness, model, and outcome. Each group includes elapsed process time, per-field
maximum token observations, observation coverage, and harness-reported USD
separated by source. Retry/token values are not summed, and child task values
are not implicitly added to a parent. Attempts without an opted-in projection
remain in denominators as unavailable; tasks without a completed result and
removed task records are disclosed separately. Each group also reports complete,
incomplete, and unknown native-event evidence separately from process outcome;
missing evidence is never counted as complete. The report is printed locally;
JSON output contains the opaque key, so do not send it to a public log or
telemetry destination.

### Private association boundary

`ahu telemetry` stores associations in an owner-only host directory outside
checkouts. `telemetry::private::PrivateMapping` validates the bounded mapping
schema and summarizes numeric input. Neither component has a tracker client or
an OTLP export connection. The repository's checkout-local state is not used
for these associations.

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
The summary includes validated repository and optional agent digests, harness,
model, and outcome to identify a homogeneous report group. Mixed groups are
rejected instead of pooled. Timing reports mean observed elapsed milliseconds
and coverage. Harness-reported cost reports mean USD and coverage separately for
each source; Claude Code and OpenCode amounts are never combined. Missing token
totals are never inferred and token values are never summed: resumed sessions
may repeat cumulative usage, and parent usage may overlap child usage. These are
coverage statistics over supplied observations, not complete task totals or
billing. Absent results, collection set to off, and undiscovered attempts are not
invented as observations. The CLI validates task ownership and reads only
opted-in numeric projections; the library primitive does not read result
envelopes or prove completeness. No provider calls are involved.

Keep the opaque key out of task records, worktree names, prompts, MCP
responses, shared configuration, OTLP attributes, and diagnostics. Store files
are outside checkouts, owner-only, symlink-resistant, locked during updates, and
written atomically. Bind through validated repository/task identity rather than
caller-supplied paths; repository moves require explicit rebinding.
Unlink associations explicitly; their retention is independent of task cleanup.
Reads and writes are bounded. ahu does not verify tracker visibility, fetch
tracker content, inherit child task links, or promise same-user process
isolation. Tracker access and private-key retention remain the responsibility of the maintainer.

### Local OTLP export

OTLP export is off unless the project opts in. Eval runs are the exception: they
enable telemetry regardless of project settings, pointed at the eval receiver
endpoint. When enabled, ahu exports its own launch and harness lifecycle traces
to the project-configured local OTLP collector and injects the same endpoint
into harness processes started by ahu, which then export their own traces,
metrics, and logs. It never changes the
invoking shell or harness sessions started directly. Child traces, metrics,
and logs all select the `otlp` exporter; inherited OTLP headers,
signal-specific endpoints, and compression settings are cleared for the
child. Exporter construction or delivery failure
does not fail the assignment. Spans are queued asynchronously in a bounded
128-span buffer and exported in batches of at most 32 every 500 ms. A full
queue drops telemetry instead of blocking a task; each OTLP/HTTP request has a
500 ms timeout. These bounds apply to ahu's exporter and do not configure the
collector's own queue or downstream exporters. Child SDK batch queues and
metric intervals remain controlled by the harness and its inherited settings;
ahu sets their OTLP request timeout
but does not impose its own span queue limits on them. An earlier panic while
telemetry holds its internal lock can still terminate the process; the
delivery-failure guarantee does not cover that case.

```yaml
ahu:
  # Keep the other existing ahu settings.
  telemetry:
    enabled: true
    endpoint: "http://127.0.0.1:4318"
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
issue content to its span attributes. When ahu configures child export, inherited
`OTEL_RESOURCE_ATTRIBUTES` are filtered to bounded, allowlisted `ahu.*` identity
fields before launch identity is added. Other inherited resource fields are
dropped. MCP correlation also uses `AHU_MCP_RESOURCE_ATTRIBUTES`, containing only
bounded, allowlisted ahu identity fields, because a harness may strip `OTEL_*`
variables from its MCP children. Codex setup forwards that variable and the eval
receiver endpoint by name, alongside decision-provider configuration; it does not
copy secret values. The exporter SDK can read ambient OpenTelemetry configuration.
Keep sensitive data out of that configuration; these settings are not a
redaction boundary.

#### Release attribution and accounting queries

For new assignments, supply an opaque release label through standard OTel
resource configuration:

```sh
OTEL_RESOURCE_ATTRIBUTES='ahu.release.id=release-candidate-a' \
  ahu @dev-astra --prompt-file /tmp/release-task.txt
```

This example assumes that agent is registered and the assignment file exists.
Release labels accept 1–64 ASCII letters, digits, dots, underscores or hyphens.
Use a public-safe label, never a private tracker key, URL or title. The label is
frozen in the existing task identity. Resumes keep it; broker children inherit
their parent's frozen value, including absence. A later shell variable cannot
assign an old task to a release. No separate release mapping is required.

Task run spans and their ahu-owned resources carry `ahu.release.id` when set,
`ahu.task.id`, `ahu.task.attempt`, and `ahu.execution.mode`, plus known
`ahu.task.root_id` and `ahu.task.parent_id`. MCP children receive matching
identity. Harnesses control whether their native exporters honor these fields;
native Kiro exports do not currently establish this task/attempt correlation.

Headless `ahu.harness.run` spans describe observed usage with explicit semantics:

| Fields | Meaning |
| --- | --- |
| `ahu.tokens.*`, `ahu.tokens.aggregation` | Maximum reported value per token field; not additive call usage |
| `ahu.cost.harness_reported_usd`, `ahu.cost.source`, `ahu.cost.aggregation` | Claude's maximum valid result total or OpenCode's sum of first valid costs per distinct part ID |
| `ahu.kiro.credits.observed`, `ahu.kiro.credits.unit` | Accepted, deduplicated Kiro prompt-completion observations in credits |
| `ahu.kiro.elapsed.observed_ms`, `ahu.kiro.context.*_percent` | Reported completion elapsed time and context-window usage; neither is billed tokens |
| `ahu.tokens.coverage`, `ahu.cost.coverage`, `ahu.kiro.credits.coverage` | `partial` when measured, `unavailable` otherwise; an observed zero remains present |
| `ahu.events.coverage` | Completeness of recognized native event evidence, independent of accounting coverage |
| `ahu.accounting.cross_attempt_additivity`, `ahu.accounting.parent_child_additivity` | `not_established`; resumed totals or nested usage may overlap |

Query the telemetry backend for `ahu.harness.run` spans grouped by release,
harness, model, mode and outcome. Deduplicate OTLP retransmissions by trace/span
identity. Include failed and cancelled attempts when examining release effort;
successful process exit does not establish acceptance or full cost coverage.
Show the number of received attempts alongside observed and unavailable values.
Do not add cumulative attempt observations, native per-request metrics and
parent/child observations together without establishing their disjoint scope.
Kiro credits remain separate from USD. These attempt observations are custom
`ahu.*` attributes because they do not satisfy per-request GenAI token metric
semantics.

Enable collection before starting release work and verify receipt in the
backend. A listening collector port is not delivery evidence. Collector queues,
export failures, sampling and retention affect coverage; keep the evidence used
for accounting unsampled and monitor loss. ahu's bounded exporter can drop spans,
and an abrupt process exit can leave no completed run span. Interactive usage,
unobserved tasks and provider billing remain separate coverage gaps. The release
label supports backend attribution; it does not certify a total release cost.

Child harness processes receive the same loopback OTLP endpoint and export
their own metrics and logs. ahu's attribute allowlist and redaction apply only
to the attributes ahu itself emits; the child harness and its SDK control their
own log and metric payloads, which may include unredacted metadata such as
resource attributes the harness copies from its environment. This is why the
endpoint is restricted to a local collector: the collector, not ahu, becomes
the redaction and retention boundary for child-emitted telemetry.

The separately launched `ahu mcp serve` process also initializes the same
project-configured local exporter. It emits a span for every parsed inbound MCP
request, including initialization, discovery, tool listing, tool calls, task
operations, subscriptions, notifications, and protocol errors. `tools/list`
spans report the count and names of ahu tools offered; `tools/call` spans use
the `ahu.mcp.tool.call` name and record bounded tool name and success/error
outcome. Typed-decision spans additionally report question count and types,
stable `telemetry_key` dimensions, decision service/model identifiers,
reported input/output tokens, adapter timing, and fixed error categories. The
MCP process also emits an `ahu.mcp.session` summary on EOF or handled SIGINT/SIGTERM
shutdown with
request, tool-call, decision-call, tool-error, tools-list, and transport-error
counts plus SDK exporter initialization
status (not collector receipt). It omits request state,
instructions, question names, answer values, and error text. ahu-launched
harnesses pass agent, agent version, harness, harness version, model, ahu
version, task ID, and attempt as approved
resource attributes to MCP child processes; optional eval run, case, corpus,
and stage identifiers can be passed as `ahu.eval.*` resource attributes. These
fields group calls by harness and join them to task results. MCP spans cover
requests that reach ahu's MCP server, not harness-internal tools or tools
provided by other servers. No prompts or transcript content are exported by
these spans.

Harness event streams are not identical. Codex, Claude Code, Antigravity, and
OpenCode use different event names and terminal records, so ahu normalizes
terminal status and the common usage keys where they are present. Provider
native OTel spans, if a harness emits them, remain harness-owned and may use
different semantic conventions. Interactive sessions generally provide timing
and process status; headless sessions additionally provide the structured event
usage fields that ahu can normalize.

### Cross-harness cost and quota coverage

ahu records harness-reported USD only where the headless event stream exposes
it. The field is named `ahu.cost.harness_reported_usd`; its source label is
`claude_code_result_total` or `opencode_step_finish_sum`. Neither value is
treated as a final bill. Claude Code's result amount can differ from account
billing and its meaning depends on whether the account uses subscription or API
key billing. OpenCode computes amounts from its provider/model cost data and can
report zero when no price is configured; ahu sums each distinct `step_finish`
part once. The JSON event stream may omit its last `step_finish`, so its observed
amount can be incomplete. See the [Claude Code usage and limits guide](https://support.claude.com/en/articles/14552983-models-usage-and-limits-in-claude-code),
[Claude Code monitoring guide](https://code.claude.com/docs/en/monitoring-usage),
[OpenCode JSON run events](https://opencode.ai/docs/cli/), and
[OpenCode stats command](https://opencode.ai/v2/docs/cli/commands/).

| Harness | Usage/cost signal and scope | Access method and freshness | Evidence level | Limits and unavailable signals |
| --- | --- | --- | --- | --- |
| Codex | `turn.completed` token usage, per completed turn. No USD field. | Headless `codex exec --json` event stream; observed at turn completion. | Documented machine-readable event schema; adapter covered by protocol fixtures. | Cost is unknown. The separate account rate-limit RPC reports used percentage, window duration and reset time, not token counts; ahu reads it only through `ahu auth budget` for a matching active project profile. |
| Claude Code | Result token usage and `total_cost_usd` for the headless invocation; API-request events have request scope. | Headless `--output-format stream-json`; result is fresh at invocation completion, request usage when its event arrives. | Documented result/usage and monitoring interfaces; adapter covered by protocol fixtures. | USD is harness-reported and may differ from account billing. Subscription/account quota is unknown to ahu; interactive `/usage` is not ingested. |
| Antigravity CLI | `step_update` usage is per event; terminal `result.usage` is invocation-scoped. Cost is unavailable. | Headless `--output-format stream-json`; observed as steps arrive and, when present, at the terminal result. | Documented NDJSON schema; adapter covered by protocol fixtures. | Cost and quota are unknown to ahu. Interactive `/usage` and `/credits` are not ingested. A missing terminal result leaves task completion evidence incomplete even when step usage was observed. The documented interactive `statusLine` callback provides quota fractions and reset fields, alongside email, workspace and transcript metadata. ahu has no reviewed opt-in bridge binding those callbacks to the active account, project and observation lifetime. Raw callback payloads must not be retained or exported, and existing callbacks must not be overwritten. Antigravity quota remains unsupported; this does not establish that a safe future bridge is impossible. |
| OpenCode | `step_finish` token usage and `cost`, per model step. | Headless `opencode run --format json`; fresh when each step event arrives. | Documented JSON run events; adapter covered by protocol fixtures. | Cost is an engine/provider-model amount, not billing proof; a missing final event can make the sum incomplete. Quota is unknown; `opencode stats` may include other sessions and is not ingested. |
| Kiro CLI V3 | Session-info prompt-completion credits/elapsed time and context-window percentages; partial current-attempt observations. | Headless `chat --v3 --no-interactive --output-format stream-json`; live markers exclude replay from accounting. | Session-event parser and deduplication covered by protocol fixtures; see [Kiro metering](#kiro-cli). | Credits are not tokens or USD, and context percentage is not account quota. Missing values, full session/helper usage, USD, and remaining quota stay unknown. |

This matrix describes documented, machine-readable information that ahu can
collect without scraping a terminal UI, reading harness databases, or deriving
cost from list prices. Missing values stay unavailable. Token usage, USD
estimates, provider invoices, subscription quota, and rate-limit failures are
separate signals and must not be substituted for one another. Source references:
[Codex event schema](https://github.com/openai/codex/blob/main/sdk/typescript/src/events.ts),
[Codex app-server account rate-limit RPC](https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/src/protocol/common.rs),
[Codex rate-limit response schema](https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/src/protocol/v2/account.rs),
[Antigravity headless stream](https://www.antigravity.google/docs/cli/headless/),
[Antigravity status line](https://www.antigravity.google/docs/cli/statusline/),
[Antigravity credits](https://www.antigravity.google/docs/cli/credits/), and
[Antigravity usage command](https://antigravity.google/docs/cli/commands/usage).

These are ahu's current headless adapters; they do not establish equivalent
coverage for interactive sessions. A model-scoped agent definition can make
cost and capacity comparisons clearer because the harness/model identity is
frozen per task. Such definitions should be reviewed and authored as normal ahu
agents under the context-lock and identity rules. ahu should not generate or
switch model variants automatically based on stale quota or price information.

### Instrumentation overhead budget

Use these per-attempt regression budgets for the opt-in headless measurements:

- Local numeric metrics, including their addition to the result record: at most
  100 ms additional p95 launch time and at most 1 ms additional p95 atomic result
  write time.
- Local OTLP trace export: at most 1.1 s additional p95 wall time for a
  short-lived headless process. Span completion remains nonblocking when the
  bounded queue is full; dropping telemetry is preferable to delaying the task.

A synthetic local benchmark used an instantaneous fake Antigravity CLI, a
loopback OTLP receiver, and 12 matched launch rounds. Baseline launch median/p95
was 2.64/2.71 s. Opt-in local metrics changed matched launch time by 4 ms median
and 86 ms p95; local result serialization and atomic write added 18 microseconds
median and 39 microseconds p95 across 500 paired writes. OTLP export added 780 ms
median and 1.03 s p95; metrics plus OTLP added 774 ms median and 903 ms p95.
These short, machine-specific synthetic measurements are evidence for the
budgets, not a performance guarantee for other hosts, collectors, harnesses, or
model latency. The budget is not yet automatically enforced by CI.

Headless results also distinguish the skill catalog copied into the task
worktree from observed skill invocations. A catalog entry records only its
name, source path, and content digest. A skill invocation is recorded only when
the harness emits a recognizable skill/tool event; mentioning a skill in text,
having a skill on disk, or having an unrecognized event does not count as use.
When an observed name matches one catalog entry, the invocation includes its
repo-relative source and digest. Identical duplicate copies establish only the
digest; conflicting copies or no local match establish neither. This is local
catalog attribution, not proof of which path the harness loaded. OTel records
the observed name and digest when resolved, but omits source paths. Invocation
records are bounded and do not retain skill contents, prompts, or tool
arguments. A present catalog entry with no invocation event is unobserved use.
`skill_observation: unavailable` means no recognizable skill-tool event was
seen; it does not mean a project skill itself is unavailable. Likely protocol
drift is counted in `skill_unknown_events` and does not count as use. A catalog
entry without a matching invocation is unobserved use, and only an explicit
failed tool result is reported as a failed invocation.

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

Headless agents can declare nested `input_schema` and `output_schema` objects
in that YAML frontmatter. See [structured agent contracts](structured-contracts.md)
for a complete example, the supported Draft 2020-12 subset, and input/result bounds.

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
pending behavior change for the next version bump. The report names the fields
that moved—see [drift names what changed](#drift-names-what-changed).

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

Interactive launches (including `ahu opencode` and interactive dry runs) also
check top-level `--help` output from the resolved program for the model, prompt,
and approval flags the command needs. OpenCode 2.0.32 lacks top-level `--model`, so
ahu checks `mini --help` and uses that persistent interface for `prompt` permissions.
The v2 `mini` interface advertises `--model` and `--prompt` but rejects `--auto`;
ahu refuses interactive `auto` launches without dropping the requested approval
control or substituting one-shot `run`. The existing default interactive shape
remains available with compatible 1.x command-line versions.

#### OpenCode 2.0.32 interactive automatic approvals

An offline review on 2026-10-19 did not establish a supported way to combine
`mini`'s exact model and persistent session with automatic approval of requests
that native policy has not denied. The refusal above remains intentional; this
is version-specific evidence, not a claim about every future OpenCode 2 release.

| Candidate | Evidence from the installed 2.0.32 CLI and bundled source |
| --- | --- |
| `mini --auto` or `--auto mini` | Both exit 1 with an unrecognized-option error. The command declaration includes `--model` and `--prompt`, but omits the approval options shared by the default TUI and `run`. The hidden aliases `--yolo` and `--dangerously-skip-permissions` also fail on `mini`. |
| `OPENCODE_PERMISSION` | No reader or declaration was found in the installed bundle. The server's explicit environment-to-config mapping reads `OPENCODE_CONFIG`, `OPENCODE_CONFIG_DIR`, and `OPENCODE_CONFIG_CONTENT`, but provides no permission override through this variable. Setting it is not a verified approval control on this version. |
| `OPENCODE_CLI_CONFIG_CONTENT` with `session.permissions = "autoaccept"` | The bundled CLI guide documents merging inline JSON over global CLI settings. The full TUI implements this preference by replying to pending requests. Mini's inspected session transport instead displays pending permissions and its frontend sends replies from the permission interaction; no equivalent automatic approval behavior was found. A setting accepted by the CLI schema is not evidence that Mini applies it. |
| Replacing native permission rules through inline server configuration | This supplies configuration, not a deny-preserving approval mode. No supported transformation was established that approves all otherwise-asked requests while preserving existing user, project, agent, and managed policy. A blanket allow overlay is not a substitute for native `--auto`. |

The packaged changelog also describes per-project settings that automatically
accept permissions. ahu does not change those persisted settings, replace
existing environment configuration, select a different native agent, or install a permission plugin
to work around this gap. `prompt` continues to inherit native policy;
`accept-edits` remains unsupported. An operator choosing a different manifest
permission mode must make that change explicitly.

Evidence was limited to local help, option-parser failures in fresh temporary
homes and XDG directories, and static inspection of the shipped bundle and
packaged documentation. No native user configuration, credentials, project
bindings, or approval settings were inspected or changed. No server, inference
session, or network probe was used; deny preservation through a live Mini
session remains unverified. Existing synthetic admission tests cover the
refusal without creating a task worktree. Revisit this mapping only with
version-specific evidence for exact model selection, persistence, conversion
of asks to approvals, and preservation of native denies and configuration.

Explicit `--headless` tasks retain `opencode run --format json --model ...`
with a positional message and `--auto` only for automatic approvals. The
2.0.32 run help advertises those options, but help inspection alone does not
establish event-stream, resume, or native isolation compatibility; the separate
headless admission checks still apply. No live v2 inference is claimed here.

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
ahu @glm-reviewer 'Review the configuration validation and report findings.' --dry-run
```

The preview's `argv` shows the command ahu builds:
`opencode --model ollama/glm-5.3:cloud --prompt <PROMPT>`, with `--auto` added
only for `permissions = "auto"`. The prompt is one argument and never passes
through a shell; see [prompts are data](#prompts-are-data). Then submit the real
assignment:

```sh
printf '%s\n' 'Review configuration validation and report findings.' > assignment.txt
ahu @glm-reviewer --prompt-file assignment.txt
```

This manifest declares `prompt`, so no approval-widening flag is needed. A
manifest declaring `auto` requires `--allow-widened-approvals` on previews and
launches alike. Interactive launches need cmux. The same agent runs unattended
with `--headless`, which builds `opencode run --format json` instead and needs
no cmux; see [headless execution](#headless-execution).

`auto` is the widest of the three and OpenCode has no sandbox to narrow it:
unlike a Codex launch, which ahu gives `--sandbox workspace-write`, an OpenCode
session's file tools can address absolute paths outside the task worktree.
Headless supervision detects some reported targets and stops the attempt after
observing them; [streaming write detection](#headless-execution) describes its
limits. This does not prevent the write or sandbox the OpenCode process.

For a coordinating OpenCode session in the current terminal, rather than an
agent in a task worktree:

```sh
ahu opencode
```

That session inherits the terminal and the working directory. ahu passes it
`--model <provider>/<model>`, the project's top-ranked OpenCode model, and
`--auto`, which approves every permission OpenCode does not explicitly deny,
without asking. Permissions set to `deny` in OpenCode's own configuration still
apply; `--auto` cannot widen past them.

### Troubleshooting

ahu's preflight covers Git, project configuration, the manifest, and the
`opencode` executable on `PATH`. Opt-in [account bindings](#local-harness-account-bindings)
also check a local Ollama login for recognized cloud model names. That check
does not verify OpenCode's effective provider endpoint, model availability,
context limits, or the identity used by a later inference request.

| Symptom | Your check | What ahu does |
| --- | --- | --- |
| `opencode` not found; exit `4` | `command -v opencode` | Reports the missing prerequisite and stops before creating a worktree. |
| The session cannot reach the endpoint | `curl -sS http://localhost:11434/v1/models` | Does not probe OpenCode's effective inference endpoint. The failure surfaces in the session; the task, branch, and worktree remain. |
| `ollama/glm-5.3:cloud` does not resolve; `opencode models` lists only `opencode/*` | The `provider` block in `opencode.json` or the global configuration | Does not write or repair OpenCode configuration. |
| The tag is absent from `ollama list` | `ollama list`, then pull the tag yourself | Does not pull or create models. |
| Hosted-model authentication fails | Ollama's own sign-in for cloud models | Holds no provider credentials and cannot authenticate for you. |
| Context below 64k | `ollama show <model>`; `limit.context` in the provider entry | Does not set or raise context limits. |
| Manifest declares `accept-edits` | The manifest's `permissions` value | Refuses the manifest with an error and offers no substitute mode, interactively and headless alike. Declare `prompt` or `auto`. |
| A headless launch reports no terminal event | The bounded result metadata and harness-owned session history | Scores a run only on OpenCode's own terminal step, never on its exit status: a refused tool call ends the run with exit 0. |

ahu refuses its own failed preflight checks; provider failures surface through
the harness. It never silently selects another model, provider, or harness,
and a failed session does not remove the task's branch, worktree, or record.

## What a task gets

Inspect and review a task from any checkout of the repository that launched it:

```sh
task_id=abc123  # replace with a task ID from ahu tasks
ahu task "$task_id" --output json
```

Task commands accept a full task ID or an unambiguous prefix. `ahu focus`
uses the newest record matching its prefix without checking ambiguity; use
a full ID when focusing a task. Inspection reads the saved record and
attempt metadata, then checks the recorded session-owner signal. A confirmed
missing owner reconciles a live record to `exited` under its task-state lock;
an unreadable or unavailable signal reports `unknown` and does not mark the task
running. Interactive runner processes hold `owner.lock`; older tasks without that
proof remain `unknown`, even when their terminal workspace still exists. JSON schema version 2 includes `task_id`, `agent`,
`harness`, `model`, `branch`, `base_commit`, `worktree`, `worktree_exists`,
`record_path`, `cmux_workspace_id`, and `cmux_window_id`. `session_state` is the
effective `starting`, `running`, `waiting-for-approval`, `exited`, `failed`,
`cancelled`, or `unknown` state; `completion_verified` is always `false`.
Headless records also include an
`attempt` projection with outcome, ownership observation, blockers, session
provenance, and review paths. Missing optional values are JSON null.
JSON remains unstyled even with `--color=always` and omits prompt text and titles.
Consumers should tolerate additional fields and check `schema_version`.

The interactive task list labels states such as `session running` and `session exited`.
A running session may be awaiting input; a process exit does not verify success.

### The task list

`ahu tasks` lays its table out to fit the terminal. The width comes from
`COLUMNS` when it is set, otherwise from the terminal itself, and is 80 when
stdout is redirected or reports no size. No row ever wraps.

`HANDLE`, `STATE`, `AGENT`, and `RUNTIME` keep their full width in any terminal
of 80 columns or more. Each says something no other column in the row does, and
`RUNTIME` is where a reader sees that a task ran on its own harness and model. A
handle is never shortened at any width: a cut handle does not resolve, and
resolving it is the only reason the column is there.

`BRANCH` is compact rather than whole: the branch prefix and the first eight
characters of the task id, as in `ahu/builder/01a0df38`. `ahu task <handle>`
prints the whole branch name for anyone who needs to paste it.

`TITLE` appears only when the title says something its handle does not, and only
when there is room left over. Handles are generated from titles, so a short title
comes through its handle word for word; such a title is left out of its row, and
a title column with nothing in any row leaves the table entirely.

The `STATE` column includes verified activity, so there is no separate `LIVE`
column. Unknown ownership never appears as `running`. Whole columns leave the
table in this order as the terminal narrows: `MODE`, then `TITLE`, then
`BRANCH`, then `AGENT`, and `RUNTIME` last. Before a column leaves, the title
and the branch are cut with an ellipsis. Whatever the surviving columns do not
need goes to the branch until it is whole, and then to the title.

| Column | What it holds |
| --- | --- |
| `HANDLE` | The task's `@name`, or the task id when it has none |
| `TITLE` | The recorded task title, when it adds something the handle does not |
| `STATE` | The reconciled session state or `unknown` when activity cannot be verified |
| `AGENT` | The agent and its version |
| `MODE` | `cmux`, `plugin`, `terminal`, or `headless` |
| `RUNTIME` | The pinned harness and model |
| `BRANCH` | The task's branch, prefix and the first eight characters of the task id |

Color, when stdout is a terminal and `NO_COLOR` is unset, carries meaning and
nothing else: handles and agents are agent-colored, the state is colored by
what it means for the reader, `unknown` is dimmed rather than asserted, the
runtime is runtime-colored, and the branch is dim. `ahu
tasks --output json` is unaffected by width and by color alike.

ahu has no diff command. A task is an ordinary branch in an ordinary checkout,
so Git reviews it directly and without an ahu-shaped wrapper around it. Text
`ahu task` prints a `review` line with the exact command for that task:

```sh
git -C /path/to/.worktrees/<task-id> diff <base-commit>
```

The same two values are `worktree` and `base_commit` in `ahu task --output
json`, so a script builds whatever Git invocation it needs. Inspection does not
stage, commit, or apply anything. If the attempt's result envelope records
write-tool paths outside the worktree, `ahu task` prints those paths too: the
branch alone does not show everything the session touched. See `ahu result` for
the full recorded list.

- **A fresh branch and worktree.** `ahu/<agent>/<task-id>`, based on the HEAD of
  the checkout you launched from, under `.worktrees/` in the primary checkout.
  Repeated and concurrent launches always produce distinct tasks.
- **Recognized repository agent configuration.** Recognized `.agents/`, `.claude/`,
  `.codex/`, `.agent/`, and `.opencode/` directories, plus `CLAUDE.md`,
  `CLAUDE.local.md`, `AGENTS.md`, `AGENTS.override.md`, `.mcp.json`,
  `opencode.json`, and `opencode.jsonc` files, are
  copied at their native paths from the committed checkout. Launch admission
  requires these inputs and `.agents/ahu/lock.md` to match clean Git state; uncommitted or
  untracked context must be reviewed, locked, and committed before launch. Some of this is executable
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

### Drift names what changed

A digest pair says that something moved, not what to do about it. A task record
stores the identity the launch ran with—version, harness, model, permissions,
instruction source and its digests—so drift compares those fields with the
agent's manifest as it stands now and reports the ones that differ, in words,
before any digest:

```
drift        1 registered agent drifted
  @chris 1.0.0  since task @fix-flaky-test (2026-09-24)
    - model: claude-sonnet-5 -> claude-opus-5 (version still 1.0.0)
  Bump the version in the agent's manifest and record what changed, or restore it.
```

`(version still 1.0.0)` rides on the first change because that is the case drift
exists to catch: the inputs moved and the label did not.

An edit to the agent's files is named the same way, by file rather than by
digest, and says which bytes moved—`instructions: .claude/agents/chris.md
changed, text ahu delivers`, or `… changed, frontmatter or metadata only -- the
text ahu delivers is unchanged` when the delivered instructions are untouched.

Digest pairs remain for the inputs no manifest field names, each with the inputs
it spans said in words: `the repository agent configuration changed`, `the hooks
in effect changed`, `the project policy changed`. A record written before ahu
split the two file digests has nothing to name, so it falls back to the combined
identity digest.

The launch-time warning and the `ahu doctor` drift section are the same
comparison rendered twice, so the two cannot disagree about what moved. Naming a
field does not change what counts as drift: a moved digest still decides that,
and drift still warns rather than blocking a launch.

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
ahu inspects Claude Code hook settings; general hook coverage for the other
harnesses remains incomplete. The separate cmux integration inspection recognizes
exact reviewed native components. Ordinary launch and inspection do not install
or edit hooks. The explicit [cmux installer](#cmux-native-integration) delegates
changes to the native command.

Only a hook's program is shown, never its arguments, and only its digest is
stored in the task record—hook commands routinely carry tokens, and a report
must not leak a credential merely to describe a hook.

For Claude Code, the launch preview and `ahu doctor` describe hooks ahu can read
from `.claude/settings.json`, `.claude/settings.local.json`, `~/.claude/settings.json`,
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

## Interactive host boundary

Interactive session operations use the in-process Rust interface
`interactive::v1::Host`. The built-in cmux adapter supplies capability checks,
workspace and group queries, creation, focus, close, sidebar updates, and
Markdown opening. The repository manager remains in ahu and validates recorded
group/window ownership again before task mutations. Launch admission, native
credentials and approvals, `run-task` supervision, cancellation confirmation,
and worktree cleanup remain in their existing core paths.

The adapter is now the optional `ahu-cmux-plugin` Rust package in `plugins/cmux`.
The `ahu-host-api` package owns the V1 types and error contract. Existing
`interactive::v1` and default-build `cmux` Rust paths remain compatibility
exports. Task fields (`cmux_*`) and the repository mapping (`cmux.json`) retain
their format; existing records still route exclusively to cmux. The V1 interface
models the current group/window/workspace topology. The compiled adapter has no dynamic ABI. Local executable packages use the separate
[Host protocol V1](executable-hosts.md), with explicit negotiation and activation.

Build or install from the complete source workspace, with its root `Cargo.lock`:

```sh
cargo build --locked --features cmux-plugin
cargo install --path . --locked --features cmux-plugin
# A separate build that omits cmux session mechanics:
cargo build --locked --no-default-features
```

The default build uses the current terminal and has no cmux dependency. `ahu plugins`
(or `--output json`) reports compiled packages, committed bindings, and explicitly
supplied local packages, declared Host API version, and selection. It reads
committed binding policy and available installed metadata without executing plugin
code, probing cmux, verifying source pins, or changing activation.
`compiled` and `selected` do not establish authority, reachability
or capability support. The workspace crates are not published to a registry;
individual `.crate` archives are package-boundary inspection artifacts, not a
standalone installation channel. Source installation includes local dependencies.

Set `AHU_INTERACTIVE_PLUGIN=cmux` to explicitly select the compiled package for a process,
or `AHU_INTERACTIVE_PLUGIN=none` to disable session hosting. Activation without an
environment selector uses a single enabled committed binding, otherwise the build
default. Bindings with `execute` but no `interactive-backend` grant are excluded
from session defaults. Multiple enabled session bindings require an explicit id.
Empty, unknown, and non-Unicode values refuse selection. Process selection never grants execution.
Actual cmux activation resolves the executable with the existing core checks,
then the package pings cmux. Task launch checks capabilities before creating a
worktree. No harness, model, or backend substitution occurs.

Without an enabled session-host binding, the built-in `terminal` backend is
selected. Interactive tasks run in the caller's foreground
terminal, with the same task worktree, frozen launch identity, process
supervision, and cancellation path. `ahu plugins` reports this selection without
probing cmux. Set `AHU_INTERACTIVE_PLUGIN=terminal` to choose it explicitly;
`none` disables interactive task launch. Headless execution,
coordinators (`ahu codex`, etc.), and local inspection need no session plugin.
Focus, Markdown opening, and cleanup requiring a recorded cmux workspace still
need the cmux plugin.
Unavailable inventory remains unknown; records are not marked stale or deleted
on that evidence. Re-enable/rebuild the plugin to inspect or clean up recorded
cmux resources. Disabling it does not close existing sessions. Cancellation still
uses core supervision; workspace close remains best effort after confirmed
termination and can be retried with the plugin available.

Headless execution never needs the session package. Native cmux hook inspection,
wrapper refusal, environment cleanup, and headless isolation checks remain in
core and active independently of session hosting. `ahu cmux install` still
explicitly delegates native harness hook installation; it does not install or
activate this session package.

### Committed project plugin bindings

`ahu plugins --project [--output json]` inspects only the bindings committed at
`.agents/ahu/plugins/<id>.md` in the selected checkout's HEAD. Use `ahu --repo PATH`
to select another checkout. It pins the commit once and reads local Git objects;
index and working-tree changes, including untracked bindings, are excluded.
An absent binding directory or a genuinely unborn HEAD yields an empty inventory
(`revision: null` for unborn HEAD). Malformed refs or missing committed objects
remain errors. No ahu project configuration, context lock, credentials,
native auth stores, or local package selection is consulted.

This mode accepts the [project binding schema](project-documents.md#plugin-binding-foundation),
including credential-free HTTPS repositories on any host. Repository URLs,
revisions, hashes and grants remain declarations. Nothing fetches, checks out,
builds, installs or executes a package. Git replacement objects are disabled.
`GIT_NO_LAZY_FETCH=1` requests local-only object reads; an empty `GIT_ALLOW_PROTOCOL` denies every transport, including
file and executable helpers, even on Git versions that predate that lazy-fetch
control. Missing local objects fail offline. No Git hooks or content filters run.

JSON schema 1 identifies `origin: committed-project-bindings`, the inspected
`revision`, and ID-sorted `bindings`. Each entry has `path`, the typed `binding`,
`ahu_compatible` (exact match with this ahu version), `source_verified: false`,
`activation_supported: true`, and `activation_blockers`. This inventory does not
verify packages, so blockers include `source-unverified`; disabled or incompatible
bindings add `disabled` or `ahu-version-mismatch`. A successful listing means the
declarations parsed, not that the source exists, its pin matches, or execution is
approved. General OKF metadata and document bodies remain inert and are omitted
from this report. Activation separately enforces committed grants.

At most 32 regular binding files are accepted, each bounded by the shared OKF
2 MiB limit. Tree listings are bounded to 64 KiB. Symlinks (including committed
ancestors), submodules, nested directories, non-Markdown entries and malformed
bindings fail the whole inventory without partial output. Inspection writes no
state and does not require the working tree to match HEAD. It does not establish
launch readiness or validate the project's context lock.

`--project` and `--probe` are mutually exclusive. Activation requires committed
`enabled: true`, the `interactive-backend` grant, exact ahu compatibility, and a
verified package matching the source, identity and version pins. These checks
also apply to `AHU_PLUGIN_PATH` packages. A single enabled binding selects that
host by default; generic `execute`-only bindings are excluded. Multiple enabled
session bindings require `AHU_INTERACTIVE_PLUGIN=<id>`.
Explicit `cmux` retains the compiled host and `none` disables hosting and selects
no inventory rows. Both names are reserved and cannot be used as binding IDs.

### Project-local installation lifecycle

The lifecycle operates on the selected checkout's **committed** OKF binding.
Create or edit `.agents/ahu/plugins/<id>.md`, review its pins and capabilities,
and commit it before installing. These commands never stage or commit changes:

```sh
ahu plugins digest /absolute/path/to/package
ahu plugins install example
ahu plugins installed --output json
ahu plugins enable example
# Review the changed binding, refresh the context lock, and commit them.
ahu plugins update example
ahu plugins disable example
# Review and commit the disabled binding before removal.
ahu plugins remove example
```

`install` acquires and verifies a declared source; a different existing installation
requires `update`. `update` installs the newly committed pin and retains all previous
versions. An already installed pin is verified again without reacquisition. Neither
operation changes whether the plugin is enabled, grants, native harness configuration, or process host
selection. `enable` requires a verified installation; `disable` and `enable` edit
only `ahu.enabled` in the working binding, preserving general OKF metadata and the
Markdown body. Enable, disable and remove require the binding to match HEAD in both the index
and working tree, including enabled state, metadata, body and executable mode.
Staged changes are refused even when the working copy has been restored to HEAD.
The check reads Git objects and working bytes without running content filters.
Review and refresh the context lock before committing policy changes.

Sources support these declaration forms within `ahu.source`:

```yaml
# Git on any HTTPS forge, without credentials, query parameters or fragments.
kind: git                     # default; existing declarations remain valid
repository: https://example.invalid/team/packages.git
revision: <full-lowercase-commit-object-id>
subdirectory: packages/example # optional; plain relative path
sha256: <64-lowercase-hex-digits>
```

Git `repository` may also be an absolute local ordinary or bare repository path.
Local Git sources copy bounded object data into an isolated temporary repository;
source Git configuration, hooks, replacement refs and upload-pack are never used.
Linked-worktree `.git` files and object alternates are refused. Missing objects fail
instead of lazy fetching. Remote sources use only HTTPS and fetch the exact commit;
mutable refs, tags and tree objects are not accepted as revision pins. Submodules
are not fetched. Redirects, credential helpers, credential prompt programs, inherited Git environment,
proxies and global/system Git configuration are excluded from source operations.
Private authenticated repositories, SSH, file URLs, custom certificate configuration
and custom transports are unsupported. HTTPS transfer has a 120-second deadline
and a 128 MiB per-file output limit; extracted content is bounded as below.

For a local directory snapshot use `kind: local`, an absolute `repository` path,
optional `subdirectory`, and set **both** `revision` and `sha256` to the digest
returned by `ahu plugins digest /absolute/package/path`. This reads and hashes local
files only. For Git sources, calculate the same digest from the intended package
content; install independently checks each blob in the immutable commit.
Use physical absolute paths: symlinks in any source path component are rejected.
A directory containing `.git` cannot itself be a package; select its package
subdirectory. Acquisition copies content; subsequent edits to a local source do
not alter an installed version.

The digest is SHA-256 over `ahu-plugin-tree-v1` plus a NUL, followed by files in
UTF-8 path byte order. Each file contributes its relative path length (u64, most
significant byte first), path bytes, one byte for whether any executable bit is set,
content length (u64, most significant byte first), then content bytes. Empty
directories are excluded. Packages support up to 4,096 entries and 32 path
components, with a 64 MiB per-file limit and 128 MiB total content limit.
Symlinks, special files, files with set-user-ID or set-group-ID bits, unsafe paths, Git metadata and Git submodules
are rejected. Manifest ID and host API must match the binding; the manifest's
Cargo package name must match the Cargo manifest, and its explicit package version must
match the binding. Cargo is never invoked. Build scripts, hooks, install scripts,
filters and declared package executables are never executed by installation.

Snapshots and receipts live in the primary checkout's owner-only, self-ignored
`.ahu/state/plugins/<id>/<installation-key>/`. An installation key includes source,
version and compatibility, but excludes enabled state and grants. Data is verified in
private staging and published by directory rename; old installations are never
replaced. Ordinary errors remove their staging directory. A crash may leave an
`.install-*` staging directory; it is never considered installed. Retry with
`update`; explicit `remove` can clean incomplete staging once cleanup is safe.
Tampered installed data is refused: disable and remove it before reinstalling.
Tracked installation paths and redirected state directories are refused.

`installed [--output json]` reads receipts and verifies installed data again offline,
without creating state or cleaning staging. It reports source verification,
whether each version matches the current committed binding, and whether it is enabled.
`--project` remains a declaration-only inventory; it never reads or
modifies installed state. Neither inventory executes a plugin or activates a host.

Legacy records lack plugin-version pins, so cleanup remains
conservative: `disable` and `remove` refuse while **any** project task is live or
unreadable, or task ownership is uncertain. Updates retain all versions, including
those possibly referenced by tasks. Cleanup holds the interactive and headless admission locks and locks terminal
headless task ownership against concurrent resume.
Both commands also refuse when a terminal task records this package and a workspace
ID: the host is still needed to verify workspace ownership or absence during task
removal, even if the workspace was already closed externally. This check reads
records without executing the package.
`remove` requires the binding to be disabled or absent in both HEAD and the
working tree, removes the plugin's installed versions and staging, and retains the binding.
Selective version pruning and automatic reclamation are not implemented. Filesystem checks do not isolate concurrent
hostile processes running as the same user.

Lifecycle commands do not execute package code. `enable` edits the binding;
commit that edit before activation. Executable activation freezes a task-level
package and executable pin. Updates preserve every installed version, and task
operations resolve the recorded version rather than the current selection.

Finish or cancel affected tasks, preserve their work (including integrating any
task-only commits), and run `ahu remove TASK` while the package remains installed
and authorized. Then run `ahu plugins disable ID`, review and commit the binding
and refreshed context lock, and run `ahu plugins remove ID`. Closing a workspace
alone does not remove its task record or establish safe cleanup.

If authority was already revoked by a direct binding edit, task removal still
fails closed. To recover, explicitly review and restore the committed enabled state and the
`interactive-backend` grant in the task's launching checkout. Package recovery depends
on the task's frozen host pin:

- For an installed package (`discovered_root` is absent), restore the exact recorded
  binding, package, and version, including its pinned source and executable identity.
  Recover the pinned source and commit the recorded binding, then use
  `plugins install` or `plugins update` as appropriate; updates retain old versions.
- For a discovered package, restore the exact trusted package at the recorded
  `discovered_root` path, including executable permissions. Installing a copy in
  the package store or changing `AHU_PLUGIN_PATH` does not replace that frozen path.

Refresh the context lock with policy changes. Remove the tasks while the recorded
package is available and authorized, then disable and remove the package in the
order above. If its identity or authority cannot safely be restored, leave the task
records and work intact; ahu does not execute a disabled package or substitute a
different host to bypass this check.

### Local session package discovery and selection

`AHU_PLUGIN_PATH` is an explicit colon-separated list of absolute package
directories. Each directory must contain `ahu-plugin.md`. Discovery uses no default
search directory, recursive scan, installation, network lookup, Cargo invocation,
or execution of package code. Unset the variable to inspect compiled packages and
committed bindings with available installed metadata;
an empty value or empty path entry is an error. Discovery never selects a local
package, even when cmux was omitted at build time.

A package manifest is OKF 0.2 Markdown with YAML frontmatter on the first line,
closed by a standalone `---` line. LF and CRLF line endings are supported. The
optional Markdown body is documentation only: it is never configuration or code.
Discovery does not fall back to the old TOML plugin manifest filename. The required
frontmatter fields are shown below; an optional `executable` mapping enables the
separate executable protocol described below. A generic `entrypoint` mapping uses
the [one-shot executable plugin protocol](executable-plugins.md) instead; invocation
requires an explicit host grant and committed `execute` permission.

```markdown
---
okf_version: "0.2"
type: ahu:plugin
title: Example session host
description: Example local session package
tags: [session, example]
schema_version: 1
id: example-host
package: example-session-host
host_api: 1
cargo_manifest: Cargo.toml
---

# Example session host

Local package metadata for discovery.
```

The `id` is the selection identity. IDs must be unique across the inventory;
`cmux` and `none` are reserved. Both `id` and `package` use 1–64 lowercase ASCII
letters, digits, hyphens or underscores and start with a letter. Only OKF 0.2,
`type: ahu:plugin`, schema 1, and Host API 1 are admitted. Missing, duplicate,
or incorrectly typed ahu execution fields are errors. Standard and extension OKF
metadata is retained in the inventory's `metadata` object and remains inert; it
never grants host capabilities. The named `Cargo.toml` is still parsed as TOML with
`[package].name` equal to `package`. This checks declared metadata only: it does
not validate the full Cargo package, resolve dependencies, compile it, or prove
that it implements Host V1.

`cargo_manifest` is relative to the directory containing `ahu-plugin.md`,
ends in `Cargo.toml`, and has at most 32 normal path components. Absolute paths,
empty components, `.`/`..`, and backslashes are refused. Reads use pinned
directory descriptors and refuse symlinks in every component, including package
roots, ancestors, and both manifest files; even links within a package are
refused. Supply physical absolute paths (for example, from `pwd -P`). Root paths
must be UTF-8, have at most 64 components, and contain no `.` or `..` components.
Both metadata files must be regular UTF-8 files of at most 64 KiB each; the
Markdown limit includes both frontmatter and body. At most
32 package directories and 64 KiB of search-path text are accepted. Invalid
entries fail the entire inventory, even with selection set to `none`.

The cmux source package includes an inspection manifest with ID `cmux-local`.
From the physical ahu checkout root:

```sh
AHU_PLUGIN_PATH="$(pwd -P)/plugins/cmux" ahu plugins --output json
AHU_PLUGIN_PATH="$(pwd -P)/plugins/cmux" AHU_INTERACTIVE_PLUGIN=cmux-local ahu plugins
```

The second command explicitly selects the local package for inspection. It does
not enable it. JSON schema 1 retains the existing fields and adds `origin`,
`manifest`, `cargo_manifest`, `executable`, `entrypoint`, and `activation_blocker`. The compiled cmux entry
comes first, followed by binding/local entries sorted by ID. A discovery path for
a binding ID takes precedence over its installation, matching activation.
Installed metadata has origin `installed-manifest`; a binding whose installed
metadata is unavailable has origin `committed-binding`, package `unknown`, and an
inspection blocker without changing selection. Selection sources are
`environment`, `committed-project-binding`, or `build-default`. Local entries always have
`compiled: false`; source-only entries have an activation blocker. `selected: true`
records selection intent, not a running host or an execution grant. A successful
launch freezes the verified package identity in its task record.

Packages with an `executable` frontmatter mapping can be activated explicitly with
`ahu plugins --probe [--output json]` or through `interactive::discover()`. The
[executable host protocol](executable-hosts.md) specifies process resolution,
framing bounds, version and capability negotiation, deadlines, cancellation,
path protections, and partial-start outcomes. Ordinary discovery never executes
the package. The current terminal is the default when no enabled binding or
environment selector exists, and headless execution does not consult this
inventory.

Executable task launch, focus, reconciliation, cancellation cleanup and removal
use the task's frozen `session_host` identity. Current committed grants still
apply. Old records without that field retain legacy cmux semantics. Group
mappings are separate for each exact external package identity; legacy
`cmux.json` mappings remain readable. Headless tasks do not activate a session host, and
current-terminal coordinators do not require cmux.

`ahu doctor` identifies the selected session host ID and package and reports its
reachability and group capabilities separately from native cmux reachability.
An external host's success does not establish that native cmux is available;
native cmux absence is a warning when the selected external host is ready.

The supplied `cmux-local` package remains source-only: no executable cmux server
or reviewed native environment contract is provided. Synthetic protocol fixtures
exercise the lifecycle without starting real terminal sessions. Plugins run as
same-user executable code, without a manifest sandbox; ownership checks do not
isolate hostile plugins or concurrent same-user mutation. Non-cmux topologies and
dynamic compatibility negotiation remain unsupported.

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
| `antigravity` | `cmux hooks antigravity install`; default user Gemini hooks configuration | Configured commands have no fingerprint verifier and remain unknown, refusing headless admission. Custom formats and extension scopes remain unverified; specialized native helper version eligibility is separate. |

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
are observations, not new compatibility claims. [Headless admission](#headless-execution)
checks the installed version and integration evidence; specialized controls
remain tied to their reviewed versions.

| Evidence class | What it establishes | What remains unverified |
| --- | --- | --- |
| Native artifact/source inspection | Exact command/plugin fingerprints, declared settings, guards, and installer arguments | Effective trust, actual hook activation, native event delivery, and unknown builds |
| Synthetic fixtures | Parser, refusal, environment filtering, and fixed-argv behavior under controlled inputs | Actual native harness behavior |
| Real PTY tests | Terminal ownership, stdin/signal handling, restoration, and cancellation with test processes | Live harness terminal interface/session conformance |
| Live cmux plumbing tests | Group/workspace operations and focus behavior with harmless startup processes | Native hooks or harness provider sessions |
| Scoped native Codex hook no-op probes | Reviewed hook behavior under the probed hook-off/absent-surface conditions | A live Codex conversation or equivalence with the other harnesses |

The status report deliberately labels its evidence `locally_inspected;
live_untested`. Do not infer live harness conformance from installation,
static inspection, PTY coverage, or cmux plumbing.

## Committed agent context and evaluation

See [Evaluate agents with ahu](evaluations.md) for a complete Markdown case,
comparison commands, and how to interpret quality, tool and usage measurements.

`.agents/ahu/lock.md` fingerprints the recognized shared repository context files ahu
copies into task worktrees. `ahu lock --update` refreshes it from the current
checkout; it never stages or commits. Review the context diff and lock diff
together, then commit changed shared context before launching. `ahu lock` and
`ahu doctor` report missing, stale, untracked, or locally modified inputs.
Launch and eval candidate admission refuse until shared inputs and the lock
match committed `HEAD`.

Claude's project-local `.claude/settings.local.json` can change the tools
available to an agent, so ahu checks its fingerprint too. That fingerprint is
stored in owner-only host state outside the checkout, separately for each user
and repository. It does not enter shared `.agents/ahu/lock.md` or telemetry. A local
harness may use the file to enable ahu MCP, and a task worktree may inherit it
for startup. Keep it limited to settings the task agent may read. A change
blocks launch until that user runs `ahu lock --update`; local acceptance does
not change or require a commit to shared `.agents/ahu/lock.md`.

When a project has `.agents/mcp_config.json`, ahu also privately fingerprints
the native Antigravity MCP file at `~/.gemini/config/mcp_config.json`. Its contents,
path, and digest stay out of the shared lock. Editing or deleting that file
requires each affected project's local acceptance to be refreshed. The file
stays in native user storage; ahu does not copy it into task worktrees.

The lock is deliberately scoped. Harness built-ins, user and managed settings
outside these recognized local inputs, provider memory, and context in skipped
scan paths cannot be guaranteed by these locks. A clean lock does not prove
those sources are absent or that a harness loaded any particular skill. The
lock's coverage limitations are part of the launch disclosure.

Use the bundled `ahu-agent-context-critic` skill to examine a concrete behavior,
form one context-change hypothesis, run the same OKF suite before and after the
committed change, and compare results with `ahu eval report`. Prefer deterministic
answer and OTel-backed tool checks; use agent evaluators for subjective rubric
dimensions, and review a sample of their judgments. Evaluation files accessible from a
candidate checkout are not secret merely because the prompt omits their answers.

## State and compatibility

Headless tasks use primary-owned coordination; interactive tasks use their own
worktree-local records. `tasks` and `task` include compatible legacy records as
well. `focus` is for interactive cmux sessions.

### Interactive cmux ownership

All interactive harnesses enter `cmux::repository::RepositoryManager` after
harness and model resolution. That manager owns repository-group selection and
recovery, anchor restoration, task-workspace creation, and the recorded
task-to-group/window/workspace association. `ahu focus`, confirmed cancellation,
task status updates, and `ahu remove` cleanup resolve that association through
the same manager. Ambiguous or mismatched ownership fails closed rather than
selecting or closing a similarly named workspace. Headless tasks do not require
cmux and do not enter this lifecycle.

ahu stores cmux's group, window, and workspace identifiers in the task record.
The workspace is the addressable task terminal surface in the cmux API used by
ahu; cmux does not provide ahu a stable separate pane/surface identifier for
that shell. The `surface_id` returned by `markdown.open` is for the separate
explanation viewer and is not a task-session identifier. These limits mean ahu
can confirm and manage the task workspace, but does not claim to inspect the
harness process or terminal internals from cmux alone.

Each interactive task stores `task.json` and `prompt.txt` under
`<task-worktree>/.ahu/state/repos/<repo-identity>/tasks/<task-id>/`. Session status
is part of `task.json`. ahu derives this location from the worktree it creates;
state discovery uses explicit checkout paths. Removing the worktree removes
its state. Task checkouts are siblings under the primary checkout's `.worktrees/`,
including nested launches. State and worktree directories ignore themselves.

`tasks`, `task`, and `focus` discover task records through those worktrees
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

On launch failure, rollback is allowed only when the task session could not have
started, and uses ordinary Git worktree removal, without force.
If removal fails, the error names the retained worktree and branch for inspection.
ahu attempts to discard partial task state only after validating its path; it
leaves redirected paths alone. Cleanup can itself fail, so inspect the reported
paths. A retained worktree without a record appears as incomplete in `ahu tasks`.
Listing does not remove it or establish that its contents are safe to delete.

Once the cmux create command has started, a failed command or discovery lookup
does not prove non-creation. ahu retains the task record, index, worktree and
branch, reports the task identity and inspection paths, and does not invent a
terminal outcome. Every new workspace candidate must match the requested
worktree directory, even when only one candidate appears. Unrelated or ambiguous
workspaces are preserved. Inspect the task and cmux session before retrying;
confirm termination before cleanup. `ahu remove` requires a terminal record. If
the record remains `starting` and manual inspection confirms no session uses the
checkout, use non-forced Git worktree and branch removal rather than fabricating
a terminal task state.

The primary checkout owns the launch lock, cmux group mapping, headless
coordination, and scoped task index. Generated architecture documents use the
invoking checkout’s `.ahu/state/`. All locations derive from
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
cancellation closes the task's recorded session-host workspace. An already-terminal task, an
unconfirmed request, or a task that finishes on its own leaves the workspace open.
A workspace-close failure is reported separately from confirmed process
termination. A terminal outcome is recorded even if foreground restoration fails.

### Utility lookup

Git and explicit cmux lookup skip empty and relative `PATH` entries. Candidates
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

The catalog in `src/catalog.rs` records, per adapter, the CLI versions whose
behavior was checked and the enforcement gaps ahu discloses for it. These are
compatibility evidence, not an allowlist or a claim that newer versions or
account entitlements were tested. Where multiple harness installations exist,
ahu uses the one its own resolution picks; it does not search for a version that
matches the catalog. An exact project pin can make that choice reproducible.
Project configuration pins catalog `2026-10-19`; a mismatch is an error.

Persisted task records use schema 5, including the frozen `session_host` binding
and `waiting-for-approval` state, with hyphenated `UUID v7` IDs. Schemas 3 and 4
remain readable; schema 2 records
also remain readable with their original 18-character hex IDs. Schema 1 is refused. Launch-preview and task-inspection JSON use
schema 1. Incompatible persisted records are refused and listed as unreadable;
ahu does not reinterpret their digests or delete their worktrees.

## Delegation and approval boundaries

Outside ahu, delegation belongs to the current harness. Registered assignments
inside ahu use registered ahu agents running their configured
harnesses and models in separate worktrees. Interactive tasks use the current
terminal by default or an explicitly selected session-host plugin; headless
descendants inherit the execution mode and primary-owned coordination scope.
Headless child grants and bounded native helpers follow the preceding policies.
The supplied delegation contract uses the canonical
`ahu @name --prompt-file assignment.txt` form so saved deliveries can
be replayed. `AHU_BIN` points to the launching ahu executable.

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
request adapter-specific flags; `ahu @agent` requires
`--allow-widened-approvals` for either, including dry runs. Existing harness
settings still affect approvals. The flag grants no additional access to the
calling session.

`prompt` therefore means *ahu imposes nothing*, not *the session will ask*. What
happens next is the harness's own default, and the harnesses do not agree:

| Manifest `permissions` | OpenCode | Effect |
| --- | --- | --- |
| `prompt` | no flag passed | OpenCode's own permission configuration decides. Its defaults allow most tools outright, so an OpenCode agent can edit files and run commands without asking. The built-in `build` agent's only `deny` rules are `question`, `plan_enter` and `plan_exit`; reads of `*.env` and `*.env.*` default to `ask`, not `deny`, and so do `doom_loop` and `external_directory`. |
| `auto` | `--auto` on supported 1.x interactive versions; refused on 2.0.32 `mini` | Where supported, approves every permission that is not explicitly denied. An `ask` rule is not a denial, so `--auto` approves it silently: with OpenCode's defaults that includes reading `*.env` and `*.env.*`, and reaching outside the project directory. If you need those refused rather than merely prompted, write a `deny` rule in your own OpenCode `permission` configuration; ahu does not add one. Still requires `--allow-widened-approvals`. |
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

All four coordinator shortcuts open the same shape of session: the harness's own
approval bypass, plus this project's top-ranked model for that harness, both
passed explicitly on the command line.

| Shortcut | Harness | Arguments ahu passes |
| --- | --- | --- |
| `ahu claude` | `claude-code` | `--model <m> --dangerously-skip-permissions` |
| `ahu codex` | `codex` | `-m <m> --dangerously-bypass-approvals-and-sandbox` |
| `ahu agy` | `antigravity` | `--model <m> --dangerously-skip-permissions` |
| `ahu opencode` | `opencode` | `--model <provider>/<model> --auto` |

`<m>` is the first entry for that harness in the project configuration's
`[model_rankings]`. The model option comes from the same mapping the headless
adapters use, so a coordinator and a registered agent on one harness cannot end
up naming the model with different flags. Each shortcut prints its whole argument
list, model included, before starting, so the disclosed line is exactly what was
launched.

Rankings may include harnesses absent from `harness_preferences`: explicit
coordinator shortcuts still use those rankings. Only automatic selection walks
`harness_preferences`. Ranking a moving model alias fixes the requested name,
not the provider's underlying model revision; see the [OpenCode model
limitations](#opencode-with-ollama-hosted-models).

A harness with no ranked model launches with **no model option at all**: ahu
prints a note that the harness default is in use and starts the session on
whichever model the harness is configured with. ahu does not substitute a model
the project never agreed to.

These shortcuts explicitly request the harness permission bypass; any outer
sandbox still applies.

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

`--dangerously-skip-permissions` is the Antigravity CLI's unattended mode.
`--auto` is OpenCode's only permission-widening flag and the only way an
`ahu opencode` session can match the other three: it approves every permission
OpenCode does not **explicitly deny**, without asking. Permissions set to `deny` in
OpenCode's own configuration or agent frontmatter still apply, and ahu cannot
widen past them. OpenCode's plugins retain their native loading behavior; ahu
passes no `--pure` here.

`ahu eval run --decision-evaluator` opts into rubric grading by the configured
typed decision provider. It sends case evidence, candidate output, and rubric
instructions to that provider and conflicts with `--evaluator` and
`--evaluator-repo`. Requests are checked for every case before candidates run;
provider failures remain failed judgments with unknown usage and no fallback.
See [Typed decision evaluator](evaluations.md#typed-decision-evaluator) for
request bounds, fingerprints, timing, and evaluator telemetry coverage.

## Local harness account bindings

Auth profiles are project-local and Git-ignored. They are stored under the
primary checkout's owner-only `.ahu/state/repos/<repo-identity>/` directory,
separate from committed `.agents/ahu/lock.md` because account identities are private.
Each profile contains a fingerprint per bound provider, not account metadata,
tokens, or API keys. A content digest detects edits outside the auth CLI;
changes should go through `bind`, `select`, or an explicit `--replace`. This
detects accidental or direct edits that did not update the digest; same-user
code can still rewrite local state, so this is not a tamper-proof boundary.

`ahu auth bind --harness ID` records the current identity in the active profile.
Supported IDs are `codex`, `claude-code`, `antigravity`, and `ollama`. Use
`--profile NAME` to bind another named profile, then
`ahu auth select --profile NAME` to make that profile active for the project.
`ahu auth status --harness ID` checks the active profile; add `--profile NAME`
to inspect another. `--replace` deliberately changes that provider's binding
inside the named profile.

`ahu auth readiness --harness ID [--model MODEL] [--output json]` provides a
concise readiness summary; JSON returns stable machine-readable fields for
automation.
The JSON includes `schema_version`, the requested `harness` and optional `model`,
`identity` and `binding` states, and `ready`; it never
includes principal values, credential material, or raw provider diagnostics.
`ready: false` means identity binding is unsupported, unavailable, absent, or
mismatched. A project without profiles reports `binding: "not_configured"`
and `ready: false`, even though fresh launches can proceed without the optional
binding policy. Readiness checks do not spend a model request and do not establish
quota, trust, or tool-approval state.

`ahu auth budget [--output json]` and the MCP tool `ahu_auth_budget` provide a
read-only, secret-free rate-limit view for the active project profile. The MCP
tool can also return registered agent candidates that meet a requested
`minimum_remaining_percent`; unknown capacities are not eligible. The JSON
contains the profile label and provider status, plus supported windows with
`used_percent`, `remaining_percent`, `window_duration_minutes`, and the provider's
reset timestamp. It never contains account identifiers, credentials, prompts, or
raw provider responses. Codex is currently the only provider with a supported
machine-readable view: ahu requests `account/read` and
`account/rateLimits/read` in one native app-server session, verifies that account
against the active profile fingerprint, and refuses to return capacity on a
mismatch. The reported percentage and time window are rate-limit signals, not
literal token balances, monthly credits, or per-agent allocations. Agents using
the same Codex account share those windows. Other providers return unsupported
or unknown capacity; ahu does not estimate it from run telemetry, scrape a TUI,
or send an inference request. The MCP tool's name and outcome join ordinary MCP
telemetry, while its arguments and result values are not added to OTel.

The view reports rows for `codex`, `claude-code`, `antigravity`, `kiro`, and
`ollama`. Only a bound Codex account is probed for capacity. Bound Claude Code,
Antigravity, Kiro, and Ollama accounts report `status: "unsupported"` with
`signal_type: "unknown"`; ahu has no supported capacity signal for them.
OpenCode has no row, so an agent on that harness is never an eligible routing
candidate. Each row carries one status:

| Status | Meaning |
| --- | --- |
| `available` | At least one window was read for the verified account, and every reported window is usable |
| `unknown` | The provider was probed but reported no usable windows, or a reported window was malformed or stale |
| `unsupported` | ahu has no machine-readable rate-limit view for this provider |
| `not_bound` | The active profile exists but holds no binding for this provider |
| `not_configured` | The project has no active profile to check against |

Only `available` rows can make a candidate eligible. Codex exposes `primary`
and `secondary` windows when its account reports them; a reported window
reflects the account, not a per-task or per-agent allowance. A mismatch between
the current Codex sign-in and the active profile fails the whole command rather
than returning capacity for an unverified account.

For Ollama, ahu reads `POST /api/me` from the loopback server named by
`OLLAMA_HOST` (default `127.0.0.1:11434`). It requires a signed-in account and
binds the account email and stable account ID. The confirmation identifies the
account but never prints credentials. Remote Ollama servers are rejected so
identity checks do not send account requests to an untrusted endpoint.

Once a profile exists, ahu verifies the selected provider against the active
project profile before creating a launch worktree. Every registered agent,
worktree, and child task in that project inherits the same active profile;
agents cannot select a different profile. Interactive task startup and
headless attempts recheck immediately before process start. Each task stores
the profile name and an identity fingerprint, never raw identity data.
With profiles configured, every `ahu resume` checks both the active profile and
the task's original profile and fingerprint before starting the harness or
sending its prompt. This check is especially useful after a long pause, when a harness may
have been signed out and back in to another account. Selecting or rebinding a
profile cannot move an existing task to another account. A project with no
profiles configured retains its prior interactive and headless behavior. Tasks
pinned before profile names were added have no profile pin and must be submitted again to resume under this profile
policy.

Codex and Claude Code expose read-only identity metadata ahu can use. Ollama
exposes its signed-in account through the local `/api/me` endpoint. For
OpenCode, ahu applies this check only when the selected model is `ollama/...`
and marked `:cloud` or `-cloud`; it checks the local Ollama account before
launch and again before resume. This assumes OpenCode routes that model to the
same loopback daemon configured by `OLLAMA_HOST`; ahu does not yet verify the
effective provider address. Local Ollama models do not have a cloud account identity
that ahu can bind. When project auth binding is enabled, ahu refuses OpenCode
models for which it cannot establish the provider identity. Other OpenCode
providers lack a verified identity check and are refused once
the project has opted into auth profiles. A custom OpenCode provider `baseURL`
can invalidate the loopback assumption: a matching Ollama fingerprint does not
establish which endpoint OpenCode uses. See the
[OpenCode provider configuration](https://opencode.ai/docs/providers/) for its
provider and base address configuration.

Codex API-key mode does not expose a verified user principal. Antigravity has no
standalone account-status command, but its startup TUI displays the signed-in
email before the prompt. ahu captures that startup display in a bounded
pseudo-terminal and terminates it without submitting a prompt. The probe sets
`NO_COLOR=1` for that capture only, because terminal color escape sequences
split the email apart and defeat extraction. After the email appears, ahu keeps
capturing until the output has been quiet for 500 ms, even if a recognized
auth-mode marker has arrived; the overall probe remains bounded to 12 seconds.
This allows fragmented email, auth-mode, and GCP project fields to arrive before
fingerprinting. The quiet interval is a heuristic, not a native startup-complete
signal: fields delayed longer than that can still be missed. This check is
version-sensitive to the visible banner; if the account email is absent, the
startup format changes, or no account line arrives
within 12 seconds, ahu reports readiness as unavailable and refuses to bind or
resume. A startup that reports an API key rather than a signed-in account is
refused as well: an API key exposes no user principal to bind. Turning color
off does not establish that every Antigravity version prints a banner ahu can
read; verify the pair with
`ahu auth readiness --harness antigravity` rather than assuming it binds.
This is best-effort identity checking at launch and resume, not an atomic
credential lock. ahu does not monitor sign-in changes during an active session,
and a same-user process could change sign-in after a check. The check reduces
the chance that a long-paused task resumes under a different account; it cannot
guarantee the identity used for every provider request. These bindings identify
the currently active native login; they do not switch accounts or isolate
concurrent accounts. Binding, readiness, launch, and resume checks do not probe
provider quota, predict request billing, change project trust, or
grant tool approvals. Reading rate-limit windows is a separate, explicit
request through `ahu auth budget` and its MCP tool; no launch path calls it.
Provider quota and entitlement errors are reported by
the harness during an actual request. Remote hosts, containers, and CI need a
separately provisioned native login and are outside this local profile guard.


## Kiro CLI

Install Kiro CLI using the [official installer](https://kiro.dev/docs/getting-started/installation/),
then run `kiro-cli login` and complete sign-in in your browser. ahu does not
install the harness or handle its credentials. Check the installation with
`kiro-cli --version` and available models with
`kiro-cli chat --v3 --list-models --format json`.

ahu supports interactive Kiro V3, verified on CLI 2.28.0. `ahu kiro` opens a
coordinator with `chat --v3 --model <ranked-model> --trust-all-tools`. As with the
other coordinator shortcuts, this explicitly widens approvals; Kiro also uses
that flag to approve hook confirmation requests. Kiro may show its own startup
trust-all confirmation; ahu does not suppress it or change its saved preference.
With no project ranking it
uses Kiro's native model default and reports that fact.

Registered agents use `harness: kiro`, an exact catalog model such as
`claude-sonnet-4.5`, and `permissions: prompt` or `permissions: auto`. Prompt
mode leaves native approval settings in control; auto adds `--trust-all-tools`.
Accept-edits is refused because ahu has no validated equivalent. The adapter
passes instructions and the assignment as one positional argument. It omits
`--` because Kiro 2.28.0 V3 dropped the initial prompt after that separator in
live verification; empty or option-shaped composed prompts are refused.
ahu checks that Kiro lists the exact requested model before launch, with no
fallback to `auto` or another model. Model listing can initiate login in Kiro,
so ahu checks native sign-in first and bounds both probes.

`ahu setup` detects `kiro-cli`, adds the ahu MCP server to
`.kiro/settings/mcp.json`, installs bundled skills under `.kiro/skills/`, and
creates an inline ahu agent manifest. Native JSON agent definitions are not
imported by `ahu onboard`; write the ahu manifest's instructions inline instead.
`.kiro/` and `.kiroignore` are recognized context, included in `.agents/ahu/lock.md` and
inherited into task worktrees. Global, synchronized, and harness-managed
configuration remains outside that lock. Native discovery and the actual
loading of skills, steering, hooks, and MCP remain Kiro's responsibility.
See [Kiro configuration scopes](https://kiro.dev/docs/configuration/).

`ahu auth readiness --harness kiro` and account bindings support the verified
GitHub social-login `whoami --format json` interface. Other login types and
API-key account binding are currently unsupported. Account checks do not prove
model entitlement or remaining credits.

Kiro V3 2.28.0 and 2.29.0 are admitted for ahu headless fresh runs and native `--resume-id`.
The adapter passes `--no-interactive --output-format stream-json
--require-mcp-startup`. Only `runFinished` with success and `end_turn` completes
an attempt. An ordinary failed tool can be followed by a successful result;
permission denials, unresolved calls/approvals, task errors, malformed streams,
missing results, interruption and nonzero exit cannot become success. Replayed
history does not determine the current attempt's outcome. MCP startup failures stop the launch.
Intermediate turn completion does not finish a task. Kiro credits are not USD;
USD cost, token and skill-invocation metrics remain unknown. Eval trajectory
records bounded ACP tool-call, completion and error counts, deduplicated by
`toolCallId`. Coverage remains partial: model steps, stable tool identities,
repetitions, recovery and native-helper attribution are unknown. Tool titles are
display text, not proof of tool identity or delegation.

Session-info events also supply partial metering under `trajectory.kiro_metering`:
latest/peak context-window percentage, observed prompt completions, reported
credits and elapsed milliseconds, plus observation, duplicate and rejection
counts. A prompt completion may include multiple model requests; it is not a
model step. Context snapshots are not billable token counts, and credits are not
USD. Live prompt and turn markers gate completion attribution, and replayed
history does not contribute to current-attempt totals. Missing amounts remain
unknown; accepted observations never certify complete session or helper usage.
Native Kiro OTLP export remains separate from ahu/MCP eval capture because the
observed native resource does not carry ahu task-and-attempt attribution.
Shared ahu supervision supplies foreground/background execution, timeout,
cancellation, result records, and resume. Histories remain in Kiro's native home.

Headless admission inspects native JSON and Markdown agent profiles in project,
user and ancestor scopes. Ordinary prompts, tools, models, file/skill resources,
permissions and inspected MCP fields are accepted without rewriting the profiles.
Markdown requires YAML frontmatter; JSONC, unknown fields, nonempty hooks,
Powers activation and knowledge-base resource objects remain unreviewed.
Available profiles remain native: ahu does not select them with `--agent` or
claim they were loaded.

The inspected CLI preferences are `chat.defaultAgent`, `chat.defaultModel`,
`chat.greeting.enabled`, `chat.disableWrap`, `chat.preserveScrollback`, and
`chat.startFullscreen`. Other custom settings, local/global hooks, extensions,
Powers, workflows, cloud configuration, custom native paths and diagnostic-output
overrides remain refused. Project ancestors and cloud-cache sources are inspected
as well. Symlinks, malformed/oversized configuration, scan limits and versions
other than 2.28.0 or 2.29.0 produce a refusal; ahu does not delete or rewrite them. Native permissions, MCP and skills remain active.
This is bounded configuration inspection, not a sandbox. Native cmux hook
installation, detailed hook inventory and bounded helpers remain unsupported;
`--native-helpers disabled` is a prompt instruction, not a native tool restriction.

This repository registers `@dev-kiro` for implementation, `@review-kiro` for
read-only review, and `@test-kiro` for reproduction and regression tests. All pin
`claude-sonnet-4.5`. Development and testing use auto approvals; review keeps
native prompt policy (which is not enforced read-only access). For example:

```sh
ahu @dev-kiro --headless --allow-widened-approvals --prompt "Implement the agreed change and validate it"
ahu @review-kiro --headless --prompt "Review the current change for regressions"
ahu @test-kiro --headless --allow-widened-approvals --prompt "Add regression coverage for the agreed defect"
```

Add `--background` for asynchronous work, then use `ahu wait <task-id>`,
`ahu result <task-id>`, or `ahu cancel <task-id>`. Use `ahu resume <task-id>
--prompt-file /tmp/continuation.txt` to continue its native session.
A successful harness result is execution evidence, not acceptance of the work.


### Live Kiro compatibility checks

Run the opt-in matrix after building ahu and signing into Kiro:

```sh
python3 scripts/check-kiro-compatibility.py \
  --ahu /absolute/path/to/ahu --allow-widened-approvals
```

This consumes model credits and authorizes automatic tool approvals only for
fixture agents that request them. The script creates an external repository
under `/tmp` and attempts fresh execution, resume, tool-error recovery,
permission denial, timeout and cancellation. The fresh-run row requires a
successful `ahu_agents_list` MCP observation, captured MCP spans, a correct
answer, and Kiro metering; merely starting the script or configuring MCP/OTel
does not pass it. The script writes its report outside the source checkout and retains the fixture for
inspection and attempts to stop its owned running tasks. A native profile in
the fixture checks configuration admission; that row does not certify native
helper invocation or isolation. Global native configuration and provider state
still apply, so a refusal or unavailable provider is a failed compatibility row,
not evidence that the adapter passed. The report identifies the binary digest,
source revision, source dirtiness, Kiro version, exact model and headless mode.

See [shared project documents and explicit migration](project-documents.md) for OKF schemas, legacy compatibility, and recovery.

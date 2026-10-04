# MCP protocol reference

[Back to the CLI and context reference](reference.md#mcp-integration).

This page documents the stdio protocol and task extension implemented by `ahu mcp serve`.

The modern path targets the [2026-07-28 MCP specification](https://modelcontextprotocol.io/specification/2026-07-28)
and its [Tasks extension](https://tasks.extensions.modelcontextprotocol.io/specification/2026-07-28/tasks).
Modern stdio requests do not use `initialize`: every request carries
`io.modelcontextprotocol/protocolVersion: "2026-07-28"` and an object-valued
`io.modelcontextprotocol/clientCapabilities` in `params._meta`. `server/discover`
returns `resultType: "complete"`, supported versions, capabilities, cache hints,
and server identity under `_meta["io.modelcontextprotocol/serverInfo"]`.

The synchronous `ahu_auth_budget` tool accepts no arguments. It returns the
active project's profile label and per-provider rate-limit status; supported
windows expose percentages and reset periods, not token counts. Codex capacity
is returned only after ahu verifies the current account against the active
profile in the same native app-server session. Other providers may be marked
unsupported or unknown. The tool neither reserves capacity nor assigns tasks,
and its result does not include identity values or raw provider responses.

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

`ahu_request_approval` is an explicit task-bound checkpoint tool. Its
arguments are limited to an operation category, a short summary, and an
optional target. The stdio server requires matching `AHU_TASK_ID` and
`AHU_TASK_DIR` values from an ahu-managed process, verifies that the task is
live in the current checkout, and waits for `ahu approve TASK` or
`ahu reject TASK`. The task record enters `waiting-for-approval`; rejection,
operator cancellation, or timeout requests cancellation. A task has at most
one pending approval request. CLI decisions refuse task or worker context;
same-user code can remove those markers, so the check is cooperative. This tool is not a general-purpose
interceptor for shell commands or a replacement for operating-system
filesystem isolation.

The stdio binding is newline-delimited UTF-8 JSON-RPC: each line is one request,
notification, or response, and stdout contains no other bytes. Diagnostics go
to stderr. Frames are limited to 1 MiB, including the newline; an overlarge
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
stored final tool results carry `resultType: "complete"`. Modern discovery and
all tool lists also carry `ttlMs: 0` and `cacheScope: "private"`, explicitly
disabling caching across changing repository state and client capabilities.
Legacy tool lists omit these modern-only fields.

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

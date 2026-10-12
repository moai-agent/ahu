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

The synchronous `ahu_auth_budget` tool accepts an optional
`minimum_remaining_percent` from 0 to 100. It returns the active project's
profile label and per-provider rate-limit status; supported windows expose
percentages and reset periods, not token counts. Codex capacity is returned
only after ahu verifies the current account against the active profile in the
same native app-server session. Bound Claude Code, Antigravity, Kiro, and Ollama
accounts report unsupported capacity with an unknown signal type; OpenCode has
no provider row. The `routing.candidates` list matches registered agents to their
harness windows and marks candidates `eligible` only when every reported
window meets the requested threshold. Unknown provider capacity is never
eligible. This is a quota filter, not a cost estimate, model-availability
check, or reservation; agents sharing an account still share its capacity.
The result does not include identity values or raw provider responses.

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
true/false values, arrays, objects, or fractional numbers. String IDs are limited
to 256 UTF-8 bytes; longer IDs receive `-32600` with a null ID. The server also
limits each serialized response frame, including its newline, to 1 MiB. If a
response would exceed that bound, it sends a small `-32603` error with the
original bounded ID when available, or a null ID when it cannot safely echo one.
Oversize outbound notifications are suppressed because they have no request ID
to correlate an error with. This server sends no requests to clients and does
not accept response envelopes.
Method parameters, when present, must be objects (`-32602` otherwise).

An envelope with no ID is a notification, regardless of its method name. Valid
notification envelopes receive no response, even with unknown methods, invalid
parameters, or absent modern metadata. Supported inbound notifications are
advisory no-ops: notifications never select a protocol mode, queue inspections,
cancel Tasks, or change subscriptions. Use requests with IDs for those operations.
A `notifications/*` method sent with an ID receives `-32601`.

Unknown methods receive `-32601`. Unknown tools, missing/non-string tool names,
and invalid tool arguments receive `-32602` in both synchronous and Tasks paths,
before any handle is created. Arguments must be objects matching the advertised
schema: agent/task list tools accept no keys, `ahu_task_get` requires `task`, and only the
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

## Local Skills extension

The modern stdio path implements the local subset of the public
[Skills extension (SEP-2640)](https://github.com/modelcontextprotocol/ext-skills/blob/main/specification/stable/skills.mdx)
for the existing `2026-07-28` base protocol. `server/discover` advertises
`resources: {}` and `extensions["io.modelcontextprotocol/skills"]: {}` when
at least one installed bundled skill verifies. Legacy `initialize` connections
do not advertise or serve Skills. Each `skills/list` and `skills/get` request
must declare `io.modelcontextprotocol/skills: {}` in the modern client
capabilities metadata. Standard resource reads do not require that extension.

`skills/list` returns `skills` and an optional opaque `nextCursor`; pass it as
`cursor` to continue. Pages contain at most two entries. Cursors are bound to
the verified inventory and must be discarded if a file changes or disappears.
`skills/get` takes `uri` and returns `skill`. Both results carry
`resultType: "complete"`, `ttlMs: 0`, and `cacheScope: "private"`.
Each skill exposes its `ahu://skills/<name>/SKILL.md` URI, unchanged parsed
`frontmatter`, and a complete `resources` array with raw-byte
`sha256:<64 lowercase hex>` digests and byte sizes. Standard `resources/list`
also lists these files, and `resources/read` takes their exact `uri` and
returns Markdown text in `contents`. Directory reads, subscriptions, and
`directoryRead` are not advertised or implemented.

Only the four distributable skills in the binary's `BUNDLED_SKILLS` allowlist
are eligible, at `.agents/skills/<name>/SKILL.md`. Each request verifies
frontmatter and the installed file's digest against the embedded bytes before
exposing it. Missing, malformed, changed, inaccessible, oversized, or symlinked
copies are omitted; lookup of an omitted resource fails. Reads are limited to
64 KiB per file, require regular files, and open every relative component with
no-follow descriptor operations. The response uses the bytes already verified,
without reopening the path. It is a verified content snapshot, not a guarantee
that the filesystem stays unchanged after a request.

The current distributed bundle contains only `SKILL.md`; its complete resource
array therefore contains one file. Adjacent scripts, references, Markdown links
outside the skill, user skills, and remote content are never added implicitly.
Supporting files would need explicit inclusion in the trusted bundle before
being served. The server never resolves client resource identifiers into arbitrary paths,
fetches content, executes scripts, installs skills, or changes permissions.
`_meta["io.github.moai-agent.ahu/skillOrigin"]` retains the bundled origin,
server version, relative local path, and content digest. `executionObserved`
is false: listing or retrieval is not evidence that a harness loaded, followed,
or executed a skill. Clients must preserve the server origin when presenting
skills; an MCP resource identifier does not replace a harness's own local skill precedence.

When MCP telemetry is enabled, these retrieval methods use the existing bounded
request spans: method, outcome, and a fixed error category on failure. Skill
text, frontmatter, resource identifiers, content digests, and origin metadata are not
exported. These observations describe retrieval only, not harness execution.

## Tool-only skill compatibility

Clients that expose only ordinary MCP tools can use the read-only
`ahu_skills_list` and `ahu_skills_get` tools on either the legacy or modern
connection. They require no Skills or Tasks extension capability. They remain
synchronous even when the client advertises Tasks. The standard Skills and
resource methods above retain their negotiation and response contracts.

`ahu_skills_list` accepts an optional opaque `cursor` (1–128 UTF-8 bytes).
It returns the same `skills`, two-entry pages, inventory-bound `nextCursor`,
digests, resource metadata, and sanitized provenance as `skills/list`.
`ahu_skills_get` requires an exact bundled `ahu://skills/<name>/SKILL.md` URI
(at most 256 UTF-8 bytes). It returns `skill` plus `contents`, containing the
Markdown bytes from the same verified snapshot. No separate resource call is
needed. Arguments must be objects; additional keys, including argument-level
`_meta`, are rejected. Invalid argument shapes or non-bundled URI values produce
`-32602`; invalid or stale cursor values, and unavailable verified content, produce
ordinary tool errors with `isError: true`.

Both tools return JSON in `structuredContent` and equivalent JSON text in
`content`. The payload carries `resultType: "complete"`, `ttlMs: 0`, and
`cacheScope: "private"`. The outer tool result also disables caching, including
on tool errors; modern outer results carry `resultType: "complete"`.
Serialized compatibility payloads are limited to 128 KiB before the MCP
text/structured duplication. The existing two-entry pagination and 64 KiB
per-file read limits still apply. The tools share the bundle verification,
local-file restrictions, supporting-resource contract, and payload-free
telemetry described above. Retrieval never establishes that a harness loaded,
followed, or executed the skill.

## Interoperability matrix

| Dimension | ahu bundled skills | Hugging Face MCP Server |
| --- | --- | --- |
| Operations | `skills/list`, `skills/get`, `resources/list`, and `resources/read`; no directory reads. Tool-only clients can use `ahu_skills_list` and `ahu_skills_get`. | `skills/list`, `skills/get`, `resources/read`, and optional `resources/directory/read`. A skill manifest identifies its resources; content is retrieved separately. |
| Transport | Local MCP stdio process. Standard Skills methods require modern protocol negotiation. The two compatibility tools work on legacy and modern tool connections. ahu does not expose a remote HTTP MCP endpoint. | Skills use the `Streamable HTTP` transport; the long-lived STDIO transport does not expose the Skills extension. |
| Authentication | No remote service or Skills credential is used. Access follows the local process and project permissions that launch ahu. | Access is determined by the endpoint's deployment. The catalog is supplied through the server's local `HF_SKILLS_DIR`; ahu neither stores nor forwards Hugging Face credentials. |
| Catalog freshness | The catalog is embedded in the ahu binary. A local skill is available only while its installed bytes match that embedded snapshot; updates arrive with a new ahu bundle. The server does not refresh from a remote source. | The server documents a three-hour snapshot TTL. A stale request uses the last verified snapshot while a background refresh runs; refresh failures retain that snapshot and delay the next retry. |
| Content verification | The installed `SKILL.md` must match the embedded digest. Only exact bundled skill URI values are served; neighboring files and linked resources are not followed or fetched. | The server verifies snapshot SHA-256 digests and actual `SKILL.md` frontmatter before retaining files. Its manifest can describe nested supporting files and directory reads. |
| Client availability | Standard Skills methods require a client that negotiates and calls the extension methods. The tool fallback requires ordinary `tools/call`; a live interactive Codex call to both tools has been verified. Other harness clients remain unverified. | A client must reach a `Streamable HTTP` endpoint and support the methods it wants to use. Availability and authentication vary by deployment. |

The fallback tools are a client-compatibility path to the same local ahu catalog;
they do not connect to the remote Hugging Face server or make it a local-use
dependency. Choose a path according to client capability: use the standard
Skills methods when the client exposes them, otherwise use ahu's ordinary tools.
Both paths keep ahu's embedded digest as the trust root. No remote skill content
is accepted, and current ahu bundles advertise no
supporting resources beyond each `SKILL.md`. This is not a claim that every
supported harness implements SEP-2640, nor that retrieval means the harness
installed, loaded, or followed a skill. See the [Hugging Face MCP Server
documentation](https://github.com/huggingface/hf-mcp-server) for its current
transport and snapshot behavior.

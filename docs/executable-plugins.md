# One-shot executable plugins (protocol V1)

A package can declare a generic JSON entrypoint separately from its optional
[session-host executable](executable-hosts.md). The host library exposes
`ahu::plugin_exec::invoke` and the read-only
`ahu::plugin_exec::task_service::invoke` integration below. The foreground CLI
uses these transports without MCP registration, background activation or
TaskService callbacks. It is suitable for
an explicit host call to a trusted local package that processes supplied data.
It does not implement an execution provider or grant task mutation rights.

For a complete offline authoring example, see the independent
[text statistics package](../examples/plugins/text-stats/README.md), including
its executable, OKF manifest, local binding, and install/invoke/disable/remove flow.

Add this field to the existing `ahu-plugin.md` frontmatter:

```yaml
entrypoint:
  path: bin/example
  protocol_version: 1
  timeout_ms: 5000
```

The existing package identity, schema 1, Host API 1 and `cargo_manifest` fields
remain required. Cargo metadata describes package name/version; ahu never runs
Cargo to install or invoke a package. `entrypoint` rejects unknown fields and
requires all three fields. Protocol version must be 1 and the timeout must be
100–30000 milliseconds. Executables must be regular, executable, non-setid files
of at most 64 MiB. Paths must have at most 32 normal relative components;
absolute paths, symlinks at every component, empty components, backslashes and
`.`/`..` are refused. Inventory reads metadata and file attributes only.

Use the existing pinned package installation lifecycle. Invocation requires the
current **committed** binding to match ahu's version and contain both
`enabled: true` and `grants: [execute]`. An `interactive-backend` grant alone
does not authorize this entrypoint. The invocation API uses the verified installed
package, never `AHU_PLUGIN_PATH` or a remote source; it does not install missing
packages. Discovery, installation and activation never invoke the entrypoint.
An enabled binding with `execute` but no `interactive-backend` grant is excluded
from default session-host selection. Explicit interactive selection still needs
its separate host declaration and grant.

## Foreground CLI invocation

```sh
ahu --repo /absolute/checkout plugins invoke example --input-file /absolute/input.json
printf '%s' '{"text":"synthetic input"}' | ahu plugins invoke example --input-file -
ahu plugins invoke example --task-observations \
  --allow-task 00000000-0000-7000-8000-000000000001 \
  --input-file /absolute/observations.json
```

The package must already be installed and have a committed enabled binding with
an exact ahu version and `execute` grant. This command runs once in the foreground;
the host does not install, fetch, probe, retry, load provider-specific configuration or authenticate.
An explicitly authorized package may call a provider; see the optional
[GitHub issue inspection package](../plugins/github/ahu-plugin.md).
When valid project telemetry is enabled, ahu also emits one local `ahu.plugin.invoke`
span containing only the plugin ID, invocation mode, outcome and effect uncertainty.
It does not include input, output, repository paths or credentials. Missing or invalid
telemetry configuration does not prevent an otherwise authorized invocation.
Task/worker-marked processes are refused at the cooperative host-operator boundary.
This is not authentication against other code running as the same OS user.

These foreground invocations are operator-only and do not count as a task-attributed
provider action. The read-only task-observation mode does not authorize tracker
updates or other task mutations.

`--input-file` is required. `-` explicitly selects non-terminal stdin; relative
file paths resolve after global `--repo` selection. Named inputs must be regular
files, opened without following a leaf symlink, never named pipes or devices. Input
reads stop at 65,537 bytes and reject anything exceeding 64 KiB. stdin intake has
a fixed 30-second deadline. SIGINT/SIGTERM request cancellation during intake,
collection and execution. The command temporarily ignores SIGPIPE so a closed
output reader produces a fixed `io` error; prior signal dispositions are restored
on exit. Filesystem and Git operations cannot be interrupted;
package verification and collection precede the separate manifest process timeout.

A successful stdout document is compact JSON followed by a newline:

```json
{"schema_version":1,"status":"ok","result":null}
```

Runtime failures leave stdout empty and write one fixed JSON document to stderr:

```json
{"schema_version":1,"status":"error","code":"denied","outcome_may_be_unknown":false}
```

| Code | Exit status |
| --- | --- |
| `invalid_input` | 2 |
| `denied`, `unavailable` | 4 |
| `cancelled`, `timed_out`, `output_limit`, `protocol`, `failed`, `io` | 5 |

Argument syntax errors use static usage diagnostics and exit 2. Success exits 0.
The complete stdout document, including framing, JSON escaping and newline, must
fit in 64 KiB. Non-ASCII characters and terminal controls are JSON escaped without
changing their values. Overflow fails before writing stdout. Delivery uses bounded
nonblocking I/O with a separate 30-second deadline per output stream. If stdout
fails or stalls after a partial write, those bytes cannot be recalled; the error
has `outcome_may_be_unknown: true`. Post-execution failures always retain this
uncertainty, including CLI framing overflow. Error output never includes input,
plugin data, task data, paths or OS diagnostics. Successful result data is released
intentionally; it is not secret-filtered or interpreted as an action.

`--task-observations` accepts the request shape in the next section. Repeat
`--allow-task UUID` to supply an independent maximum scope (up to 128 unique,
canonical UUIDs). Scope flags are invalid in generic mode. No grants means an
empty scope, never all tasks. Prefixes, handles, duplicate grants and operations
other than list/read/replay are refused. Every operation and list cursor is
validated against the full scope before any task read. Only read/replay IDs and
IDs on requested list pages are loaded; unused missing or corrupt granted tasks
and unrelated records do not block a narrow request. Capabilities-only requests
and empty pages do not access task storage.

CLI observations support current schema-2 pointers in the primary-owned task
index, with validated checkout/store ownership and readable task record schemas.
Tasks absent from the index and legacy external indexes/stores are unsupported
(`unavailable`); there is no enumeration, repair, migration or registration fallback.
Inspect `ahu task UUID` or `ahu tasks` separately for legacy recovery diagnostics.
Matching durable V1 observations retain revision/replay semantics; conflicting
identity fails closed. Without durable V1 state, observations synthesize revision 1
and replay is local to this invocation, not execution history that can be resumed
after reconnecting.
Collection never creates V1 directories or locks or reconciles task state.
Snapshots are coherent per task, not a repository transaction. Returned plugin
JSON cannot dispatch Submit/Cancel/Input or any task action.

## Trusted host call

The embedding host supplies a repository-bound `HostGrant` and a minimal,
allowlisted invocation context. This type has private fields and cannot be built
from an untrusted JSON document. Its constructor belongs in trusted host code,
not a request handler that accepts caller-selected identities or scope.

```rust,ignore
use ahu::plugin_exec::{HostGrant, invoke};
use std::sync::atomic::AtomicBool;

// `repo` is the repository selected by trusted host code.
let grant = HostGrant::new(&repo, "example", br#"{"invocation_id":"example-1"}"#)?;
let cancelled = AtomicBool::new(false);
let result = invoke(&repo, &grant, br#"{"text":"synthetic input"}"#, &cancelled)?;
```

Context accepts exactly one required field: `invocation_id`, a host-chosen opaque
identifier of 1–64 lowercase ASCII letters, digits, hyphens or underscores. Raw
context JSON is bounded to 256 bytes. Unknown fields (including credential, path
and arbitrary metadata fields), path separators and other identifier characters
are rejected before activation. The host must use a synthetic correlation ID,
never an encoded credential or sensitive path; syntactic validation cannot prove
that an otherwise valid identifier has no secret meaning.

Input must be valid JSON of at most 64 KiB, and the serialized request envelope
must also fit within 64 KiB. Duplicate object keys at any depth, trailing JSON and excessive
JSON nesting are refused. The input payload remains trusted host-supplied data:
callers must explicitly authorize its release and exclude credentials and sensitive
paths. It is not a secret detector. Treat returned values as untrusted data, not
instructions or grants.

## Read-only task observation calls

`ahu::plugin_exec::task_service::invoke` is the core path for sending task
observations to an enabled executable. It uses the generic protocol unchanged:
`input` contains an `ahu.task-observations` V1 payload. The protocol has no
duplex remote procedure call or callback endpoint. The executable cannot request
more data after startup.

Core constructs a `TaskReadGrant` from a `HostGrant`, an existing `Controller`,
and an explicit set of canonical ahu task UUIDs. The controller's repository
must match the host grant. The plugin principal is minted from the host grant's
plugin ID; observation requests cannot supply identity, scope or operations beyond
list/read/replay. The grant is opaque and cannot be created by parsing untrusted
input. Only trusted core code may construct it; accepting plugin-chosen grants through an untrusted request
handler would defeat this boundary. The controller stays in core.

```rust,ignore
use ahu::plugin_exec::{HostGrant, task_service::{self, TaskReadGrant}};
use std::sync::atomic::AtomicBool;

// `repo` and `core` are the host-selected Repo and existing Controller.
// Core has already admitted the tasks through normal launch policy. This call
// neither registers tasks nor opens/discovers arbitrary repository task records.
let host = HostGrant::new(&repo, "example", br#"{"invocation_id":"read-1"}"#)?;
let grant = TaskReadGrant::new(host, &core, [
    "00000000-0000-7000-8000-000000000001".to_owned(),
].into())?;
let result = task_service::invoke(&repo, &grant, br#"{
  "version":1,
  "observations":[
    {"operation":"list","limit":1},
    {"operation":"read","task":"00000000-0000-7000-8000-000000000001"},
    {"operation":"replay","task":"00000000-0000-7000-8000-000000000001","after":0}
  ]
}"#, &AtomicBool::new(false))?;
```

The strict request has exactly `version` and `observations`. Each observation is
one of `list` (`limit`, optional/null `after` task ID), `read` (`task`), or `replay`
(`task`, required `after` revision). Unknown fields, duplicate keys, trailing
JSON, excessive nesting, unsupported versions and other operations are refused.
Capabilities are always included, so an empty observations array is a
capabilities-only request. Empty task scopes are valid and list no tasks.

The request may also contain an optional `data` JSON value. Core forwards it
unchanged to the executable as `data` alongside the observations. This lets an
operator supply provider-specific inputs, such as a selected local KB root, in
the same call that carries scoped task observations. It does not change task
identity, grants, capabilities, or allowed operations; the core independently
authorizes every observation before loading task data. Provider input remains
untrusted, is subject to the same 64 KiB request/payload/envelope limits, and
must not contain credentials or sensitive values unless the operator explicitly
authorizes their release to that executable. The host does not infer, filter, or
validate provider-specific fields.

The generic envelope still identifies the host-selected plugin and contains
`grant: {"operation":"invoke"}`. Its input has exactly these fields:

```json
{
  "kind":"ahu.task-observations",
  "version":1,
  "capabilities":{
    "version":1,
    "operations":["list","read","replay"],
    "max_input_bytes":65536,
    "replay_window":128
  },
  "observations":[{
    "operation":"read",
    "snapshot":{
      "version":1,
      "task":"00000000-0000-7000-8000-000000000001",
      "revision":1,
      "lifecycle":"prepared",
      "cancellation_requested":false
    }
  }]
}
```

When the request supplies provider data, the input includes one additional
`data` field containing that value. If it is omitted, the field is omitted from
the payload so existing V1 consumers receive the same shape.

Observations retain request order. `read` contains `snapshot`; `list` contains
`tasks` (snapshots in ascending task-ID order) and `next_after` (an exclusive
cursor or null); `replay` contains `snapshot` and `changes`. Each change contains
only `revision`, `kind`, `lifecycle`, and `cancellation_requested`, with the V1
TaskService meanings. A snapshot's fields are exactly those shown in the preceding
example: full service identities, owners, source paths, prompts, digests,
argument vectors, native credentials, effect receipts and repository records are
omitted. The task ID is an ahu UUID, never an external work-item ID. External
associations and any subsequent writes require separate host policy; this API accepts and persists
no tracker identity or mapping. Returned JSON is untrusted data and never
triggers task or tracker operations, even if it resembles a mutation request.

Bounds and failure semantics:

- At most 128 granted task IDs and 128 requested observations per invocation.
  IDs must be canonical UUIDs; prefixes and `ahu:task:` handles are not expanded.
- List limits are 1–128. A list cursor must itself be granted. Read/replay require
  exact granted IDs; IDs or cursors absent from the granted list are denied before lookup.
  Only the IDs on a requested page are read. No repository enumeration occurs.
- Replay includes at most the last 128 coordination changes, strictly after the
  supplied revision. Expired and future revisions fail; refresh with a separately
  authorized read. This is lifecycle metadata, never execution history.
- Request JSON, projected input, the complete generic envelope, and stdout each
  have a 64 KiB limit. Collection checks the projected payload after each
  observation using a bounded writer. Overflow fails without truncation or plugin
  startup. Envelope overhead can cause rejection even when input alone fits.
  The capability's `max_input_bytes` is the TaskService V1 limit, not a new input
  mutation grant; the stricter transport/envelope limits also apply here.
- Every snapshot is coherent and each replay pairs changes with its snapshot
  atomically. Multiple observations are collected sequentially, not as a
  cross-task or cross-observation transaction. Already collected input is fixed
  for that invocation. Invoke again to refresh; core must reauthorize scopes and mint
  a fresh grant when reconnecting. Old in-memory grants have no revocation feed.
- The current committed enabled/execute binding is checked before collection and
  again by generic invocation. Package and compatibility validation, cancellation,
  fixed failures, output bounds and process cleanup retain their generic behavior.
  A missing installation never causes automatic installation.
- Service denial maps to `Denied`; invalid requests, version/operation errors,
  expired replay and future revisions map to `InvalidInput`; missing records and
  other service/storage failures map to `Unavailable`. No service diagnostics or
  storage paths escape. Any collection failure prevents startup; no partial
  payload is delivered. Cancellation is checked between observations. Service
  reads and package verification precede the generic process deadline and are
  filesystem operations that do not support cancellation.

The library path works with existing memory or local controllers; the CLI uses
an immutable exact inspection. The library path adds no launch
adapter, legacy-record discovery/migration, task registration, effects operation,
Submit/Cancel/Input, CLI/MCP execution control, tracker client, network call or
credential lookup. Local service reads may acquire coordination locks but do not
change lifecycle, revisions or effect state. It is a data-release boundary, with
the same same-user executable limitations described below.

## Wire contract and process lifetime

ahu snapshots the verified installed executable into a private temporary
directory outside repositories, then starts it with this fixed argument vector:

```text
<private snapshot executable> --ahu-plugin-protocol=1
```

The host does not construct a shell command, select an executable through PATH,
extend the caller's argument vector, or probe. Working directory is the private snapshot directory.
The environment is cleared, then `PATH=/usr/bin:/bin` is set; no auth/environment
variables or repository configuration are forwarded by default. An explicit
committed `github-cli` binding grant forwards the listed native auth environment
without parsing it, so the trusted package and `gh` can use the user's existing
login. This is a deliberate credential-access grant, not credential isolation.
It additionally forwards `HOME`,
`XDG_CONFIG_HOME`, `GH_CONFIG_DIR`, `GH_TOKEN`, and `GITHUB_TOKEN` when present,
plus the caller PATH as `AHU_GITHUB_PATH` and the marker `AHU_GITHUB_CLI=1`.
This occurs only after authorization and installed-package verification; the
host never searches for `gh` or inspects the native credential store. Interpreter
PATH stays fixed. This grant gives the trusted package native GitHub authentication
access, not OS confinement or a read-only token scope. Disabled and uninstalled bindings
receive nothing. See the [package contract](../plugins/github/ahu-plugin.md) for
its bounded read-only calls and domain errors. stderr goes to the null
device, and no execution trace is retained. Snapshot bytes and directory are
removed on return. The binary must be self-contained apart from system runtime
libraries/interpreters; package-relative assets are not copied.

stdin carries exactly one UTF-8 JSON document and is then closed:

```json
{"protocol_version":1,"plugin_id":"example","grant":{"operation":"invoke"},"context":{"invocation_id":"example-1"},"input":{"text":"synthetic input"}}
```

stdout must contain exactly one document, close, and the process must exit zero:

```json
{"status":"ok","protocol_version":1,"result":{"text":"SYNTHETIC INPUT"}}
```

Or return a fixed error code (`failed`, `invalid_input`, or `unsupported`):

```json
{"status":"error","protocol_version":1,"code":"failed"}
```

Unknown envelope fields, duplicate keys, unsupported versions, multiple documents,
malformed JSON and nonzero exits fail. stdout is limited to 64 KiB total,
including whitespace. Input and output use concurrent nonblocking I/O so a peer
that writes before reading cannot deadlock the host. A single deadline covers
spawn, input, output and exit observation. Package verification/snapshot I/O
precede that deadline; filesystem access and OS process creation continue
independently of this token.

Another thread may set the supplied atomic cancellation token. A token already
set prevents activation; in-flight transport checks it at least every 20 ms
between bounded I/O steps. A response without EOF or process exit still times
out. On completion, cancellation or failure, ahu signals its owned process group
before reaping the leader, with a bounded 250 ms cleanup wait on failure. Group
cleanup prevents ordinary inherited-pipe descendants from outliving the call.
A process that ignores these signals can remain alive after the cleanup wait ends.

`Failure` exposes only a fixed `Code` and `outcome_may_be_unknown`. Child stderr,
error text, OS diagnostics and package contents are never included. Successful
`result` data is not redacted. Failures after spawn set the outcome flag: even a
malformed response or timeout may follow side effects. The host does not retry
automatically or claim to roll back.

This is a communication and lifecycle boundary, **not an OS sandbox**. An enabled
executable has the invoking user's OS rights and can access files, credentials
or networking on its own, or escape its process group. No network client, auth
lookup or provider-specific behavior is supplied by this slice. Use trusted
packages; hostile same-user mutation during verification is outside the existing
installation threat model. Strong filesystem/network isolation would require a
separate platform sandbox design.

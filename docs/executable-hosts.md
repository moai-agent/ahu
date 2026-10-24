# Executable session hosts (protocol V2)

For generic one-shot JSON processing without session-host methods, see
[executable plugins](executable-plugins.md). The two declarations and grants are separate.

ahu can explicitly activate a discovered local executable and speak Host V2 over
its standard input and output. The current terminal is the default. Providers such as cmux are installed as external plugins. Discovery with
`ahu plugins` reads metadata only. `ahu plugins --probe` executes the selected
host, negotiates compatibility, then terminates the connection. Neither command
creates an ahu task.

Executable hosts require an enabled committed OKF binding with an
`interactive-backend` grant and exact ahu compatibility. Discovery metadata and
manifest capabilities never grant execution. The selected package must match the
binding's source digest, identity and version. Installations resolve by their
content-addressed receipt; discovered packages are verified in place.

Before session startup, ahu persists `session_host` in the current task schema
(schema 6). Older task schemas are refused. The record freezes the committed
binding and revision, package name, executable declaration, executable SHA-256,
and protocol version. It stores a single opaque `session_ref`; cmux group, window,
and workspace identifiers remain inside the provider plugin. Task operations use
the frozen host identity, not the current selection. Focus and cleanup ask the
provider to verify ownership against the task's repository and worktree.
If a provider created a workspace but ahu did not persist its creation reply,
ahu can reconcile with an empty `session_ref` plus the exact repository identity
and worktree path. The host must return `owned` only for one exact, verified
match; no match is `absent`, and mismatched or ambiguous ownership stays blocked.
Any following mutation with an empty reference must repeat the same ownership
check. Empty is a recovery sentinel, never a wildcard.
Core retains harness startup, supervision, approvals, cancellation confirmation,
and worktree cleanup. Uncertain creation preserves task inputs and work.
Task focus and cleanup require the frozen host identity and verified ownership.
When a reference is recorded, the host checks that reference. When it is missing
after a possible create, ahu asks the pinned host to reconcile by repository and
worktree; a unique match can be acted on, while absent, mismatched, or ambiguous
ownership remains explicit. An uncertain create still preserves task inputs and
work until that reconciliation succeeds.

Updating a binding retains old installations and existing tasks keep their exact
version. The binding's enabled state and grants in the launching checkout still
apply: revocation, absent packages, incompatible policy and tampering refuse host
activation. A discovered package must remain available at its frozen location.
Headless execution and coordinators in the current terminal need no host.

## Package declaration and process resolution

Keep the local manifest fields described in the
[package reference](reference.md#local-session-package-discovery-and-selection)
and add this optional mapping inside the `ahu-plugin.md` YAML frontmatter
(before the closing `---` line):

```yaml
executable:
  path: bin/session-host
  protocol_version: 2
  timeout_ms: 5000
```

`path` has at most 32 normal relative components below the manifest directory;
absolute paths, empty components, `.`/`..`, and backslashes are refused. Every
ancestor, manifest, directory, and executable is opened without following
symlinks. Discovery checks an executable regular file, at most 64 MiB, with an
execute bit and without setuid/setgid bits. Activation uses no PATH lookup, manifest
argument list, shell expansion, build, installation, or remote resolution.
Unknown mapping fields and protocol versions are errors. `timeout_ms` defaults to
5000 and must be 100–30000 inclusive. Source-only manifests remain discoverable and selectable but cannot activate.

At activation ahu reopens the selected executable through pinned directory
handles, reads at most 64 MiB, and copies those bytes to an owner-only temporary
directory outside any Git checkout. The copy is executable and read-only, mode
0500. This avoids executing a replacement symlink/path after validation; it does
not prove the source bytes are immutable during copying. ahu runs that absolute
copy with the single argument `--ahu-host-protocol=2`, uses the temporary directory
as its working directory, and clears the environment except `PATH=/usr/bin:/bin`.
The executable must be self-contained; native loader dependencies or a script's
absolute interpreter remain OS-managed. Package-relative assets, the caller's
working directory, environment credentials, native settings, and cmux socket
variables are not forwarded. ahu does not change the caller's auth or approval settings.

The child starts a separate process group. stdin/stdout are dedicated pipes;
stderr goes to the null device to avoid deadlock or capture of unbounded or
sensitive diagnostic streams. Report bounded errors through the protocol.
Executable snapshots are removed when the connection is dropped. No wire
transcripts are stored. These are same-user processes, not plugins isolated by a sandbox.

## Framing and negotiation

Each frame is a four-byte unsigned big-endian byte count followed by exactly that
many bytes of one UTF-8 JSON object. Each direction allows 1–1,048,576 JSON bytes;
the four-byte header is outside that bound. No newline framing, banners, logging,
notifications, batches, or unsolicited output is permitted on stdout. Partial
reads and writes are supported. EOF, invalid JSON/envelopes, oversized frames,
wrong versions, and mismatched IDs poison the connection. Leftover unsolicited
bytes are refused before sending the next request. The connection never retries
or reconnects automatically, particularly after mutations.

The envelope types and limits are public in `ahu_host_api::protocol`. Requests
use increasing unsigned IDs starting at 1; one request is outstanding at a time:

```json
{"protocol_version":2,"id":1,"method":"hello","params":{"protocol_versions":[2],"host_api":1,"plugin_id":"example-host","required_capabilities":["create_task_session","verify_task_session","select_task_session","close_task_session","set_task_status","expand_task_session","place_coordinator","set_agent_metadata","open_markdown"]}}
```

The first response must negotiate protocol 2, Host API 1, the exact manifest ID,
and every required method capability. Duplicate capabilities, missing methods,
wrong IDs or versions, and malformed hello data refuse activation and kill the
child group. Extra unique capabilities are permitted but are not invoked by V2.
`inherited_workspace` is an opaque provider-owned reference or JSON null.

```json
{"protocol_version":2,"id":1,"result":{"protocol_version":2,"host_api":1,"plugin_id":"example-host","capabilities":["create_task_session","verify_task_session","select_task_session","close_task_session","set_task_status","expand_task_session","place_coordinator","set_agent_metadata","open_markdown"],"inherited_workspace":null}}
```

Success has exactly `protocol_version`, `id`, and `result` (including JSON null
for void methods). Failure has exactly `protocol_version`, `id`, and `error`:

```json
{"protocol_version":2,"id":2,"error":{"code":"not_created","message":"request refused before creation"}}
```

Error codes are `failed`, `not_created`, and `outcome_unknown`. Unknown envelope
or error fields are refused. Remote error messages are bounded to 512 characters
and escaped for terminal display. Missing method prerequisites are errors, never
an empty successful inventory.

## Operations

Methods correspond to `ahu_host_api::v2::Host`. cmux topology and identifiers
are provider-private. ahu stores only an opaque `session_ref` in the task record;
the provider verifies it against the repository identity and expected worktree
before each mutation. Paths are absolute UTF-8 strings. ahu still owns identity,
approvals, worktree creation, records, cancellation confirmation, and repository
associations.

| Method | Parameter keys | Success result |
| --- | --- | --- |
| `create_task_session` | repository identity/name/root, title, summary, worktree, startup command, focus | `CreatedSession` containing `session_ref` |
| `verify_task_session` | `session_ref`, repository identity, expected directory | `absent`, `owned`, or `mismatch` |
| `select_task_session` | `session_ref`, repository identity, expected directory | null |
| `close_task_session` | `session_ref`, repository identity, expected directory | null |
| `set_task_status` | `session_ref`, repository identity, expected directory, value | null |
| `expand_task_session` | `session_ref`, repository identity, expected directory | null |
| `place_coordinator` | repository identity/name/root, label, harness, model, startup command | placement notes and whether a session opened |
| `set_agent_metadata` | `session_ref`, repository identity, expected directory, agent, harness, model | null |
| `open_markdown` | absolute path, focus | opaque opened-session reference |

For creation, callers must persist all required task inputs before dispatching
`startup_command`: the host may start it before replying. ahu sends the command
as one JSON string; it performs no shell interpolation in this transport. The
host must start it in the requested worktree.
After a success reply ahu records the opaque reference and asks the provider to
verify it against the requested worktree. Failed verification never closes a
session.

## Deadlines, cancellation, and partial start

Every remote procedure call has one monotonic deadline covering pipe writes,
header reads, and body reads. Slow or non-reading peers cannot extend it with
partial progress. The manifest's deadline also applies to hello after process
spawn; OS process creation, executable copying, and temporary-file operations
are not hard real-time bounded. Creation verification uses separate calls, each
with its own deadline.

`ExternalHost::cancellation_token()` returns a token another thread may set.
Transport polls it at most every 50 ms while waiting; cancellation poisons the
connection and sends SIGKILL to its process group. `ExternalHost::cancel()`
terminates an idle connection immediately. Drop also kills that group and polls
for reaping for at most 250 ms. It does not close backend workspaces or infer
whether a startup command ran. The host must exit on stdin EOF; if ahu is killed
without running `Drop`, cleanup depends on the host honoring EOF. An escaping
process, OS-uninterruptible process, or independent GUI session is not contained
by this contract. The protocol has no cooperative wire cancellation method,
restart, or retry.

For `create_task_workspace`, a local error before any request bytes were written
is `NotCreated`. A valid `not_created` response is the host's affirmative guarantee
that no resource or startup side effect occurred. Once any request bytes were
written, timeout, cancellation, EOF, malformed replies, generic `failed` errors,
`outcome_unknown`, and failed ownership checks become `OutcomeUnknown`. Callers
must retain task inputs and work on that outcome. Other mutating methods return
errors without implying rollback. Killing the transport is not proof of
cancelling an already-started task.

## Fixture and external host status

`tests/fixtures/external-host.py` is a complete synthetic peer used by offline
integration tests. It models groups and workspaces in memory, exercises the real
activation and wire transport, and deliberately never starts a shell or GUI. It
is not a replacement terminal host. Tests cover creation, metadata, inventory,
selection/close, protocol mismatch, malformed/partial frames, blocked pipe writes,
indeterminate startup, deadlines, cancellation, cleanup, and executable paths.

Session providers are separate repositories and are installed through the standard
plugin package and committed binding workflow. Core keeps the Host V2 contract,
cross-provider task lifecycle, and harness safety checks that must be enforced
before launching a process. It does not contain a cmux session adapter. Plugins
execute with the user's OS permissions; the protocol and ownership checks do not
sandbox them.

Package tests exercise the server and all nine V2 methods with a fake native CLI,
including creation outcome preservation and strict framed input/output. Core
lifecycle tests use synthetic Host V2 peers. A local ahu-managed Codex smoke task
also created and verified a cmux workspace, ran a read-only command, and removed
the task; cleanup verified ownership and closed that exact workspace. This is a
single live observation, not a claim that other harnesses or cmux versions have
the same result. Repeat live checks after changing either side.

## Native hook and provider policy boundary

Rust core coordinates harness invocation, tasks, worktrees and MCP. It does not
recognize provider hook fingerprints, inspect provider wrapper installations,
or alter provider routing and hook-control environment variables. Guarded
admission conservatively checks native executable configuration; explicit
`--work-mode` leaves compatibility unverified and uses native automatic approvals.
Neither mode establishes filesystem confinement.

Provider-specific hook diagnostics or installation belong to an explicitly
invoked plugin. Such a plugin would need a declared capability and permission
for each operation, bounded structured diagnostic results separating observed
configuration from live verification, and an explicit user-authorized mutation
operation with scope and outcome. Diagnostic results must not authorize core
admission or silently rewrite native hooks. Host V2 remains the session lifecycle
contract; it is not a hook-policy API. This cleanup adds no extension API or
provider default and invokes no native installer.

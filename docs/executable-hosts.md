# Executable session hosts (protocol V1)

For generic one-shot JSON processing without session-host methods, see
[executable plugins](executable-plugins.md). The two declarations and grants are separate.

ahu can explicitly activate a discovered local executable and speak Host V1 over
its standard input and output. The current terminal is the default. Providers such as cmux are installed as external plugins. Discovery with
`ahu plugins` reads metadata only. `ahu plugins --probe` executes the selected
host, negotiates compatibility, reads its complete workspace inventory, then
terminates the connection. Neither command creates an ahu task.

Executable hosts require an enabled committed OKF binding with an
`interactive-backend` grant and exact ahu compatibility. Discovery metadata and
manifest capabilities never grant execution. The selected package must match the
binding's source digest, identity and version. Installations resolve by their
content-addressed receipt; discovered packages are verified in place.

Before workspace startup, ahu persists `session_host` in a schema-5 task record
(older ahu binaries refuse this schema): the
original committed binding and revision, package name, executable declaration,
executable SHA-256 and protocol version. Task operations use that frozen identity,
not the current selection. Each package identity has a separate group mapping;
task records without a frozen host identity cannot be resumed through a guessed provider.
Focus and cleanup verify host identity, workspace directory, group and window.
Core retains harness startup, supervision, approvals, cancellation confirmation,
and worktree cleanup. Uncertain creation preserves task inputs and work.
Executable task focus and cleanup require the recorded workspace, group, and
window identifiers; missing identifiers do not prove absence. If creation is
uncertain, a later terminal supervisor outcome still retains the task inputs and
package. An in-flight cancellation reports missing workspace ownership as unknown,
and automated removal refuses until ownership is reconciled. `ahu` does not recover
ownership automatically after a lost creation reply.

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
  protocol_version: 1
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
copy with the single argument `--ahu-host-protocol=1`, uses the temporary directory
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
{"protocol_version":1,"id":1,"method":"hello","params":{"protocol_versions":[1],"host_api":1,"plugin_id":"example-host","required_capabilities":["list_groups","find_group","current_window","window_for_workspace","add_workspace_to_group","current_workspace","create_anchor_workspace","set_anchor","create_group","expand_group","create_task_workspace","open_markdown","select_workspace","close_workspace","set_status","set_agent_metadata","workspaces","workspaces_in_window"]}}
```

The first response must negotiate protocol 1, Host API 1, the exact manifest ID,
and every required method capability. Duplicate capabilities, missing methods,
wrong IDs or versions, and malformed hello data refuse activation and kill the
child group. Extra unique capabilities are permitted but are not invoked by V1.
`inherited_workspace` is a host-declared ID or JSON null; ahu does not forward
`CMUX_WORKSPACE_ID` to establish it.

```json
{"protocol_version":1,"id":1,"result":{"protocol_version":1,"host_api":1,"plugin_id":"example-host","capabilities":["list_groups","find_group","current_window","window_for_workspace","add_workspace_to_group","current_workspace","create_anchor_workspace","set_anchor","create_group","expand_group","create_task_workspace","open_markdown","select_workspace","close_workspace","set_status","set_agent_metadata","workspaces","workspaces_in_window"],"inherited_workspace":null}}
```

Success has exactly `protocol_version`, `id`, and `result` (including JSON null
for void methods). Failure has exactly `protocol_version`, `id`, and `error`:

```json
{"protocol_version":1,"id":2,"error":{"code":"not_created","message":"request refused before creation"}}
```

Error codes are `failed`, `not_created`, and `outcome_unknown`. Unknown envelope
or error fields are refused. Remote error messages are bounded to 512 characters
and escaped for terminal display. Missing method prerequisites are errors, never
an empty successful inventory.

## Operations

Methods correspond to `ahu_host_api::v1::Host`. Parameters are objects with the
keys below; absent optional parameters must be represented as JSON null. Paths are
absolute UTF-8 strings. `Group`, `CreatedWorkspace`, and `WorkspaceInfo` use the
public Host V1 `serde` representation. IDs are host-owned opaque strings; inventory
and creation validation requires nonempty IDs of at most 256 bytes without
control characters. ahu still owns identity, approvals, worktree creation,
records, cancellation confirmation, and repository associations.

| Method | Parameter keys | Success result |
| --- | --- | --- |
| `list_groups` | `window_id` | `{"complete":true,"groups":[Group]}` |
| `find_group` | `group_id`, `window_id` | `Group` or null |
| `current_window` | none | window ID or null |
| `window_for_workspace` | `workspace_id` | window ID |
| `add_workspace_to_group` | `group_id`, `workspace_id`, `window_id` | null |
| `current_workspace` | none | workspace ID or null |
| `create_anchor_workspace` | `group_id`, `title`, `cwd` | workspace ID |
| `set_anchor` | `group_id`, `workspace_id` | null |
| `create_group` | `name`, `cwd` | `Group` |
| `expand_group` | `group_id` | null |
| `create_task_workspace` | `group_id`, `window_id`, `title`, `summary`, `cwd`, `startup_command`, `focus` | `CreatedWorkspace` |
| `open_markdown` | `path`, `focus` | workspace ID |
| `select_workspace` | `workspace_id` | null |
| `close_workspace` | `workspace_id` | null |
| `set_status` | `workspace_id`, `value` | null |
| `set_agent_metadata` | `workspace_id`, `agent`, `harness`, `model` | null |
| `workspaces` | none | `{"complete":true,"workspaces":{"id":WorkspaceInfo}}` across all windows |
| `workspaces_in_window` | `window_id` | same inventory shape, scoped to the supplied window; null means all windows |

`complete:false` is an error. Group identities and members must be unique within
their respective lists, and each group's members must include its anchor.
`find_group` must return the requested identity. Inventory completeness is a
host assertion, not proof of external reality.

For creation, callers must persist all required task inputs before dispatching
`startup_command`: the host may start it before replying. ahu sends the command
as one JSON string; it performs no shell interpolation in this transport. The
host must preserve the requested working directory, group, and optional window.
After a success reply ahu independently queries a complete window inventory, the
workspace's window, and its group membership. It accepts creation only if the
exact requested directory and ownership match. Failed verification never closes
a workspace.

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

## Fixture and external cmux status

`tests/fixtures/external-host.py` is a complete synthetic peer used by offline
integration tests. It models groups and workspaces in memory, exercises the real
activation and wire transport, and deliberately never starts a shell or GUI. It
is not a replacement terminal host. Tests cover creation, metadata, inventory,
selection/close, protocol mismatch, malformed/partial frames, blocked pipe writes,
indeterminate startup, deadlines, cancellation, cleanup, and executable paths.

Session providers are separate repositories and are installed through the standard
plugin package and committed binding workflow. Core keeps only the Host V1
contract and cross-provider task lifecycle. Plugins execute with the user's OS
permissions; the protocol and ownership checks do not sandbox them.

Package tests exercise the server and every V1 method with a fake native CLI,
including creation outcome preservation and strict framed input/output. Core
lifecycle tests use synthetic Host V1 peers. End-to-end activation of the cmux
binary through core and live cmux lifecycle verification remain integration gaps,
including default socket discovery under core's cleared environment. Package
existence and project activation do not establish live compatibility.

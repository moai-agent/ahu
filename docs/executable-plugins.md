# One-shot executable plugins (protocol V1)

A package can declare a generic JSON entrypoint separately from its optional
[session-host executable](executable-hosts.md). The host library exposes
`ahu::plugin_exec::invoke`; this initial slice has no CLI invoke command, MCP
registration, background activation, or TaskService callbacks. It is suitable for
an explicit host call to a trusted local package that processes supplied data.
It does not implement an execution provider or grant access to task operations.

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
packages. Discovery, installation and enablement never invoke the entrypoint.
An enabled binding with `execute` but no `interactive-backend` grant is excluded
from default session-host selection. Explicit interactive selection still needs
its separate host declaration and grant.

## Trusted host call

The embedding host supplies a repository-bound `HostGrant` and a minimal,
allowlisted invocation context. This type has private fields and is not
deserializable. Its constructor belongs in trusted host code, not an untrusted
request handler that
passes through caller-selected identities or scope.

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

TaskService remains a separate host-owned boundary. Hosts can release data they
have already authorized through a scoped TaskService session, but this transport
neither creates such a session nor gives the executable a service/controller
handle, task IDs, storage paths, credentials, or callback channel. A package
cannot expand authority by returning a task mutation or embedding grants in its
input. Those values are ordinary JSON data with no dispatch behavior.

## Wire contract and process lifetime

Ahu snapshots the verified installed executable into a private temporary
directory outside repositories, then starts it directly with this fixed argv:

```text
<private snapshot executable> --ahu-plugin-protocol=1
```

There is no shell command construction, PATH-based executable selection, caller
argv extension or probe. Working directory is the private snapshot directory.
The environment is cleared, then `PATH=/usr/bin:/bin` is set; no auth/environment
variables or repository configuration are forwarded. Stderr goes to the null
device, and no execution trace is retained. Snapshot bytes and directory are
removed on return. The binary must be self-contained apart from system runtime
libraries/interpreters; package-relative assets are not copied.

Stdin carries exactly one UTF-8 JSON document and is then closed:

```json
{"protocol_version":1,"plugin_id":"example","grant":{"operation":"invoke"},"context":{"invocation_id":"example-1"},"input":{"text":"synthetic input"}}
```

Stdout must contain exactly one document, close, and the process must exit zero:

```json
{"status":"ok","protocol_version":1,"result":{"text":"SYNTHETIC INPUT"}}
```

Or return a fixed error code (`failed`, `invalid_input`, or `unsupported`):

```json
{"status":"error","protocol_version":1,"code":"failed"}
```

Unknown envelope fields, duplicate keys, unsupported versions, multiple documents,
malformed JSON and nonzero exits fail. Stdout is limited to 64 KiB total,
including whitespace. Input and output use concurrent nonblocking I/O so a peer
that writes before reading cannot deadlock the host. A single deadline covers
spawn, input, output and exit observation. Package verification/snapshot I/O
precede that deadline; filesystem access and OS process creation are not
interruptible by this token.

Another thread may set the supplied atomic cancellation token. A token already
set prevents activation; in-flight transport checks it at least every 20 ms
between bounded I/O steps. A response without EOF or process exit still times
out. On completion, cancellation or failure, ahu signals its owned process group
before reaping the leader, with a bounded 250 ms cleanup wait on failure. Group
cleanup prevents ordinary inherited-pipe descendants from outliving the call.
An uninterruptible process may remain unreaped after the cleanup bound.

`Failure` exposes only a fixed `Code` and `outcome_may_be_unknown`. Child stderr,
error text, OS diagnostics and package contents are never included. Successful
`result` data is not redacted. Failures after spawn set the outcome flag: even a
malformed response or timeout may follow side effects. There is no automatic
retry or claim of rollback.

This is a communication and lifecycle boundary, **not an OS sandbox**. An enabled
executable has the invoking user's OS rights and can access files, credentials
or networking on its own, or escape its process group. No network client, auth
lookup or provider-specific behavior is supplied by this slice. Use trusted
packages; hostile same-user mutation during verification is outside the existing
installation threat model. Strong filesystem/network isolation would require a
separate platform sandbox design.

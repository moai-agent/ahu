# Installed harness runtime API

This crate contains owned descriptors, envelopes, fixed operation names and
strict JSON decoding. It has no core dependency or provider implementation.
`examples/independent.rs` is a synthetic Rust protocol peer, not a supported
native harness plugin. The core transport's independent integration fixture does
not use these serialization types.

A staged Rust package places this top-level object in `ahu-plugin.md`:

```yaml
harness_runtime:
  protocol_version: 1
  path: bin/independent
  timeout_ms: 1000
  descriptor:
    id: independent
    display_name: Independent
    program: independent-native
    aliases: [ind]
    models: [example/exact-v1]
    modes: [interactive, headless, direct]
    operations: [probe]
    capabilities: []
    tested_versions: []
    context_paths: [.independent/]
    private_context_paths: [.independent/local.json]
    skill_paths: [.independent/skills]
    native_source_paths: [.independent/agents]
```

The plugin package ID need not equal the harness ID. Names and aliases must be
unique across enabled runtime packages and cannot collide with core commands.
Models are suggestions, not a core allowlist. Paths describe native locations;
they do not authorize filesystem access or mutations by core.

Core requires a committed enabled binding with the `harness-runtime` grant,
exact core compatibility and verified installed package bytes. Discovery and
setup preparation never execute code. A runtime snapshots its executable and
freezes binding plus descriptor identity. Callers persist `binding_digest()` in
attempt state and compare before resuming or reconciling. Opening another session
rechecks current policy and installed bytes and refuses drift. An already open
session retains its original peer for completion/cleanup; it never rebinds.

Each request is one JSON line, including its terminating newline within the
1 MiB limit. IDs start at 1 and increase for each session:

```json
{"protocol_version":1,"id":1,"operation":"probe","params":{}}
{"protocol_version":1,"id":1,"result":{"available":true}}
```

A failure has `error` instead of `result`, with exactly one fixed string:
`failed`, `invalid_input`, `unsupported`, `outcome_unknown`, `needs_native_trust` or `not_signed_in`. The last two are closed, actionable health causes, never free text. Null is a valid
result. Unknown envelope fields, duplicate keys at any depth, incompatible
versions, sequence mismatches, oversized or partial frames, and unsolicited
frames fail the session. No diagnostic message field is accepted. Use
`json::from_slice` before consuming untrusted JSON; plain Value deserialization
loses duplicate keys.

The runtime starts with `--ahu-harness-runtime-protocol=1`, a private snapshot
working directory and an empty environment. Core passes any selected native
home, executable and PATH through explicit operation parameters. Stderr goes to
the null device. A session owns a process group; errors, cancellation and Drop
kill it before reaping its leader, with a 250 ms cleanup bound. Descendants that
escape the group and processes stuck in uninterruptible kernel calls remain
outside confirmed cleanup. Explicit `finish()` reports cleanup failure; Drop
cannot. This executes same-user plugin code and does not provide a sandbox.

Normal `Runtime::call` closes stdin after the response and requires clean stdout
EOF and successful peer exit within the operation deadline. Persistent users
call `open_stream`, then ordered operations, then `finish`. Decoding requires
`decode_start`, zero or more `decode_event`, and `decode_finish`. Finalization
never invents decoder success. `Session::call_cancellable` accepts a host-owned
atomic cancellation flag. Failed peers are poisoned; no automatic retry occurs.
Timeouts and other failures after dispatch leave effects unknown.

The concrete consumer is core's native-service orchestration. A skill or the
one-shot actions contract cannot retain decoder state or bind installed native
services to attempts, so runtime authority is separate. This transport validates
envelopes and bounds I/O; consumer modules must validate typed operation payloads
before using them, including launch program/argv/prompt rules, proposed setup
writes and exact trust receipts. The returned Value grants no command execution,
filesystem mutation, credential access or task-state authority by itself.

Private context paths are optional, limited to 64 unique plain relative paths,
and must be covered by context_paths. A trailing slash names a subtree. They
select owner-local acceptance rather than shared lock fingerprints; declaring
a path does not authorize reading outside the project or following links.

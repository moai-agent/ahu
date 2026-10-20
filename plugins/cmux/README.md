---
okf_version: "0.2"
type: ahu:knowledge
title: ahu cmux session plugin
description: Build and inspect the compiled and executable cmux session package
tags: [session, terminal]
---

# ahu cmux session plugin

This Rust package implements `ahu_host_api::v1::Host` and supplies the standalone
`ahu-cmux-host` executable for the public Host V1 framed protocol.
It owns cmux CLI/RPC transport, capability checks, and surface mechanics.
Core owns identity, permissions, worktrees, records,
repository associations, supervision, cancellation confirmation, and outcomes.

## Executable server

Build the executable independently of the core CLI, from the workspace root:

```sh
cargo build --locked --release -p ahu-cmux-plugin --bin ahu-cmux-host
cargo test --locked -p ahu-cmux-plugin
```

The binary is `target/release/ahu-cmux-host` (or under `CARGO_TARGET_DIR` when
set). Invoke it with exactly `--ahu-host-protocol=1` and exchange binary frames
on stdin/stdout as described in [the protocol guide](../../docs/executable-hosts.md).
It is not a line-oriented command or interactive terminal. It requires a first
`hello` with ID 1, Host API 1, protocol 1, and plugin ID `cmux-local`. After
checking native reachability and required cmux capabilities, it declares every
Host V1 method. Subsequent request IDs must increase; repeat hello, invalid
envelopes/parameters, unknown fields, duplicate JSON keys, and invalid frames
terminate the connection without dispatching that request. Frames in each
direction are limited to 1 MiB. Backend failures use bounded protocol errors;
process diagnostics use stderr. EOF between requests exits cleanly.

Native CLI lookup uses absolute PATH entries followed by
`/Applications/cmux.app/Contents/Resources/bin/cmux`. Implicit candidates are
canonical executable files outside Git working trees. Direct standalone callers
can set `AHU_CMUX_BIN` to an explicit absolute CLI path, including a fake CLI for
testing; relative overrides are refused by this entrypoint. Direct invocations
retain the native `CMUX_SOCKET_PATH` and `CMUX_WORKSPACE_ID` behavior.

Core activation clears those environment variables and supplies only
`PATH=/usr/bin:/bin`. It copies the plugin executable into a temporary directory
and does not provide package-relative assets. Under core activation, native cmux
must therefore be reachable through that PATH or the application-bundle location
and its default socket discovery. The server does not widen core's environment
contract. Live socket discovery and lifecycle behavior in that environment have
not been verified here.

The checked-in discovery manifest remains source-only because no built binary is
shipped in this directory. For an executable distribution, stage the built binary
as `bin/ahu-cmux-host` beside a copy of the package metadata, keep ID `cmux-local`,
and add this frontmatter mapping to the staged `ahu-plugin.md`:

```yaml
executable:
  path: bin/ahu-cmux-host
  protocol_version: 1
  timeout_ms: 15000
```

Project activation additionally requires the committed binding, compatibility,
digest, and `interactive-backend` grant documented in the protocol guide.
Building the server neither installs a package nor selects a session host.
Distribution automation and live lifecycle verification remain separate work.

The server delegates creation to the existing cmux implementation: failure
before dispatch can report `not_created`; failure after dispatch remains
`outcome_unknown`. It never retries creation or closes an uncertain workspace.
Complete inventory errors stay errors. A null `workspaces_in_window` scope
queries all windows as required by Host V1. Core still verifies returned
ownership and exact directories. The native CLI subprocess implementation is
unchanged and has no new per-command deadline or output cap; core's transport
deadline and process-group cancellation bound calls when activated by core.
The 15-second example allows room for the adapter's 10-second discovery loop,
but cannot guarantee a response from a blocked native CLI.

Offline tests exercise the actual server with a fake cmux executable. They cover
every method, strict decoding, frame limits, complete inventories, and creation
outcomes without a live cmux app or execution of supplied startup commands.

## Compiled compatibility adapter

The compiled adapter remains an opt-in compatibility path. The normal ahu
build uses the current terminal and does not include or require cmux. To build
the compatibility adapter from the complete ahu source workspace, use its root
lockfile:

```sh
cargo build --locked --features cmux-plugin
cargo install --path . --locked --features cmux-plugin
ahu plugins --output json
```

Builds with `--features cmux-plugin` include this package. The default build
omits it. `AHU_INTERACTIVE_PLUGIN=cmux` explicitly selects it;
`AHU_INTERACTIVE_PLUGIN=none` disables session operations for that process.
Inventory never executes package code or activates it. Activation pings cmux;
task launch then checks capabilities. No other backend is selected.

Window-scoped group queries preserve their scope on failure. A failed query,
including a build that rejects the window filter, is reported as an error;
ahu does not retry against the current window to infer task ownership or absence.

The included `ahu-plugin.md` uses OKF 0.2 YAML frontmatter with `type: ahu:plugin`
and exposes this source package to local metadata
discovery under the distinct ID `cmux-local`. From the physical workspace root:

```sh
AHU_PLUGIN_PATH="$(pwd -P)/plugins/cmux" ahu plugins --output json
AHU_PLUGIN_PATH="$(pwd -P)/plugins/cmux" AHU_INTERACTIVE_PLUGIN=cmux-local ahu plugins
```

This reads manifests only, without building, loading, or enabling the package.
This manifest remains source-only and has no executable declaration; stage an
executable distribution as described above to use the new server through core.
Select `cmux` to use the compiled adapter. Discovery accepts only bounded regular metadata
files and rejects symlinks and paths escaping the supplied directory. The
repository-root `docs/reference.md` documents the manifest schema, limits,
selection semantics, and remaining lifecycle work.

These workspace packages are unpublished. Individual Cargo archives inspect
package contents; they are not a registry installation channel. There is no
installer hook, dynamic loading, stable binary ABI, or independent update path.
This package runs as same-user code and has no sandbox. Native harness hooks
and their headless isolation checks remain core responsibilities.

The repository-root `docs/reference.md` also documents missing-plugin behavior
and legacy record/cleanup compatibility.

---
okf_version: "0.2"
type: ahu:knowledge
title: ahu cmux session plugin
description: Build and inspect the compiled cmux session package
tags: [session, terminal]
---

# ahu cmux session plugin

This optional, compiled Rust package implements `ahu_host_api::v1::Host`.
It owns cmux CLI/RPC transport, capability checks, and surface mechanics.
Core selects the executable and owns identity, permissions, worktrees, records,
repository associations, supervision, cancellation confirmation, and outcomes.

This older compiled adapter is an opt-in compatibility path. The normal ahu
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
This manifest remains source-only and has no executable declaration. Other local
packages can use the protocol documented in repository-root
`docs/executable-hosts.md`, but
external cmux activation still needs a server wrapper and durable backend routing. Continue selecting
`cmux` for actual session hosting. Discovery accepts only bounded regular metadata
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

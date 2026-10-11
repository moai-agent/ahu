# ahu cmux session plugin

This optional, compiled Rust package implements `ahu_host_api::v1::Host`.
It owns cmux CLI/RPC transport, capability checks, and surface mechanics.
Core selects the executable and owns identity, permissions, worktrees, records,
repository associations, supervision, cancellation confirmation, and outcomes.

Build/install from the complete ahu source workspace using its root lockfile:

```sh
cargo build --locked --features cmux-plugin
cargo install --path . --locked --features cmux-plugin
ahu plugins --output json
```

The default build includes this package for compatibility. Build ahu with
`--no-default-features` to omit it. `AHU_INTERACTIVE_PLUGIN=cmux` explicitly
selects it; `AHU_INTERACTIVE_PLUGIN=none` disables session operations for that
process. Inventory never executes package code or activates it. Activation pings
cmux; task launch then checks capabilities. No other backend is selected.

These workspace packages are unpublished. Individual Cargo archives inspect
package contents; they are not a registry installation channel. There is no
installer hook, dynamic loading, stable binary ABI, or independent update path.
This package runs as same-user code and has no sandbox. Native harness hooks
and their headless isolation checks remain core responsibilities.

See [the host reference](../../docs/reference.md#interactive-host-boundary) for
missing-plugin behavior and legacy record/cleanup compatibility.

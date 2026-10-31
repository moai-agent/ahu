# kiro harness plugin

This external Rust executable provides native launch, configuration inspection,
setup proposals and event normalization to ahu. Install the native CLI separately
and sign in using its own commands. No Python runtime is needed.

The executable requires the explicit `harness-runtime` grant. Discovery is inert;
authorized operations may inspect native configuration and invoke native probes.
Core supervises the agent, validates task outcomes and owns worktree lifecycle.
A Git worktree is not a sandbox.

Build from this checkout with `cargo build --release --manifest-path plugins/harness-kiro/Cargo.toml`.
Source builds use adjacent Rust support crates; distribute the resulting binary
with this package manifest through ahu's verified plugin installer.
No native CLI or credentials are bundled.

See [the runtime contract](../../docs/harness-runtime-contract.md) and
[implementation security](../harness-runtime/SECURITY.md).

Native 2.34.0 version and chat help were inspected locally. The V3, exact model,
non-interactive, stream-json, MCP-startup, trust and resume-ID options remain
present. This is argument evidence only; account capacity and live event-format
conformance must be checked independently.

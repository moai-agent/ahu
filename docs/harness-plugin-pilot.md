# Harness runtime migration

The launch-only experiment has been superseded by the installed harness runtime
contract. The `harness-plugin-pilot` feature, `harness-launch` grant and launch
protocol 2 are no longer accepted by the current implementation.

## Discover and prepare a local package

Use the [Rust harness plugin installation workflow](harness-plugins.md#installation-and-activation).
Build and stage a Rust runtime executable, prepare its content-pinned binding,
install explicitly, then review and commit activation with `harness-runtime`.
No builtin native adapter is a fallback for a missing or revoked runtime.

The [runtime contract](harness-runtime-contract.md) covers native services and
stateful event normalization as well as launch planning. Published release notes
remain historical records of their respective versions.

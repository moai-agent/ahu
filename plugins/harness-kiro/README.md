# kiro harness plugin

This external Rust executable provides native launch, configuration inspection,
setup proposals and event normalization to ahu. Install the native CLI separately
and sign in using its own commands. No Python runtime is needed.

The executable requires the explicit `harness-runtime` grant. Discovery is inert;
authorized operations may inspect native configuration and invoke native probes.
Core supervises the agent, validates task outcomes and owns worktree lifecycle.
A Git worktree is not a sandbox.

Follow the [complete build, staging and installation guide](https://github.com/moai-agent/ahu/blob/v0.11.1/docs/installation.md).
This package is released as source in the tagged ahu repository; it is not a
standalone source checkout. Build uses adjacent Rust support crates. Stage the
native executable before computing its content digest. Installation never builds
source or bundles the native CLI or credentials.

See [the runtime contract](https://github.com/moai-agent/ahu/blob/v0.11.1/docs/harness-runtime-contract.md) and
[implementation security](SECURITY.md).

Native 2.34.0 version and chat help were inspected locally. The V3, exact model,
non-interactive, stream-json, MCP-startup, trust and resume-ID options remain
present. This is argument evidence only; account capacity and live event-format
conformance must be checked independently.

Kiro is optional. Its native authentication, tool policy and workspace-trust
defaults remain prerequisites; ahu does not accept trust on the user’s behalf.
Use another installed harness without installing or configuring Kiro. A native
trust refusal is not evidence of successful interactive execution.

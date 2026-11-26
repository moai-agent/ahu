# Rust harness plugins

## Vocabulary

A **harness** is the vendor's CLI (Claude Code, Codex, Antigravity, OpenCode,
Kiro): it runs the agent loop and owns the conversation. A **harness plugin** is
ahu's installed Rust wrapper around one harness; it manages and health-checks
that harness in the context of ahu and exposes native behavior that is not
available over MCP. A **native agent** is an agent built for one harness and run
only in it. ahu refuses to run a native agent in another harness or with a model
that has not been admitted for it.

Any harness that supports MCP can use ahu's agents, tasks and skills through
`ahu mcp serve` without a plugin. A plugin adds launch, setup, health, admission
and event observation for that harness.

ahu delegates native harness integration to installed Rust executables. Core owns
agent identity, task and worktree state, process supervision, MCP coordination,
context locking, and acceptance of eval evidence. Plugins own native command
arguments, version/model interpretation, configuration, admission, account
observations, trust receipts and event normalization. A plugin never declares a
task successful on its own.

## Packages

| Harness ID | Native CLI | Package | Historical implementation baseline |
| --- | --- | --- | --- |
| `codex` | `codex` | [Codex](https://github.com/moai-agent/ahu-plugin-codex) | 0.163.0 |
| `claude-code` | `claude` | [Claude Code](https://github.com/moai-agent/ahu-plugin-claude-code) | 2.1.310 |
| `antigravity` | `agy` | [Antigravity](https://github.com/moai-agent/ahu-plugin-antigravity) | 1.4.7 |
| `opencode` | `opencode` | [OpenCode](https://github.com/moai-agent/ahu-plugin-opencode) | 2.0.32 |
| `kiro` | `kiro-cli` | [Kiro](https://github.com/moai-agent/ahu-plugin-kiro) | 2.34.0 (version/help inspected) |

Each package has an independent source repository, semantic version, manifest,
lockfile and Rust binary. The repository links above are public source
repositories; select the reviewed version from each repository’s Releases page.
No sibling checkout or core source tree is needed to build a plugin. Each owns
its native implementation, and vendors a frozen version of the small provider-independent wire-contract crate
with recorded checksums. This dependency is source only, not a
second executable or an automatic upgrade. Core links the data contract and never
the native implementations. Customer builds and execution need no Python.

The [publication plan](publication.md) covers initial publication. Later harness
releases proceed independently while compatible with core's runtime protocol and
binding requirements. Source directories must be built/staged before installation.
Historical version evidence is not live all-mode certification.

## Installation and activation

Follow the [complete installation guide](installation.md) for locked builds,
repository-local staging, disabled bindings, explicit installation and grants,
project setup, a live smoke and cleanup. It includes copyable commands for all
five packages. No development helper is required.

The former all-harness development installer is retired. Use each independent
repository's INSTALL.md and Rust package command. Installation copies a previously
built, content-pinned package and never invokes Cargo or implicitly grants runtime
access. Version changed package contents and review each new pin.

Harnesses are optional. Without a harness plugin, core commands remain available;
launching that harness reports the missing installed capability. A binding may
use a package ID independent of the harness ID in its descriptor. Conflicting harness
IDs, aliases and aliases colliding with core commands are refused.

## Harnesses without a plugin

A package may declare `harness_runtime` with no `path`, modes `[direct]`,
operations within `[probe, launch]` and the capability `mcp-only`. It contains no
executable. Core answers `probe` (runs the declared program once with `--version`)
and `launch` (a direct coordinator session, no arguments, no prompt). Models,
admission, setup, health checks and event decoding need a real plugin, so such a
harness has no model enforcement and its probe reports no reviewed compatibility.
Register `ahu mcp serve` in the harness's own MCP configuration to use ahu's
agents, tasks and skills. [`examples/mcp-only-harness`](../examples/mcp-only-harness/README.md)
is a copyable package; it is bound, installed and granted like any other harness
package, so it stays content-pinned and reviewed.

## Health diagnostics

A plugin failure carries one fixed code, never free text. Besides `failed`,
`invalid_input`, `unsupported` and `outcome_unknown`, two actionable causes are
reported: `needs_native_trust` (the harness is waiting for its own folder-trust
decision in this checkout; open it there interactively, because ahu never approves
native trust) and `not_signed_in`. `ahu auth status` and launch refusals show the
matching instruction.

## Native behavior and upgrades

`ahu harness ID` starts a direct coordinator session. Installed aliases provide
shortcuts such as `ahu codex`, `ahu claude` and `ahu agy`. Headless agent execution
requires no tracker plugin. GitHub is a separate optional integration.

The selected plugin receives the exact model, mode, permissions, native version,
timeout and optional resume identity. Core inserts task prompts only after
validating the returned command. Interactive and headless modes have different
native controls; for example, OpenCode mini refuses unattended auto permissions.
Native settings may still deny tools even with automatic approval requested.

A provider update is delivered by versioning its package and reviewing new content
pins. No core recompile is required while the runtime protocol remains compatible.
Existing tasks freeze package/binding identity; an upgrade cannot silently rebind
resume or cleanup. Finish and remove old tasks before activating replacements.
Unpinned native CLI versions float; this does not establish live compatibility.
Exact project pins and version-specific native controls remain enforced. Invalid
versions and unsupported operations fail without a provider fallback.

## Security and evidence

Runtime permission is broader than launch planning: native inspection may read
configuration and execute bounded native probes, setup proposes writes, and trust
operations reconcile exact task receipts. Installed code runs with the user's OS
privileges. Protocol limits and worktrees are not a sandbox. Review package source
and provenance before granting runtime access.

One external normalizer handles an attempt's native events. Core retains process
outcome and completion checks. Missing usage, unobserved helpers and unsupported
measurements stay unknown. Native credits are not tokens or USD. Raw events and
final answer snapshots must not be published or logged by the runtime transport.

The [runtime contract](harness-runtime-contract.md) specifies the bounded protocol
and required integration checks. Validate native version/model, interactive and
headless modes, actual MCP calls, local OTel, evals, resume, cancellation and cleanup
separately. Synthetic protocol tests are not live harness certification.

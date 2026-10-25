# Rust harness plugins

ahu delegates native harness integration to installed Rust executables. Core owns
agent identity, task and worktree state, process supervision, MCP coordination,
context locking, and acceptance of eval evidence. Plugins own native command
arguments, version/model interpretation, configuration, admission, account
observations, trust receipts and event normalization. A plugin never declares a
task successful on its own.

## Packages

| Harness ID | Native CLI | Package | Current implementation version evidence |
| --- | --- | --- | --- |
| `codex` | `codex` | [Codex](../plugins/harness-codex/README.md) | 0.163.0 |
| `claude-code` | `claude` | [Claude Code](../plugins/harness-claude-code/README.md) | 2.1.310 |
| `antigravity` | `agy` | [Antigravity](../plugins/harness-antigravity/README.md) | 1.4.7 |
| `opencode` | `opencode` | [OpenCode](../plugins/harness-opencode/README.md) | 2.0.32 |
| `kiro` | `kiro-cli` | [Kiro](../plugins/harness-kiro/README.md) | 2.34.0 (version/help inspected) |

Each package has an independent semantic version, manifest, lockfile and binary.
Source builds use adjacent Rust implementation libraries. Core links only the
shared data contract, never the native implementations. Customer execution needs
no Python interpreter. These source packages are not a published distribution;
version evidence does not establish live account access or all-mode certification.

## Installation and activation

Build the selected package from this checkout using an external build directory:

```sh
CARGO_TARGET_DIR=/absolute/build/ahu-plugins cargo build --locked --release --manifest-path plugins/harness-codex/Cargo.toml
```

Stage its manifest, Cargo metadata, README, license and security notice in a
separate package directory. Put the built `ahu-plugin-codex` binary at
`bin/ahu-plugin-codex`, matching the runtime declaration. Do not put credentials,
native configuration, execution traces or build caches in the staged package.
Installation verifies and copies content; it never builds or executes the plugin.

From the project in which the plugin will be used:

```sh
ahu setup --list-harness-plugins
ahu setup --harness-plugin /absolute/staged/harness-codex
```

Discovery reads explicit `AHU_PLUGIN_PATH` directories and retained installations
without executing plugins or native CLIs. Preparation creates a disabled,
content-pinned local binding. Review and commit its binding and context lock.
Then install explicitly:

```sh
ahu plugins install harness-codex
ahu plugins installed
ahu plugins enable harness-codex
```

Add `harness-runtime` to the binding's grants, refresh with `ahu lock --update`,
and review and commit the activation. Keep the staged package unchanged until
installation completes. Setup does not silently authorize a runtime. Run normal
`ahu setup` after activation to select exact models and review native MCP/skill
setup proposals. Native CLIs and their authentication are installed separately.

Harnesses are optional. Without a harness plugin, core commands remain available;
launching that harness reports the missing installed capability. A binding may
use a package ID independent of its descriptor's harness ID. Conflicting harness
IDs, aliases and aliases colliding with core commands are refused.

## Native behavior and upgrades

`ahu harness ID` starts a direct coordinator session. Installed aliases provide
shortcuts such as `ahu codex`, `ahu claude` and `ahu agy`. Headless agent execution
requires no terminal-host or tracker plugin. cmux and GitHub are separate optional
integrations.

The selected plugin receives the exact model, mode, permissions, native version,
timeout and optional resume identity. Core inserts task prompts only after
validating the returned command. Interactive and headless modes have different
native controls; for example, OpenCode mini refuses unattended auto permissions.
Native settings may still deny tools even with automatic approval requested.

A provider update is delivered by versioning its package and reviewing new content
pins. No core recompile is required while the runtime protocol remains compatible.
Existing tasks freeze package/binding identity; an upgrade cannot silently rebind
resume or cleanup. Finish and remove old tasks before activating replacements.
Unknown versions and unsupported operations fail explicitly, without another
provider or built-in fallback.

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

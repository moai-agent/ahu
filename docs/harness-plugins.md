# Rust harness plugins

ahu is moving native harness integration into independently versioned Rust
executables. Core remains responsible for agent identity, MCP coordination,
worktree and task state, process supervision, and evidence attribution. A plugin
selects native behavior; it does not become the authority for task completion.

## Prepared packages

| Harness ID | Native CLI | Package | Initial native version |
| --- | --- | --- | --- |
| `codex` | `codex` | [Codex planner](../plugins/harness-codex/README.md) | 0.163.0 |
| `claude-code` | `claude` | [Claude Code planner](../plugins/harness-claude-code/README.md) | 2.1.310 |
| `antigravity` | `agy` | [Antigravity planner](../plugins/harness-antigravity/README.md) | 1.4.7 |
| `opencode` | `opencode` | [OpenCode planner](../plugins/harness-opencode/README.md) | 2.0.32 |

Each package has its own Cargo manifest, lockfile, semantic version and executable,
with no dependency on ahu implementation crates. Build and stage a native binary
before installation. Neither setup nor installation compiles or executes package
code. These packages are source preparations, not published distributions.

The optional `harness-plugin-pilot` build routes registered task command planning
through installed packages with no built-in fallback. Ordinary builds retain
native adapters. Direct native shortcuts, native configuration, trust, event
parsing and enforcement reporting are not yet extracted. Installing a planner
alone does not make an arbitrary new harness usable.

## Version ownership

Protocol 2 passes the observed native semantic version and requested task timeout
to the selected planner. The package decides which native versions it accepts and
owns the command arguments. Its manifest declares the native executable name and
`tested_versions` for offline inspection. That list is package metadata, not a
claim that all live integration checks have passed.

A native argument or accepted-version change can be delivered by rebuilding and
versioning the package, updating its reviewed content pins, installing it and
committing activation. No core recompile is needed when the existing protocol and
remaining core native integrations are compatible. A new event format, configuration
format or runtime topology may still require core changes until extraction is
complete. Existing task bindings are frozen; do not update an active task's
planner and expect its identity to change silently.

Unknown native versions are refused. Plugins never automatically download, update,
fall back to another version or substitute a harness. See each package's README for the
supported execution modes. In particular, OpenCode's interactive `mini` mode does
not support unattended auto permissions; the plugin refuses that combination.
Headless planning supports unattended work mode only.

## Installation and authority

Follow the [setup and activation workflow](harness-plugin-pilot.md#discover-and-prepare-a-local-package)
with an explicitly staged package path. Discovery is metadata-only. Preparation
creates a binding that is turned off with no grants; activation requires reviewed committed
pins and the `harness-launch` grant. No customer Python interpreter is required.

The planner receives model, mode, permissions, native version, timeout and optional
resume identity. It receives no prompt, account credentials or project path.
Core validates a bounded response and inserts the prompt after planning. A cleared
environment and private execution directory reduce accidental exposure; they are
not a sandbox. Installed code runs with the user's OS privileges. Review package
source and provenance before granting execution.

## Remaining extraction gates

Before removing built-in adapters or making this the default integration path:

- Move native configuration, MCP setup, trust and approval mappings into packages.
- Move native event decoding, usage interpretation and resume compatibility into
  a bounded contract, keeping observed process/task state in core.
- Replace compiled harness discovery and direct shortcut selection with installed
  capability discovery, including useful behavior when no harness is installed.
- Verify headless and interactive modes independently with actual MCP calls,
  task-attributed local telemetry, evals, resume, cancellation and cleanup. Record
  native version and model; synthetic planner tests are not live certification.
- Verify package-only upgrades on the same core binary, and publish platform
  artifacts with installation instructions and independent package CI.

The [protocol and Codex pilot observations](harness-plugin-pilot.md) distinguish
verified behavior from the remaining work. Harness and session-host plugins are
separate: headless execution does not require cmux or an issue tracker.

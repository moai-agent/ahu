# Customer runtime inventory

ahu-maintained customer executables are implemented in Rust. Python is allowed
in development tests and tooling, including native CLI fixtures and migration
oracles. It is not an installation or runtime prerequisite. Skills and
declarative configuration remain text. This policy does not prescribe the
implementation language of external CLIs or harnesses.

| Component | Implementation | Distribution and scope |
| --- | --- | --- |
| ahu CLI, MCP, task coordination, worktrees, OTel and evals | Rust | Core executable |
| Codex runtime | Rust | Optional external harness package |
| Claude Code runtime | Rust | Optional external harness package |
| Antigravity runtime | Rust | Optional external harness package |
| OpenCode runtime | Rust | Optional external harness package |
| Kiro runtime | Rust | Optional external harness package |
| GitHub CLI and tracker | Rust | Optional external packages; require native `gh` |
| cmux host | Rust | Optional standalone Host V2 package; not executed by ahu 0.11.1 |
| Native Ollama decision client | Rust | Core client; requires compatible external Ollama and model |
| text-stats authoring example | Rust | Independent example; build and stage before installation |

The five harness runtime packages live under `plugins/`. They link shared Rust
native-service and event-normalization implementation libraries; core links only
the data-only `ahu-harness-api` contract. Core's `src/harness/` holds generic launch
validation and lifecycle facades. Package updates can change native compatibility
without recompiling core. Rust implementation does not establish current account
availability or live compatibility with every native version.

The GitHub migration retains independent protocol fixtures, including
Python development oracles. Those files must never appear in the staged runtime
packages. Build artifacts are platform-specific and are pinned after staging;
ahu does not execute installation hooks or invoke Cargo during plugin install.
Review the plugin's README and security inventory before enabling it.

[Harness packages](harness-plugins.md) must prove agent work, actual MCP calls,
attributed local OTel, eval outcomes and cleanup separately for each advertised
mode. Task/process authority stays in core. Native limitations remain explicit,
and missing telemetry must never become zero usage.

See the [installation guide](installation.md) and [coordinated publication
checklist](publication.md) for source repositories, release versions and artifact
boundaries.

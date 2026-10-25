# Customer runtime inventory

Ahu-maintained customer executables are implemented in Rust. Python is allowed
in development tests and tooling, including native CLI fixtures and migration
oracles. It is not an installation or runtime prerequisite. Skills and
declarative configuration remain text. This policy does not prescribe the
implementation language of external CLIs or harnesses.

| Component | Implementation | Distribution and scope |
| --- | --- | --- |
| ahu CLI, MCP, task coordination, worktrees, OTel and evals | Rust | Core executable |
| Codex adapter | Rust | Built into ordinary ahu; optional Rust launch-planner experiment |
| Claude Code adapter | Rust | Built into ordinary ahu |
| Antigravity adapter | Rust | Built into ordinary ahu |
| OpenCode adapter | Rust | Built into ordinary ahu |
| Kiro adapter | Rust | Built into ordinary ahu |
| GitHub CLI and tracker | Rust | Optional external packages; require native `gh` |
| cmux session host | Rust | Optional external package; requires native cmux and Git |
| text-stats authoring example | Rust | Independent example; build and stage before installation |

The five harness adapters are in `src/harness/`; their launch, native capability,
event and telemetry behavior is also exercised by the headless and integration
tests. Rust implementation does not establish current account availability or
live compatibility with every native version. Read the
[harness compatibility evidence](reference.md#harness-compatibility-evidence)
for execution-mode and evidence limits.

The GitHub and cmux migrations retain independent protocol fixtures, including
Python development oracles. Those files must never appear in the staged runtime
packages. Build artifacts are platform-specific and are pinned after staging;
ahu does not execute installation hooks or invoke Cargo during plugin install.
Review the plugin's README and security inventory before enabling it.

All-harness extraction is distinct from the runtime language policy. The
[Codex pilot](harness-plugin-pilot.md) has not yet moved native setup, trust,
events, usage and lifecycle into an external plugin. Future harness plugins
must prove agent work, actual MCP calls, attributed local OTel, eval outcomes
and cleanup separately for each advertised mode. Keep task/process authority
in core, make native limitations explicit, and never infer zero usage from
missing telemetry.

# ahu

ahu coordinates repository-defined coding agents across native harnesses. Each
agent has a pinned harness and model and runs an unattended task in a fresh Git
worktree, with results available for review from your coordinator session.

## Get started

Follow the [installation guide](docs/installation.md) to build ahu, install the
[harness plugins](docs/harness-plugins.md#packages) you need, and configure your
project. You need Git and an installed, authenticated native harness CLI.
Source builds require Rust/Cargo; installed executables do not require Python.

After setup, select an agent listed by `ahu agents`:

```sh
ahu doctor
ahu agents
ahu @dev-codex --prompt 'Review this repository and report findings.' --work-mode
ahu tasks
```

Replace `dev-codex` with your registered agent's name. Work mode requests native
automatic approvals; a Git worktree is not a sandbox. Review the task's changes
and results before integrating them. The [task workflow](docs/usage.md) covers
inspection, permissions, evaluations, and cleanup; `ahu help` lists commands.

## Learn more

See the [CLI reference](docs/reference.md) for command details and the
[architecture](docs/plugin-architecture.md) for core and plugin responsibilities.
GitHub tracking is optional; headless tasks do not require an issue tracker or
session host. Agents can also use [typed decisions](docs/typed-decisions.md) and
[evaluations](docs/evaluations.md) to measure their work.

Published versions and source downloads are on [GitHub Releases](https://github.com/moai-agent/ahu/releases).
For the upcoming coordinated release, see the [0.11.1 notes](docs/releases/0.11.1.md)
and [publication checklist](docs/publication.md). Contributors should start with
[the development guide](CONTRIBUTING.md).

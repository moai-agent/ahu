# Keep ahu core small

ahu coordinates agents running in existing harnesses. External services and
terminal products integrate through optional skills and plugins. A project can
require an integration for its own workflow without making it an ahu dependency.
Current-terminal and headless tasks work without cmux, GitHub, or a tracker.

## Ownership

| Core ahu | External integration |
| --- | --- |
| Harness selection, launch and supervision | Provider CLI/API calls and authentication |
| Task/attempt state, coordination and MCP tools | External issue identities, fields and transitions |
| Git branches, worktrees and task cleanup | Terminal groups, workspaces and provider ownership checks |
| Generic plugin discovery, pins, grants and bounded transports | Provider installation guidance and workflow skills |
| Bounded task telemetry and evaluation evidence | Provider-specific diagnostics and live conformance tests |

Harness adapters remain in core today. This boundary does not claim that every
harness is a dynamically installable plugin. Git worktrees separate changes;
they do not confine processes or filesystem access.

## Choose the smallest integration

Start with a skill when an agent can use an existing CLI to carry out a workflow.
GitHub issue and release workflows can use `gh` through the harness's tools.
Skills describe policy and procedure; they do not enforce authorization.

Use an [executable plugin](executable-plugins.md) when an operator needs a bounded,
machine-readable action. The current one-shot JSON interface is foreground and
operator-only. Its task-observation mode is read-only; it does not provide
automatic event delivery, tracker synchronization or task mutation authority.
An agent's direct CLI use is governed by harness permissions, not plugin grants.

Use a [session-host plugin](executable-hosts.md) when ahu must create, focus,
inspect or close an external terminal session. The cmux plugin owns its command
mapping and group/workspace state. Core keeps the frozen host identity and opaque
session reference, verifies the contract, and retains responsibility for harness
supervision and worktree cleanup. Headless execution does not select a host.

The GitHub packages and cmux package are separate external repositories and use
the same documented contracts available to independently authored packages.
Provider-specific retry, stale-write handling and error interpretation belong
there. Do not introduce a core provider registry, tracker client or duplicate
built-in integration to support a single package. The independent
[text statistics example](../examples/plugins/text-stats/README.md) demonstrates
the one-shot authoring path without either provider.

## Review a proposed core change

First ask whether a skill or existing plugin contract can implement it. A core
addition should express a shared task, harness or lifecycle requirement, with
provider-neutral inputs and behavior. Demonstrate it with an independent fixture
without provider binaries, accounts or network access. Do not expand the contract
solely to mirror one provider's API or promise a general framework before a
concrete workflow needs it.

Keep external work-item completion distinct from process exit, accepted work,
merge and task removal. A tracker failure must not replay an agent launch.
Coordinators review and merge agent work; workflow skills can explain that Git
flow without putting release orchestration into Rust.

## Security and evidence

Extraction removes provider-specific commands and state handling from core; it
does not eliminate their risk. Enabled executable plugins run as local code and
have no OS confinement provided by ahu. Pins and grants control ahu's invocation path, not every
action that a same-user process or harness tool can perform.

Discovery does not execute a plugin. Installation, activation and invocation
are separate; execution requires the relevant committed binding, verified package
and grant. Integrations that are turned off must not cause provider probes, credential
checks or network calls through ahu. Existing tasks retain their frozen host
identity; revocation or a missing host can block session operations rather than
silently switching providers or deleting uncertain work.

Test core contracts offline and provider behavior in each plugin. Record live
checks separately from fixtures and publication verification. Do not claim a
provider works because an independent fixture passed. Keep private bindings,
tracker content and raw telemetry out of public source and release artifacts.

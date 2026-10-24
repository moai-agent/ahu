# Codex harness plugin experiment

This branch tests an external **launch planner**, not a complete harness plugin.
The experiment is enabled at build time with `codex-plugin-pilot`. Normal builds
retain the existing adapters. Do not publish or install this experimental build
as a replacement for a released ahu binary.

## What the experiment proves

Registered Codex tasks use an installed `harness-codex` package to construct
interactive commands and headless work-mode commands. The external planner owns
those argument mappings. Core resolves the native executable, supplies the prompt,
checks the frozen command, and supervises the actual harness process.

The planner runs as a short-lived subprocess, not an in-process Rust library.
It uses a dedicated launch-planning protocol, separate from the one-shot action
and session-host protocols. It shares only executable verification, executable snapshots,
restricted process environment, bounded pipe exchange, and process cleanup.
The independent Python package demonstrates that the boundary does not require
linking Rust implementation types.

In a pilot build, missing installation, policy that is turned off, a missing grant, changed
package bytes, or invalid protocol output refuses registered Codex task launch.
The pilot refuses built-in launch-command fallback. Other harnesses are unchanged.

## Authority and identity

The committed binding ID is `harness-codex`, with an explicit `harness-launch`
grant. `execute` and `interactive-backend` do not authorize planning. Installation
and inventory do not run the planner. The package uses the existing source pin,
installation, activation, and removal mechanisms.

The command freezes a digest of the committed planner binding. Runtime command
reconstruction must reproduce both this digest and the argument vector. Updating
or removing the binding cannot silently substitute a planner for an existing task.
A current build without the feature reconstructs a different command and refuses
it. Older released binaries have not been certified to read pilot task records;
use the same pilot build until every experimental task is removed.

The request contains model, mode, permission selection, and optional native resume
identity. It contains no prompt, credentials, repository path, or native settings.
Core replaces exactly one final prompt placeholder after validating the response.
Returned executable identity must equal the selected harness name, not an arbitrary
path or shell. This is validation of a trusted local plugin contract, not a sandbox
or a proof that plugin code cannot perform other actions.

## Experimental protocol

The `harness_launch` manifest field declares `harness`, relative executable `path`,
`protocol_version: 1`, and `timeout_ms` between 100 and 30000. Only the executable
is copied into a private external directory. It runs there with
`--ahu-harness-launch-protocol=1`, a cleared environment, and `PATH=/usr/bin:/bin`.
Standard error is discarded; stdin/stdout are bounded to 64 KiB each. The deadline
covers process exchange; package verification occurs before that deadline.

One JSON request is sent, then stdin closes. A response must be one JSON object
and the planner must exit successfully. Unknown fields, duplicate keys, wrong
versions or identities, excess output, and invalid prompt positions are errors.
No retries occur. The planner must not launch the harness itself.

Request example:

```json
{"protocol_version":1,"harness":"codex","mode":"headless","model":"example-model","permissions":"auto","work_mode":true,"session":null}
```

Response fields are `protocol_version`, `harness`, `program`, `args`, and
`prompt_arg`. The indexed final argument must be exactly `<prompt: see prompt.txt>`.
The example package supports interactive prompt/accept-edits/auto mappings and
headless work mode with optional resume. Guarded headless profiles are explicitly
unsupported by this pilot. Interactive auto retains the current native
workspace-write mapping; it is not equivalent to unrestricted headless work mode.

## Validation and remaining extraction

Run from the repository root:

```sh
CARGO_TARGET_DIR=/tmp/ahu-harness-pilot-build cargo build --locked --features codex-plugin-pilot
CARGO_TARGET_DIR=/tmp/ahu-harness-pilot-build cargo test --locked --features codex-plugin-pilot --test harness_plugin --test plugin_authoring
```

The tests install a pinned package into a disposable ordinary Git repository,
compare launch commands with the existing adapter, exercise refusal paths, and
verify that the planner never receives the prompt. They do not require Codex,
a provider account, cmux, or GitHub. Real harness tests must separately establish
MCP use, local telemetry, results, integration, and normal cleanup.

This does **not** yet remove Codex from the catalog, setup, native trust/admission,
model selection, direct `ahu codex` shortcuts, enforcement reporting, event
parsing, resume policy, or usage accounting. Some native controls are still applied
by core after planning. The example's argument mappings are deliberately duplicated
for parity testing during this experiment, not a proposed permanent second adapter.

Before calling this a full harness extraction, define ownership for those remaining
surfaces, move the native behavior into the package, and prove both interactive
and headless lifecycle parity. In particular, keep core as the sole authority for
observed execution and task state. Evaluate a second harness before stabilizing a
shared contract. Do not expand the existing one-shot protocol into an agent runtime.

Planner exchange has a timeout and cleans up its owned process group on return.
The pilot does not yet connect interactive launch cancellation to that exchange.
Abrupt coordinator termination is not a guaranteed planner cleanup path; this
needs explicit lifecycle tests before promoting the contract to supported use.

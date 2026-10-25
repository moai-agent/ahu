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
The independent Rust executable has its own Cargo workspace and lockfile and
links no ahu implementation crates. Python is used only by independent test
fixtures, never by the packaged planner. The protocol remains language independent.
Prepare the platform-specific package using the [example build instructions](../examples/plugins/harness-codex/README.md)
before computing its digest or creating a binding. Installation copies verified
package contents; it does not build or execute the planner. Binaries stay outside
Git. This migration is not live provider certification.

In a pilot build, missing installation, policy that is turned off, a missing grant, changed
package bytes, or invalid protocol output refuses registered Codex task launch.
The pilot refuses built-in launch-command fallback. Other harnesses are unchanged.

## Discover and prepare a local package

`ahu setup --list-harness-plugins` reads `harness_launch` declarations from
explicit `AHU_PLUGIN_PATH` package directories and verified retained installations.
It works without a terminal, harness binary, provider account, or network access.
Normal interactive setup also shows this inventory. Discovery never executes
package code or provider probes, selects a candidate, or changes policy. Duplicate
IDs in `AHU_PLUGIN_PATH` are errors; multiple candidates for a harness are listed
as ambiguous. Retained versions may appear alongside staged packages.

After independently building and staging a package, run:

```sh
ahu setup --harness-plugin /absolute/staged/package
```

This explicit path selects the package independently of discovery order. Setup
validates the metadata, Cargo package name and explicit semantic version, declared
executable, and content digest using the installer rules. Source-only packages,
missing or non-executable planners, unsafe paths, and package symlinks are refused.
The package ID must be `harness-<declared harness>`. No registry or published package
is assumed, and setup never builds, downloads, installs, or runs the package.

Preparation creates `.agents/ahu/plugins/<id>.md` with a local source, SHA-256
revision/content pins, the current exact ahu compatibility version, `enabled: false`,
and no grants. It uses setup's conflict-checked writes, preserves identical files,
refuses different existing bindings, and refreshes `.agents/ahu/lock.md` without
silently accepting existing private-context drift. As with normal setup, a later
lock-refresh failure can leave prepared files for review; resolve the error and
refresh the lock before committing. Keep the staged package unchanged for install.

Review and commit the binding and lock. Then explicitly run `ahu plugins install
<id>`; use `ahu plugins update <id>` if a different version is already retained.
Installation reads committed pins, so preparation alone cannot authorize installing
the new binding. Inspect it with `ahu plugins installed`. To authorize the planner,
run `ahu plugins enable <id>`, explicitly add `harness-launch` to the working
binding's grants, then run `ahu lock --update`. Review and commit the enabled
binding and lock before launching. Enabling itself does not execute the planner.

Discovery and preparation are available in ordinary builds, but only registered
Codex task launches in a `codex-plugin-pilot` build use a planner. Preparing or
enabling another harness's declaration does not migrate that harness or add launch
support. The existing native adapters remain in core.

The setup consumer needs an offline CLI route to produce an installer-compatible
binding; the launch protocol is unchanged. Authority remains in reviewed committed
bindings. Focused setup tests cover ambiguous discovery, duplicate IDs, invalid
packages, conflict preservation, and preparation followed by explicit installation.

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
CARGO_TARGET_DIR=/tmp/ahu-rust-codex-agent-target cargo build --locked --manifest-path examples/plugins/harness-codex/Cargo.toml
CARGO_TARGET_DIR=/tmp/ahu-rust-codex-agent-target cargo build --locked --features codex-plugin-pilot
CARGO_TARGET_DIR=/tmp/ahu-rust-codex-agent-target cargo test --locked --features codex-plugin-pilot --test harness_plugin
```

The first build prepares the independent dependency cache. The tests explicitly
compile the independent Rust crate offline with its lockfile, then
install that real executable in a pinned package into a disposable ordinary Git repository,
compare launch commands with the existing adapter, exercise refusal paths, and
verify that the planner never receives the prompt. Direct executable tests cover
strict request validation, duplicate/missing fields, framing, and byte limits;
independent Python fixtures exercise malformed responses and pre-execution refusal. They do not require Codex,
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

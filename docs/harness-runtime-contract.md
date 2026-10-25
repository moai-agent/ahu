# Harness runtime extraction contract

This document defines the implementation boundary for replacing the launch-only
pilot. Completion requires deletion of the built-in provider paths and validation
of the integrated runtime; this document alone does not establish support.

## Ownership

Core owns task identity, permission authorization, process supervision, worktrees,
MCP coordination, context locking, generic setup transactions and eval acceptance.
An installed Rust plugin owns native flags, version/model semantics, native file
formats, admission, account observations, trust receipts and native event parsing.
The runtime plugin must not spawn or supervise the main agent process.

Only a reviewed, installed, enabled package with a `harness-runtime` grant may
execute runtime operations. Discovery reads verified metadata and never executes
code. Core has no built-in fallback. Freeze package/binding identity before an
attempt; updates cannot rebind existing tasks or cleanup operations silently.

## Installed descriptor

A `harness_runtime` declaration contains protocol version 1, the relative packaged
executable path, bounded operation timeout, and a descriptor. The descriptor
contains the harness ID, display name, native program, shortcut aliases, supported
operations/modes, native version evidence, suggested models, context paths and
native source/skill locations. These are provider-owned data, not a core registry.
Reject duplicate IDs/aliases and aliases colliding with core commands. Unknown
harnesses and model identifiers are not inherently invalid; the selected plugin
validates its model and native compatibility. Project rankings still select exact
identities without fallback.

Shared `ahu-harness-api` types describe this wire contract and generic validation
only. They contain no provider implementation, credentials or core dependency.
Plugin implementation helpers may be shared separately, but must not be linked
into core or depend on the ahu crate.

## Bounded operations

Invoke the packaged executable with `--ahu-harness-runtime-protocol=1`. Each
newline-terminated JSON frame contains `protocol_version`, an increasing integer
`id`, `operation`, and operation-specific `params`. The response echoes version
and ID and contains exactly one of `result` or a fixed, non-sensitive `error`.
Reject duplicate/unknown envelope fields, mismatched IDs, frames exceeding the byte limit,
partial output and unsolicited frames. No raw frame is written to telemetry.

Operations are explicitly enumerated: `probe`, `models`, `launch`, `admission`,
`setup`, `native_sources`, `auth`, `budget`, `trust_prepare`, `trust_cleanup`,
`decode_start`, `decode_event`, `decode_finish`, and `recover`. Unsupported
operations fail explicitly. Consumer-specific payloads have typed validation.
No generic shell-command or filesystem-operation response is accepted.

Launch uses an explicit interactive, headless or direct mode and contains exact
model, permission/profile, version, timeout and resume inputs. A returned plan
names the declared native program and argument vector with one final prompt
placeholder when a prompt is supplied. Core inserts the prompt after validation.
Native environment changes are bounded named entries/removals, never permission
to alter the coordinator environment or opaque task identity.

Native inspection receives explicitly selected paths and native executable;
account observations never return raw credentials. Core retains profile storage
and fingerprint comparison. Setup returns bounded proposed file changes with
expected-before digests; core applies them through conflict-checked transactions.
Native trust operations use an explicit task context, persist intent before
mutation, retain exact receipts and reconcile uncertain outcomes. A timeout is
not proof that a mutation did not occur, and must not trigger automatic replay.

## Native event normalization

One supervised normalizer process owns decoder state for one attempt. Core sends
`decode_start`, ordered `decode_event` frames identifying stdout or stderr, and
`decode_finish` after draining native streams. Responses provide bounded normalized
observations; they do not set task state. Core still requires process outcome,
validated native completion and successful decoder finalization before success.

Keep existing usage semantics, replay exclusion, model-identity uncertainty,
skill/helper observations, trajectory deduplication and write-target observations.
Unknown measurements remain unknown. Provider credits are not tokens or USD.
Native observations are evidence, not proof of a write or instruction adherence.
Do not retain raw native events, tool arguments, reasoning or transcripts.

Bound frame bytes, queues, subprocess output and deadlines. Drain native stdout
and stderr concurrently. Decoder death, malformed responses, blocked I/O or
cancellation must stop owned processes and report incomplete evidence; never
substitute exit status or replay an agent automatically. The normalizer shares
owned-process cleanup rules but has no authority over unrelated native services.

## Validation

An invented provider must work without core source changes. Exercise ordinary CLI
launch, direct mode, setup, discovery and doctor through installed capabilities.
Move native parser tests with the implementation; keep independent malformed
protocol fixtures in core. Verify missing/revoked/tampered plugins, resume identity,
setup conflicts, trust reconciliation, interrupted decoding, timeout and cleanup.

Live validation separately records native version, model, execution mode, actual
MCP calls, task-attributed local OTel, deterministic evals and cleanup. Account or
quota failures are explicit skips, never successful conformance. Keep raw traces
outside repositories and publication artifacts.

# Native harness event decoder

`ahu-harness-events` is Rust implementation support for external runtime plugins.
It is a standalone workspace, with its own lockfile, and has no dependency on ahu.
Core must not link this crate. The public boundary is `serde_json::Value`; the
internal structs are not a second shared wire API.

## Wrapper integration

Create exactly one `Decoder::new(harness)` per attempt. Supported IDs are `codex`,
`claude-code`, `antigravity`, `opencode`, and `kiro`. Feed each complete stdout
JSON line to `observe_stdout`, and each stderr line to `observe_stderr`, in receipt
order. Both return **cumulative snapshots**, not deltas: replace observations,
never sum successive snapshots. Do not feed stderr into the stdout parser.

Call `finish` after draining both streams. It is idempotent and freezes observation
input; it does not fabricate a missing terminal result or decide task acceptance.
Input after finish fails. `snapshot` does not finalize.
A boundary error returns a fixed, non-sensitive `Error` and permanently marks
failure and incomplete evidence; `snapshot` remains available for diagnostics.
Valid native failure events return `Ok` with `failed: true`. Wrappers must handle
both failure paths, preserve decoder errors, and must not infer task acceptance
from `Ok`, `terminal`, a summary, or process exit alone.

The wrapper owns `decode_start/event/finish` framing, request correlation, byte
encoding, process transport, timeout, cancellation, and cleanup. This library
performs no filesystem access, persistence, network access, process launching,
account probing, or native-home lookup. It retains no raw event, reasoning, tool
arguments, or tool result payload. Temporary parsed JSON is dropped after each
call. **Snapshots include ephemeral final answer text in `summary`: never log,
trace, persist, or include a whole snapshot in an error.** Core may inspect that
text in memory for acceptance and must exclude it from durable results.

## Snapshot fields (schema_version 1)

All fields below are cumulative. Missing/null measurements mean unknown, never
zero. Nested optional fields may be omitted rather than null. Future additive
fields should be ignored by wrappers; incompatible versions must be rejected.

| Field | Meaning |
| --- | --- |
| `schema_version`, `harness`, `finished` | Schema, selected provider, and whether both streams were declared drained. |
| `session` | First valid native session reference, or null. Invalid/conflicting aliases and session drift fail the attempt. Kiro also checks replay identities. |
| `terminal`, `terminal_evidence`, `failed` | Native completion evidence and sticky observed failure. `terminal_evidence` is null for stream observations. These do not set task state. |
| `summary` | Last supported final text, at most 64 KiB, **memory only**. Oversized text is discarded with an explicit error. No reasoning/tool output is substituted. |
| `blockers`, `unknown_events` | Fixed diagnostic categories and unsupported top-level event count. Unknown evidence is not successful zero-work evidence. |
| `stderr_diagnostics` | Deduplicated permission, quota/rate-limit, or authentication failure categories. Raw stderr is discarded. |
| `stderr_unclassified_lines`, `stderr_informational_lines` | Counts of nonempty unknown stderr and the two exact native stdin-progress messages. |
| `usage` | `input`, `output`, `cached`, `cache_write`, `reasoning`, `total`: independent maximum reported fields, not additive call totals or a coherent billing snapshot. |
| `cost` | Optional `usd` and `source`. Claude cumulative result maximum; OpenCode sum of finite nonnegative `step_finish.part.cost`, once per valid part ID. Never billing attestation. |
| `cost_tracking_limited` | Cost dedup capacity, invalid identity, or arithmetic loss. A partial cost must not be treated as a complete total. |
| `model` | First syntactically safe stream selection, or latest safe Kiro configuration selection. This is not attested inference identity. |
| `model_attributes` | Map containing `ahu.harness.model.reported.coverage` (`partial`, `complete`, `conflicting`, `redacted`) and, when consistent and safe, `ahu.harness.model.reported`. Kiro selection never becomes reported inference identity. |
| `skills` | Bounded name, harness, status (`invoked`, `completed`, `failed`), evidence, execution evidence, optional timing/provenance. Never arguments, call IDs, or result bodies. |
| `skill_observation`, `skill_unknown_events` | Evidence classification and malformed/ambiguous skill correlation count. Reused IDs do not establish unique completion. |
| `native` | Helper records, task-kind classifications, background shell IDs, refusal counters, reported spawn/completion/depth/role counts, unknown count, terminal visibility, and `stream_incomplete`. Helpers retain native ID, role, depth, status, background flag, output-file reference, and token count. Their `summary` is always empty. File references are never opened. |
| `native_completeness`, `native_blockers` | Lifecycle/visibility assessment: joined, unjoined, unobserved work, unknowns, and evidence completeness. `violations` is empty: frozen policy/depth/budget enforcement belongs to core. A completed lifecycle is not necessarily a successful helper or an accepted task; core must inspect statuses and apply its policy. |
| `trajectory` | Native stream source/coverage, optional steps/calls/completed calls/errors/repeats/recoveries, malformed/unclassified counts, tracking limit and incomplete flags. IDs/tool names are hashed internally and never serialized. |
| `trajectory.metering` | Optional context-window latest/peak percentages, attributed prompt completions, observed credits and elapsed milliseconds, counts of observations/duplicates/rejected/unattributed completions. Credits are not USD or tokens; prompt completions are not model steps. |
| `write_target_candidates` | Sorted, unique native write-path strings. These are untrusted candidates, including relative paths, not proof of a write or an outside-worktree verdict. Core resolves paths and applies worktree and structured-result exceptions. |

Skill receipt intervals use a monotonic clock, bounded to `u64` milliseconds.
Absolute `observed_at` and `completed_at` are omitted/null because this API has no
trusted wall-clock input; wrappers may add their own receipt timestamps. Skill
source/digest attribution remains with the caller's trusted catalog.

## Provider semantics and limits

The parsers preserve the existing tested native shapes, including Kiro V3 ACP
2.28/2.29 replay exclusion, unresolved approvals/tools/errors, credit attribution,
request reuse detection, and context windows. Kiro trajectory is always partial.
OpenCode reasoning and new step activity clear terminal status; only a final
`stop` step establishes stream completion. Duplicate cost/trajectory snapshots
are deduplicated independently. Duplicate Codex/Claude/Kiro terminal events remain
failures; there is deliberately no blanket line deduplication. Claude helper
classification distinguishes `local_bash` from helper work, including unknown
agent kinds carrying a role. Codex collaboration without joinable native IDs
remains unknown. Antigravity has outcome/skill/usage support, with unsupported
trajectory measurements left unknown.

Bounds are applied before parsing: 1 MiB per native line, 64 MiB combined input
per attempt (empty calls consume one byte), and JSON nesting under 64 levels.
Summary is bounded to 64 KiB; diagnostics to 32 categories; write candidates to
256 entries of 4096 bytes without control characters. Skills are limited to 128;
skill identities to 128 bytes. Helper event evaluation is limited to 256 events;
projected metadata strings/keys to 4096 bytes and collections to 256 entries;
accumulated role counters are also capped at 256. Cost and trajectory identity
sets are capped at 4096; trajectory tool identities at 256. Kiro tool/approval
maps are capped at 4096 and approval choices at 32. Bounds that lose evidence
produce failure or an explicit limited/partial observation; no zero fallback.
Response bounds must account for cumulative observations and JSON escaping, not
just the size of the last native line; the transport must reject oversized
responses as incomplete evidence without retrying an agent.

## Pure recovery parser

`recovery::export_establishes_completion(export, session, attempt_started_ms)`
accepts an already exported OpenCode session JSON string. It checks a 1 MiB byte
bound, session identity, succeeded/idle state, creation time relative to the
supplied attempt epoch milliseconds, one prompt, completed tool calls, final
assistant stop and final idle success. Message count is capped at 4096. It returns
only success or a fixed reason, with no transcript projection. A native-services
plugin owns bounded export retrieval and version eligibility (the original
recovery gate was OpenCode 2.0.32); core owns eligibility after process outcome,
prior failures, and resume checks. This helper does not mutate a Decoder, clear
failure, or automatically recover missing stream evidence.

Codex app-server account/budget/hook/config inspection in `codex_metadata.rs` is a
separate request/response service, not headless attempt stdout. Its process and
RPC state remain with the native-services extraction; this crate does not launch
app-server or reinterpret metadata RPC responses as attempt events.

## Extraction and validation

Parser logic and synthetic tests were extracted from `headless` Events, usage,
cost, identity, skills and Kiro modules; native helper Observations; eval trajectory
Observer; and OpenCode's pure recorded-completion parser. Generic eval budgets,
scoring, policy planning, filesystem resolution, launch, transport and task
acceptance were excluded. The extraction adds explicit API input/depth/summary/
write bounds, invalid session-alias rejection, safe model selection, bounded
accumulated helper roles, and loss propagation for helper coverage. Tests retain
native parsing assertions; policy/process assertions stay with core. Absolute
skill timestamps are intentionally no longer fabricated inside the parser.

Run focused checks from this directory:

```sh
export CARGO_TARGET_DIR=/tmp/ahu-full-plugin-events-target
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Tests use synthetic in-memory events. They do not establish live native version
compatibility, installed-plugin conformance, transport behavior, or integration
with core. No full core suite, coverage run, paid model calls, native process
retrieval, or external service tests belong to this crate's focused validation.

The only dependencies are serde/serde_json for the value boundary and sha2 for
bounded, non-serialized trajectory identity fingerprints. Manifest/lock review
and the repository's dependency security scan remain release integration gates.

## Core integration contract

Core `headless::decoder` consumes this schema without linking this crate. A single
runtime session receives `decode_start {"harness":ID}`, ordered
`decode_event {"stream":"stdout"|"stderr","line":UTF8_STRING}` requests and
`decode_finish {}` only after both pipe readers report complete drain. The
worker serializes calls from a bounded 16-line queue; readers apply cancellable,
10-second bounded backpressure. The supervisor never holds its observation lock
across a plugin call. Plugin errors, wrong schema/harness, invalid bounds,
incomplete drains and decoder finalization failure prevent acceptance. Input is
not retried. Non-UTF-8 native lines and unterminated stdout are explicit failures.

Runtime frames have a 1 MiB limit including escaping/envelopes; a native line or
cumulative snapshot that cannot fit fails closed. Core also checks the snapshot's
1 MiB size, required field set, nesting/count/string bounds, finite amounts,
session consistency and cumulative usage/counter behavior. It discards plugin
lifecycle verdicts after shape validation and recomputes helper policy assessment
from normalized observations and the frozen profile. Core resolves normalized
write candidates against the actual worktree and its exact structured-result
exception. Generic scoring and policy remain in core.

Finalization is deliberately separate from native completion: a finalized decoder
may return `terminal:false, failed:false`. Core may then request recorded recovery
only for a fresh session, clean process exit, successful decoder finalization,
known session, no blockers/unknown events and no prior failure. Core always
requires terminal evidence before accepting success. Previously this crate marked
missing terminal as failure during `finish`; that decision now belongs to core so
eligible recovery cannot erase an observed failure.

The headless launch operation matches the coordinator's runtime launch payload:

```json
{"mode":"headless","model":"exact-model","permissions":"auto","work_mode":true,"session":null,"native_version":"1.2.3","timeout_seconds":1800,"cwd":"/task/worktree","broker_dir":"/task/requests","native_profile":null,"native_controls":[]}
```

The response is exactly `{program,args,prompt_arg,native_eval_otlp}`. `program` must match the
installed runtime descriptor; args must end with one literal
`<prompt: see prompt.txt>` placeholder at `prompt_arg`. Core substitutes the
actual prompt after validating bounds and freezes `Runtime::binding_digest()` in
`LaunchCommand.planner_binding_digest`. Decoder startup compares that frozen
binding before invoking any native model process. No launch-only pilot or built-in
adapter is used by the headless command builder.

Native-services payloads (implemented by the separate native-services package):

- `admission {"action":"native_profile","project":ABSOLUTE_PROJECT,"request":REQUEST}`
  returns the serialized core-compatible `native::Profile` directly. REQUEST
  contains `harness`, `harness_version`, `policy`, `session_model`, `helper_model`,
  `helper_role`, `max_concurrent`, `max_depth`, `budget_usd`, `assignment_writes`.
  This is pure native policy/flag planning. It must include invocation/isolation
  controls in `enforced`; core no longer adds provider-specific isolation flags.
  The service must explicitly distinguish this request from its existing full
  configuration-inspection admission payload. Core validates the requested
  identity/policy/ceilings and rejects profile environment changes to host-owned
  HOME/PATH/PWD, AHU_* or OTEL_* variables. Repository-aware callers use `native::profile_at`; there is no ambient profile API.
- `recover {"harness":ID,"native_version":VERSION,"native_executable":PATH,"expected_executable_digest":DIGEST,"worktree":PATH,"session":SESSION,"attempt_started_ms":EPOCH_MS,"home":EXTERNAL_HOME,"environment":DECLARED_NATIVE_ENVIRONMENT}`
  returns exactly `{"recovered":true|false}`. Core invokes it only if the frozen
  runtime declares `recover`. The service owns native version eligibility,
  executable digest verification, bounded external-home session retrieval and
  pure format validation. Unsupported recovery returns false or a fixed refusal;
  it must never launch another model attempt. Core records a generic recovery
  provenance label and cannot clear prior failure through this operation.

The bridge uses sibling `Session::call_cancellable` for event/final decoder calls,
then `Session::finish` for clean EOF/exit and group cleanup. The latter currently
has an operation deadline but no cancellation argument; cancellation during that
EOF/exit phase can therefore wait for the runtime operation deadline. Core times
out its final drain wait and marks evidence incomplete rather than accepting a
late result. The worker is cancelled and joined on drop, so even early returns
wait for owned decoder cleanup rather than leaving its process behind. Making that final peer cleanup directly cancellable is a transport
integration follow-up, not authority to restore built-in decoding.

Legacy tests outside the assigned source files that directly call
`Events::observe`, `Observer::observe`, or `native::Observations::observe` must
move to plugin tests or consume normalized fixtures. Those methods no longer
exist in core. Parser regression coverage lives in this standalone package;
core boundary tests use an invented harness and normalized observations.

Cost provenance uses `native_cumulative_usd` for the maximum reported cumulative
amount and `native_step_sum_usd` for the sum of first valid step amounts. Core
must consume this observed source together with the frozen runtime identity;
it must not reconstruct cost provenance from a harness name.

Core integration regressions include an independent JSONL peer for malformed
responses, invalid snapshots, explicit errors, mixed streams, cancellation,
EOF timeout and owned-process cleanup. The native parser regression suite lives
here; core telemetry/evaluation tests inject normalized observations.

### Invocation capability admission

Core uses the native-services runtime admission operation before structured
output or broker dispatch. Request:
`{"action":"execution_capabilities","harness":ID,"native_version":VERSION,"permissions":"prompt"|"auto"|"accept-edits","work_mode":BOOL}`.
The exact response is `{"structured_output":BOOL,"broker_transport":BOOL}`.
Both fields are mandatory booleans; unsupported/malformed responses refuse the
requested feature. The native service must preserve native read-only restrictions
(including both false for a Codex prompt/read-only invocation). Broker dispatch
also checks the runtime binding against its frozen parent. No provider fallback
or prompt-based permission override exists in core.

Trajectory `source` is a 1–128 byte provenance label using ASCII letters, digits,
periods, underscores or hyphens. `unsupported` is the explicit absence sentinel;
other bounded labels are plugin-owned and need no core enum variant. Optional
`trajectory.metering` has provider-independent context percentage, observed credit,
elapsed-millisecond and completion counters. Its amounts are partial observations,
never interchangeable with tokens or USD. Core exports these as `ahu.metering.*`
and groups them under `metering_*` keys. Old provider-specific aliases are rejected.

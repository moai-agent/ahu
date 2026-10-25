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

Call `finish` after draining both streams. It is idempotent and checks for a
native terminal result; input after finish fails. `snapshot` does not finalize.
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
| `trajectory.kiro_metering` | Optional context-window latest/peak percentages, attributed prompt completions, observed credits and elapsed milliseconds, counts of observations/duplicates/rejected/unattributed completions. Credits are not USD or tokens; prompt completions are not model steps. |
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

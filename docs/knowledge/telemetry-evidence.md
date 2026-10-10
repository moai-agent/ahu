---
type: Architecture
title: Telemetry evidence and privacy boundaries
description: Release attribution through OpenTelemetry, usage and cost semantics, and collection coverage.
tags: [telemetry, privacy, evals, cost]
status: draft
generated: { by: docs-astra/1.1.2, at: 2026-10-03T00:00:00Z }
sources:
  - id: telemetry
    resource: ../../src/telemetry.rs
    title: Opt-in local OTLP and normalized metrics
  - id: private
    resource: ../../src/telemetry/private.rs
    title: In-memory private mapping and numeric summary boundary
  - id: headless
    resource: ../../src/headless.rs
    title: Headless usage and cost normalization
  - id: eval
    resource: ../../src/eval.rs
    title: Eval aggregation and comparison output
---

# Telemetry evidence and privacy boundaries

ahu records observations from harness event streams. An observed token count is
not a price; a harness-reported USD amount is not a provider invoice; an absent
quota field is not evidence of remaining capacity. Reports preserve those
distinctions and leave unavailable values missing.[^headless][^eval]

## Collection path

```mermaid
flowchart LR
    Harness["Headless harness event stream"] --> Normalize["Bounded event normalizer"]
    Normalize --> Result["Task attempt result"]
    Normalize --> Span["Opt-in ahu OTLP span"]
    Span --> Collector["Local OpenTelemetry Collector"]
    Collector --> Backend["Telemetry backend: release queries and coverage"]
    Result --> Eval["Eval report aggregation"]
    Result --> Local["Opt-in local numeric metrics"]
    Host["Owner-only host mapping store"] --> Private["Private grouped report"]
    Local -. "explicit linked task membership" .-> Private
    Private -. "never exported" .-> Span
```

Token fields are normalized from the harness stream when present. Claude Code's
result `total_cost_usd` and OpenCode's observed step costs are recorded as
`ahu.cost.harness_reported_usd`, with a bounded source label. OpenCode step
events are deduplicated by part ID. The cost field does not claim final billing,
and ahu does not derive cost from token counts or catalog prices. Codex and
Antigravity currently provide no documented per-run USD field to this adapter.
Claude cumulative result amounts use the maximum valid observation, so repeating
a result cannot add the same amount twice. Kiro's accepted prompt-completion
credits are exported separately from USD, with partial coverage. The user
reference describes the current source and scope for each harness.[^headless]

## Release measurement through OTel

Use the existing OTLP pipeline to group execution evidence by release. Set a
public-safe, opaque `ahu.release.id` in `OTEL_RESOURCE_ATTRIBUTES` when creating
tasks. ahu freezes it in the existing task identity; a resume keeps that label
and a broker child inherits its parent's label. The release label is not a
private tracker reference. It does not create another mapping or accounting
store. Older tasks without a label remain unassigned.[^telemetry]

The backend should query `ahu.harness.run` spans by release, task, attempt,
harness, model, execution mode and outcome. Task root and parent attributes
describe the execution hierarchy. Deduplicate retransmitted spans by trace and
span ID before counting observations. Do not sum token maxima, cumulative
session costs, and child costs into a release total: resumed and nested usage
may overlap. The exported aggregation, scope and coverage attributes identify
what each observation supports. Missing numbers remain missing; observed zero
is a number. Complete native event capture does not establish complete cost
coverage.[^headless][^telemetry]

Collection must be enabled before the work starts and verified at the backend.
A configured exporter or reachable port alone does not prove delivery. Monitor
collector refusals, exporter failures, queue exhaustion and retention; retain
the accounting evidence without sampling if it is used for totals. ahu's
bounded asynchronous export can lose observations on overload or abrupt exit.
Report unavailable harnesses and interactive usage explicitly. A release label
provides attribution, not a guarantee that all release work was observed.

Use native per-request usage metrics when their documented units and temporality
support aggregation, and keep them separate from ahu's attempt-level observations
unless overlap has been resolved. ahu does not relabel per-field maxima as
standard GenAI per-request token metrics or convert Kiro credits into USD.
Until providers expose sufficient disjoint measurements and collection coverage
is verified, a backend report must call its result observed usage or cost,
rather than total release spend.

`local_metrics` is opt-in and does not require an OTLP collector. Schema version
2 contains the normalized token fields, elapsed process duration, and an
observed/unavailable USD field. An observed zero is distinct from an unavailable
value. Per-attempt token
snapshots use the maximum reported value for each field; that rule does not
claim an additive task total. Eval reports keep USD fields separate from token
means and show observation counts against group run counts.[^telemetry][^eval]

## Data classification

| Data | Local task result | OTLP span | Private mapping summary |
| --- | --- | --- | --- |
| Task, harness, model identity | Existing bounded result metadata | Allowlisted identity attributes | Caller supplies repository digest and task UUIDs |
| Token usage and harness-reported USD | Numeric observation with provenance | Numeric attributes, plus a bounded cost source label | Numeric-only summary; no inferred cost |
| Opaque local record key | Host-private mapping only | Not accepted as an OTLP attribute | Excluded from numeric summary |
| Tracker title, address, or body | Not read or stored | Not accepted as an OTLP attribute | Not accepted |
| Prompt, transcript, tool arguments, credentials, account history | Excluded from normalized metrics | Excluded from ahu's normalized spans | Not accepted by the summary input |

The telemetry endpoint is restricted to a local collector. A local collector
may have its own exporters and retention; ahu cannot establish those policies.
When OTLP export is enabled, ahu-configured child harnesses also receive the
loopback endpoint and export their own metrics and logs. Inherited resource
attributes are filtered to bounded, allowlisted ahu identity fields when ahu
configures child export. That filtering does not inspect child-produced payloads:
ahu's attribute allowlist and redaction apply to attributes ahu itself emits; child
harness and SDK log/metric payloads may carry unredacted metadata from the
child's environment, so the local collector, not ahu, is the redaction and
retention boundary for child-emitted telemetry.
The opaque mapping key and tracker content must not be added to resource
attributes, task records, prompts, public reports, or committed repository
files. The private mapping type intentionally lacks `Debug` and `Serialize`. Its summary
contains a validated repository digest and optional agent digest with harness,
model, and outcome labels so reports can group comparable runs. It also reports
timing coverage, per-field maximum token observations, and mean harness-reported
cost by source. It does not pool observations across different groups or cost
sources, and does not include the mapping key, task IDs, or paths.[^telemetry][^private]

## Private association lifecycle

`ahu telemetry link|unlink` maintains explicit task membership under the user's
owner-only host state directory, outside all checkouts. The key is an opaque
local string and ahu does not contact a tracker or verify its visibility. The
mapping store is schema-versioned, bounded, symlink-resistant, locked during
updates, and atomically written. It accepts validated task references only;
task records, prompts, configuration, MCP, and OTLP never receive the key.[^private]

Retries and resumes retain a task UUID and are distinguished by attempt number.
Identical duplicate observations for an attempt count once; conflicting values
for the same task and attempt fail the summary. A child task must be explicitly
included in the mapping. Merging a worktree does not change its task UUID.
`ahu remove` leaves the mapping intact; reports show missing task records, and
`ahu telemetry unlink` removes one task or the whole mapping explicitly.
Mappings are not synchronized between machines and do not expire automatically.
No automatic traversal of related task directories occurs.[^private]

## Threat boundaries and gaps

The parser rejects unknown fields, caps input size and task/observation counts,
validates canonical identifiers, and returns a fixed redacted error. Its
numeric summary does not emit task IDs, repository paths, arbitrary attributes,
or the private record key. This prevents accidental disclosure through this
library API; it does not authenticate a mapping, prove tracker visibility, or
make an insecure host store safe.[^private]

`ahu telemetry report` reads only result envelopes with validated task ownership
and extracts opted-in numeric projections. It groups by agent identity,
harness, model, and outcome, and shows timing, per-field token observations,
coverage, and source-separated harness cost. Attempts without local metric
projections remain unavailable in their groups. ahu has no automatic quota
query, final billing API, interactive-session usage reader, tracker client, or
automatic issue visibility check. Every report explicitly shows capacity as
unknown because this path collects no per-run or account capacity signal.
Account rate-limit windows are read only by an explicit `ahu auth budget`
request against the active auth profile; that view is a separate signal and
never joins a telemetry report, a run record, or an eval projection.
Each group separately counts complete, incomplete, and unknown native-event
evidence so process exit status is not mistaken for a complete harness stream.
Offline fixtures verify adapter behavior; they do not prove live provider
billing or quota.

[^headless]: `src/headless.rs` normalizes bounded harness events and persists attempt results.
[^eval]: `src/eval.rs` aggregates observed values and renders eval comparisons.
[^telemetry]: `src/telemetry.rs` configures local-only opt-in OTLP and local metrics.
[^private]: `src/telemetry/private.rs` validates the private mapping and numeric summary input.

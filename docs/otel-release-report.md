# Review a release from local OpenTelemetry

`ahu telemetry report` can read standard OTLP JSONL files written by a local
OpenTelemetry Collector. This mode reads files and prints a report. It does not
create a database, link tracker records, contact billing APIs, or enable the
optional local metrics store.

Set `OTEL_RESOURCE_ATTRIBUTES=ahu.release.id=candidate-a` before launching work.
Use an opaque public release label, without private tracker references. Task
identity and release attribution are frozen when a task starts. Existing tasks
do not acquire a new label from a later shell environment.

Keep captures, the selection file, and reports outside every repository. A
selection is an explicit array of task attempts to inspect, including attempts
for which no telemetry arrived:

```json
[
  {
    "task_id": "example-task",
    "attempt": 1,
    "harness": "kiro",
    "model": "claude-sonnet-4.5",
    "mode": "headless",
    "role": "implementation",
    "provider": null,
    "telemetry_enabled": true
  }
]
```

The provider and role are declared labels. They are not backend attestation.
Use `null` for unknown provider or telemetry policy. Task IDs and attempts must
match the actual task records. Include retries separately, and include both
parents and registered children when reviewing their coverage.

```sh
ahu telemetry report \
  --otlp /absolute/external/path/traces.jsonl \
  --selection /absolute/external/path/selection.json \
  --release candidate-a --output json
```

Repeat `--otlp` to include rotated files. Duplicate trace/span identities count
once. Conflicting duplicates, conflicting task identity, or multiple terminal
run spans make that attempt's measurements unknown. Files must contain complete
JSONL records; use a stable capture when the collector is actively writing.
Malformed or oversized input fails with a bounded error instead of a partial
success report. Input limits are 32 capture files, 128 MiB per file, 16 MiB per
line, 4,096 selected attempts, and 200,000 matching spans.

The report shows per-attempt measurements and coverage grouped by harness,
declared provider, requested model, execution mode, and role. Each metric has an
observed and unknown denominator. An observed zero stays zero; missing data is
`null`. Group maxima describe observations, not consumption totals. Reported USD
retains its harness source and is not invoice-grade. Kiro credits, elapsed time,
and context percentages remain separate from token and USD measurements.

The report never adds cumulative retries, parents and children, or repeated
terminal snapshots into a release total. Cross-attempt and parent/child
additivity are not established by the native measurements. Interactive usage
remains unknown even when MCP activity is observed. A missing terminal span can
mean unfinished work, export loss, or missing coverage; it does not establish
task failure. A tool-call span establishes an observed invocation, not successful
task acceptance. Review the tool result and code validation separately.

Compare the observed service versions, MCP-call coverage, terminal outcomes,
missing measurements, and independent acceptance with the expectations recorded
before the assignments. Keep that comparison and all telemetry local. Never put
captures or private release evidence in public release artifacts.

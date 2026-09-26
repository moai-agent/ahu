# Local agent evaluations

This is a small, repeatable starting point for comparing ahu-launched local
agents. The checked-in cases are synthetic. Run evidence and OpenTelemetry
traces belong in a user-owned directory outside this checkout; never commit
prompts, transcripts, task results, or trace exports.

`ahu eval run` orchestrates repeated candidate runs and optionally a separate
ahu evaluator agent. It validates answer and score artifacts, writes compact
JSONL records, and captures ahu OpenTelemetry spans through a temporary local
receiver. `ahu eval report` compares those records. The lower-level
`scripts/local_eval.py record` and `trend` commands remain available for manual
or older workflows.

## What is measured

Each run records the case and corpus version, model identifier, harness and
ahu versions, skill bundle revision, task outcome, a deterministic case score,
and reported token and timing metrics when available. Keep the same case set
and scoring rules when comparing model or skill changes. Compare repeated runs
as well as averages: model output can vary between attempts.

ahu's headless and MCP spans are the observability stream. `ahu eval run`
temporarily routes candidate and evaluator OTLP/HTTP exports to a receiver bound
to loopback on an ephemeral port. It needs no collector binary, backend, or
query API and does not change the project's saved telemetry settings. The
receiver holds only bounded ahu identifiers, trace IDs, durations, and MCP
counters in memory; raw spans and payload text are discarded. The normal ahu
spans report lifecycle, token usage, skills, tool listings and calls, failures,
and typed-decision outcomes. MCP spans are joined to task attempts through
`ahu.task.id` and `ahu.task.attempt`; eval case, corpus, run, and stage IDs are
also attached as resource attributes.

MCP counters are observed only when the MCP process exports its session summary.
If the harness does not start the configured `ahu mcp serve` entry or telemetry
is unavailable, the record marks MCP coverage as missing. Missing values stay
missing; they are never turned into zero. The eval runner also enables ahu's
local token projection for its result envelope, without changing repository
configuration. This reports only the harness's observed counters, not provider
billing totals.

OTLP has no vendor-neutral query API. The runner avoids that dependency by
receiving the exports directly for the duration of each eval command and
projecting the relevant fields into the run record. `ahu eval report` reads
those compact records only; it never queries or retains traces.

## Running a case

Use a disposable Git repository for every run. This keeps ahu's task state,
worktrees, prompts, and harness-created artifacts outside the source checkout.
Configure the temporary project with:

- an OpenCode agent manifest pinned to the local Ollama model;
- the same skill bundle that the tested agents should receive;
- an OpenCode local MCP server entry that launches `ahu mcp serve` and sets
  `AHU_DECISION_URL=http://127.0.0.1:8001/v1/decisions`;
- A registered candidate and, optionally, evaluator agent.

Start Ollama and `python3 examples/ollama_decision_service.py --model <installed-local-model>`
outside the checkout. Run the suite against the disposable project while
pointing at the case file in the ahu checkout:

```sh
ahu --repo "$EVAL_WORKSPACE" eval run \
  --case "$AHU_CHECKOUT/evals/cases/decision-routing.json" \
  --agent @triage --evaluator @judge --runs 5 \
  --records "$EVAL_HOME/runs.jsonl" --output json
```

Omit `--evaluator @judge` to use the case's deterministic expected-answer score.
With an evaluator, the candidate receives only the case state and typed
questions; its `answer.json` is checked against deterministic expected values
and the blinded evaluator scores the rubric without candidate identity, model,
harness, trace, or expected answer. The score artifact must match the versioned
schema. Repeated runs are separate ahu tasks and contribute independent records.

The command never widens manifest approvals implicitly. Pass
`--allow-widened-approvals` only when the candidate or evaluator manifest
explicitly requires that permission and you intend to grant it. Keep each agent
run fully local: an Ollama model name must have local weights and its provider
endpoint must be localhost. `ahu` pins the manifest's harness and model but
does not prove that a provider alias routes only to local inference.

The initial case is `cases/decision-routing.json`. The expected decision is
known, so its score is deterministic and can be compared over time. Add cases
that probe separate skills and decision types rather than making one large
benchmark prompt. Version changes to cases or scoring rules explicitly; do not
silently change the baseline.

## Longitudinal record

Store one compact JSON object per run in an external JSONL file. Include the
case ID and version, timestamp, model (including quantization/tag), harness
and version, ahu version/commit, skill digest, task ID/attempt, outcome, score,
observed token fields, elapsed time, decision-call count, and the corresponding
OTel trace ID. Keep this record numeric and non-sensitive: omit prompts,
subjects, answers, paths, issue references, and transcripts. Report mean score,
pass rate, and token/time coverage separately so a missing observation does
not look like a zero.

## Record and compare

`ahu eval run` writes each accepted answer, task result, optional judge score,
and prompt into a private (`0700` directory, `0600` files on Unix) run folder
next to the external JSONL file. Each JSONL row omits prompts, transcripts,
paths, and answer values. It contains scores, task IDs, compact token fields,
timing and MCP observations. Keep that folder and records outside the checkout.

For manual scoring, `scripts/local_eval.py record` still accepts a structured
answer such as `{"department":"billing","refund_requested":0.96}` and an
`ahu result --output json` envelope:

```sh
python3 scripts/local_eval.py record \
  --answer "$EVAL_HOME/current/answer.json" \
  --result "$EVAL_HOME/current/result.json" \
  --agent '@triage' --agent-version 1.0.0 \
  --evaluator '@judge' --evaluator-version 1.0.0 \
  --run-id "$EVAL_RUN_ID" --stage candidate \
  --model 'ollama/qwen3.6:35b-mlx' \
  --harness opencode --harness-version 1.18.32 \
  --ahu-revision "$(git rev-parse --short HEAD)" \
  --skill-digest '<digest-of-tested-skill-bundle>' \
  --trace-id '<matching-otel-trace-id>' \
  --elapsed-ms '<elapsed-ms-from-otel>' \
  --output "$EVAL_HOME/runs.jsonl"
```

When evaluating manually, use matching MCP spans to populate `--decision-calls`
with the number of `ahu_typed_decide` calls. A nonzero count is evidence that
the tool was invoked, not that its result was followed correctly; the case score
measures the output. The recorder refuses to write its run file inside the ahu
checkout.

Two readers summarize the accumulated records. `scripts/local_eval.py trend`
prints a Python summary of the same records:

```sh
python3 scripts/local_eval.py trend --records "$EVAL_HOME/runs.jsonl"
```

`ahu eval report` is the native reader. It groups by corpus version and
surfaces MCP/tool coverage alongside scores:

```sh
ahu eval report --records "$EVAL_HOME/runs.jsonl"
ahu eval report --records "$EVAL_HOME/runs.jsonl" --output json
```

Without `--output json` the readable report owns stdout. With it, the versioned
JSON comparison goes to stdout and the readable report to stderr, so a pipeline
can consume one while a human reads the other.

### What `ahu eval report` does and does not do

It is a reader. It never launches a candidate or an evaluator, never queries a
collector, and never writes anything — not the record file, not the checkout.

`--records` is required and must resolve *outside* the repository. The path is
canonicalised before the comparison, so a relative path and a symlink that
points back into the checkout are both refused, as are the primary checkout and
every task worktree beneath it. Run evidence stays in a user-owned directory;
summarizing a record file must not be a way to make it repository content.

Records are external input, so every field is validated. A malformed line fails
as a usage error naming the file and the line number it is on — blank lines are
skipped but still counted, so the number is the one in your editor. A record
missing a field the report groups or scores by is refused by name rather than
guessed at. An unknown field is ignored, so a newer recorder stays readable
here. A record declaring a `schema_version` this ahu does not read is refused
rather than reinterpreted under the old field meanings. Record content is
escaped before it reaches the terminal.

### Grouping

One row per comparable configuration. Records are grouped by case ID and corpus
version, stage, candidate agent and version, evaluator agent and version,
candidate/evaluator model and harness identities, ahu revision, and skill digest. Any difference in
that identity is a separate row, so a skill-bundle or harness-version change
never averages into the baseline it should be compared against. A field the
recorder left unset, null, or empty reads as `unspecified` (stage defaults to
`candidate`), and those records group together.

### Coverage versus means

Score and pass rate are taken over every run in the group. Token, timing, and
decision-call figures are not: each is reported as a coverage count
(`timing 1/2`) alongside a mean taken only over the runs that carried the
measurement. A configuration that reported nothing is visibly uncovered rather
than silently cheap and fast.

A reported zero is an observation; an absent field is not. Two runs where one
reports `elapsed_ms: 0` and the other reports no timing at all give
`timing 1/2` with a mean of `0`, never `timing 2/2`. In the terminal a metric
nothing reported prints as `none observed`; in JSON it is `null`, never `0`.

### JSON contract

`--output json` emits a single object with `schema_version` 1, `command`,
`record_count`, and `groups`. Each group carries the full identity above,
`runs`, `passed`, `mean_score`, `pass_rate`, a `coverage` object for token,
timing, decision-call, and MCP observations, and an `observed` object with
timing, decision, MCP session/tool/error means, per-tool call means, and token
fields. Per-tool means include observed zero calls when MCP telemetry exists.

The contract deliberately carries **no local path**: the caller already supplied
the records location, and the record content itself is free of paths and
transcripts. Candidate, model, harness, and skill identifiers can still reveal
project details, so review the report before sharing it outside the intended
audience.

## Agent evaluators and local OTel

`ahu eval run` runs a versioned case through a registered candidate agent and,
when `--evaluator` is supplied, a separate registered evaluator agent. The
candidate sees the task and output schema, but not the expected answer or rubric.
The evaluator sees the rubric and candidate JSON but not candidate identity,
model, harness, trace, or deterministic reference answer. It must return a
versioned `score.json`; deterministic checks are recorded alongside its scores.
Use `--runs` to repeat a case. The command appends one record per completed
candidate run to the JSONL file passed with `--records`.

During each candidate task, ahu directs OTLP spans to a temporary loopback
receiver and aggregates task timing, MCP session counters, and calls/errors for
the known ahu tools. It stores the compact metrics with the score record and
discards received spans; it does not persist traces or send them to a remote
collector. Local token metrics are included when the selected harness exposes
them. Missing telemetry remains uncovered rather than being reported as zero.
`ahu eval report` compares candidate and evaluator identities and summarizes
MCP coverage and tool use as well as scores and timing.

Run artifacts, including prompts and harness diagnostics, are written outside
the checkout beside the records file. Treat that directory as private. The first
iteration runs one case per command, requires candidate output in `answer.json`
and evaluator output in `score.json`, and does not calculate confidence
intervals or repeat evaluator judgments. A local model choice depends on the
registered agent and harness configuration; the command cannot guarantee that
an agent alias routes only to local inference.

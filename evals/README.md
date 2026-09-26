# Local agent evaluations

This is a small, repeatable starting point for comparing ahu-launched local
agents. The checked-in cases are synthetic. Run evidence and OpenTelemetry
traces belong in a user-owned directory outside this checkout; never commit
prompts, transcripts, task results, or trace exports.

The suite has two moving parts today: manual headless launches that produce
result artifacts, and two readers over the JSONL record file those launches
feed. `scripts/local_eval.py record` scores one run and appends it;
`ahu eval report` and `scripts/local_eval.py trend` summarize the accumulated
records. Nothing here launches an agent for you.

## What is measured

Each run records the case and corpus version, model identifier, harness and
ahu versions, skill bundle revision, task outcome, a deterministic case score,
and reported token and timing metrics when available. Keep the same case set
and scoring rules when comparing model or skill changes. Compare repeated runs
as well as averages: model output can vary between attempts.

ahu's headless and MCP spans are the observability stream. Enable the project's
`[telemetry]` exporter and point it at a local OTLP/HTTP collector on
`127.0.0.1:4318`; ahu exports lifecycle, normalized token usage, and observed
skill evidence without prompts or transcripts. The MCP server exports spans
for parsed protocol requests, tool listings, and each tool call. Tool-list
spans show which ahu tools a harness was offered; `ahu.mcp.tool.call` spans
show which tools it used. Typed-decision spans include question shape,
outcome, backend/model, reported token counts, and service timing. Use resource
attributes `ahu.harness`, `ahu.model`, `ahu.task.id`, and `ahu.task.attempt` to
compare harnesses and join calls to result envelopes. The collector and its
storage must be outside the repository. `local_metrics = true` adds the same
reported token projection to the result envelope without an exporter. Missing
usage remains missing; do not estimate it.

OpenTelemetry standardizes trace *export*, not trace *query*: there is no
vendor-neutral query API, so reading spans back means whatever your collector's
backend offers. `ahu eval report` therefore reads no traces at all. It
summarizes only the fields already present in the JSONL records, which is why
span-derived figures such as elapsed time and decision-call counts reach the
report through recorder flags rather than through a collector query.

## Running a case

Use a disposable Git repository for every run. This keeps ahu's task state and
worktrees outside the source checkout. Configure the temporary project with:

- an OpenCode agent manifest pinned to the local Ollama model;
- the same skill bundle that the tested agents should receive;
- an OpenCode local MCP server entry that launches `ahu mcp serve` and sets
  `AHU_DECISION_URL=http://127.0.0.1:8001/v1/decisions`;
- `[telemetry] enabled = true` and a local collector endpoint.

Start Ollama and `python3 examples/ollama_decision_service.py --model <installed-local-model>`
outside the checkout, then launch the temporary agent with `ahu @<agent>
--headless --output json`. OpenCode `auto` permissions allow file and shell
tools without a sandbox, so use only disposable synthetic repositories and
review the launch preview before running an agent that edits files. Keep each
agent run fully local: the Ollama model name must have local weights (`ollama
list` shows a nonzero size), and the Ollama provider endpoint must be localhost.

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

After a run, save the model's structured answer and `ahu result <task-id>
--output json` into your external run directory. The synthetic answer artifact
shape is `{"department":"billing","refund_requested":0.96}`. Then append its
score to an external JSONL file:

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

Use matching MCP spans to populate `--decision-calls` with the number of
`ahu_typed_decide` calls. A nonzero count is evidence that the tool was invoked,
not that its result was followed correctly; the case score measures the output.
The recorder refuses to write its run file inside the ahu checkout.

Two readers summarize the accumulated records. `scripts/local_eval.py trend`
prints a Python summary of the same records:

```sh
python3 scripts/local_eval.py trend --records "$EVAL_HOME/runs.jsonl"
```

`ahu eval report` is the native reader. It rounds the same way, so the two
agree on a score, and it additionally splits rows by corpus version:

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
version, stage, agent and agent version, evaluator and evaluator version, model,
harness and harness version, ahu revision, and skill digest. Any difference in
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
`runs`, `passed`, `mean_score`, `pass_rate`, a `coverage` object
(`token_observations`, `timing_observations`, `decision_call_observations`), and
an `observed` object (`mean_elapsed_ms`, `mean_decision_calls`, `token_fields`).

The contract deliberately carries **no local path**: the caller already supplied
the records location, and the record content itself is free of paths and
transcripts. Candidate, model, harness, and skill identifiers can still reveal
project details, so review the report before sharing it outside the intended
audience.

## Agent evaluators and the ahu CLI

The remaining direction is for the CLI to orchestrate the evaluation rather than
ask users to join files by hand. A candidate stage would run each registered ahu
agent against the same versioned case in an isolated headless task. A separate
registered evaluator agent would receive the candidate artifact and rubric with
candidate identity removed, then return a strict, versioned score object with
criterion scores and short reason codes. The evaluator must not receive the
candidate's model, harness, agent name, or telemetry summary. Repeated judging
and deterministic checks against known-answer cases can detect judge variance
and scoring drift.

The run record would join the candidate and evaluator task IDs to their ahu
result envelopes and MCP spans. Run, case, corpus version, and stage IDs would
be passed as `ahu.eval.*` resource attributes, so the CLI could present tool
discovery, calls, outcomes, typed-decision usage, token observations, elapsed
time, and evaluator scores together. Traces would remain in the user's
configured OTel collector; compact score and coverage records remain outside the
checkout. Missing telemetry is reported as missing coverage rather than silently
treated as zero — the rule `ahu eval report` already follows.

**None of that orchestration exists yet.** `ahu eval run` is not implemented; it
fails with an explicit refusal rather than an unknown-subcommand error. There is
no automatic candidate or evaluator stage, no run ID the CLI assigns, and no
joining of telemetry to records. Running a case still means launching each agent
manually with `ahu @<agent> --headless --output json`, producing the result
artifacts yourself, reading any span-derived figures from your own collector,
and recording each run with `scripts/local_eval.py record`. `ahu eval report`
covers only the last step: summarizing the records that already exist.

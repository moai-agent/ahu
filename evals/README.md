# Local agent evaluations

This is a small, repeatable starting point for comparing ahu-launched local
agents. The checked-in cases are synthetic. Run evidence and OpenTelemetry
traces belong in a user-owned directory outside this checkout; never commit
prompts, transcripts, task results, or trace exports.

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

The first iteration deliberately uses ahu's existing OTEL and headless result
interfaces. The scorer measures answer quality; traces show which tools were
called and whether they succeeded. The MCP contract and case format stay
harness- and model-neutral.

## Agent evaluators and the ahu CLI

The next suite iteration should make the CLI orchestrate the evaluation rather
than ask users to manually join files. A candidate stage runs each registered
ahu agent against the same versioned case in an isolated headless task. A
separate registered evaluator agent receives the candidate artifact and rubric
with candidate identity removed, then returns a strict, versioned score object
with criterion scores and short reason codes. The evaluator must not receive
the candidate's model, harness, agent name, or telemetry summary. Repeated
judging and deterministic checks against known-answer cases can detect judge
variance and scoring drift.

The run record should join the candidate and evaluator task IDs to their ahu
result envelopes and MCP spans. Run, case, corpus version, and stage IDs are
passed as `ahu.eval.*` resource attributes, so the CLI can present tool
discovery, calls, outcomes, typed-decision usage, token observations, elapsed
time, and evaluator scores together. Traces remain in the user's configured
OTel collector; compact score and coverage records remain outside the checkout.
The CLI should report missing telemetry as missing coverage rather than
silently treating it as zero. A first usable command shape is:

```text
ahu eval run <suite> --candidate @agent-a --candidate @agent-b --evaluator @judge
ahu eval report <run-id> --output json
```

The current prototype does not implement these commands yet. It still requires
manual headless launches, collector queries, and `scripts/local_eval.py`. The
CLI runner should replace that glue while keeping ahu agents, OTel, and external
run storage as the underlying interfaces.

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
python3 scripts/local_eval.py trend --records "$EVAL_HOME/runs.jsonl"
```

Use matching MCP spans to populate `--decision-calls` with the number of
`ahu_typed_decide` calls. A nonzero count is evidence that the tool was invoked,
not that its result was followed correctly; the case score measures the output.
The recorder refuses to write its run file inside the ahu checkout.

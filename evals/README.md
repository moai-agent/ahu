# Local agent evaluations

A small, repeatable way to compare ahu-launched local agents on synthetic cases.
Every checked-in case is invented. Run evidence — prompts, answers, judge
artifacts, task results, and OpenTelemetry traces — belongs in a user-owned
directory outside this checkout and is never committed.

`ahu eval run` orchestrates repeated candidate runs and, optionally, a separate
ahu evaluator agent. It validates answer and score artifacts, writes compact
JSONL records, and captures ahu OpenTelemetry spans through a temporary local
receiver. `ahu eval report` compares those records.

## What is measured

Two independent things:

- **Answer outcome** — the candidate's `answer.json` against the case's
  deterministic `expected` values and weights, plus the evaluator's rubric
  scores when an evaluator ran.
- **Tool-behaviour outcome** — the tools the candidate actually called against
  the case's `tool_expectations`.

Neither outcome gates the other. A candidate can answer correctly while
reaching for a tool the case forbids, or abstain correctly while getting the
answer wrong, and the report shows both. Weighing them against each other is a
judgement for the reader, not something the scorer folds into one number.

Each record also carries the case and corpus identity, the candidate and
evaluator agent/model/harness identities, ahu version and build digest, target
repository head, the skill bundle digest, task outcome, and observed token and
timing metrics. When the decision response reports usage, the record keeps
`decision_service.input` and `decision_service.output` token observations
separate from the local agent's token counters. It also reports the sum of
successful decision calls' `decision_service_duration_ms` separately from the
agent's end-to-end elapsed time. It never adds unlike counters into a synthetic
total.

### Tool status can be unknown

Tool-behaviour scoring depends on MCP session telemetry. If the harness never
starts the configured `ahu mcp serve` entry, or the MCP process exits without
exporting its session summary, the tool status for that run is **unknown** — not
zero calls and not a clean abstention. Missing observations stay missing; they
are never turned into zeroes, and an unknown tool status is not a pass on a
`forbidden` expectation. A timeout or cancellation force-stops the attempt's
process group, so an MCP subprocess may not flush its final summary; those runs
retain partial or absent telemetry and an unknown tool status.

An ahu server entry and a successful tool call are separate checks. `ahu setup`
validates ahu's own stdio handshake; it cannot confirm that a harness loaded the
project entry, trusted the workspace, or allowed a tool call. Before a tool-use
comparison, inspect the selected harness's MCP status and make one real call
from the candidate workspace. For headless trials, configure any required
per-tool approval only in the disposable evaluation project and only for the
tool the case needs. In particular, do not make `ahu_typed_decide` a default
auto-approved tool in a normal project: it sends supplied evidence to the
configured decision provider. A missing server call can also mean the harness
denied the call before MCP dispatch; inspect native approval outcomes before
concluding the candidate chose not to use the tool.

### Telemetry, without a collector

ahu's headless and MCP spans are the observability stream. `ahu eval run`
temporarily routes candidate and evaluator OTLP/HTTP exports to a receiver bound
to loopback on an ephemeral port. It needs no collector binary, backend, or
query API, and does not change the project's saved telemetry settings. The
receiver holds only bounded ahu identifiers, trace IDs, durations, and MCP
counters in memory; raw spans and payload text are discarded.

ahu spans report lifecycle, token usage, skills, tool listings and calls,
failures, and typed-decision outcomes. MCP spans are joined to task attempts
through `ahu.task.id` and `ahu.task.attempt`; eval case, corpus, run, and stage
IDs ride along as resource attributes. The runner also enables ahu's local token
projection for its result envelope, again without touching repository
configuration; those are the harness's observed counters, not provider billing.
Each run record also carries receiver-side counters for spans rejected at the
task cap, malformed or oversized requests, connections rejected at capacity,
and listener errors. The report keeps those counters separate from agent tool
counts and shows how many runs supplied them; older or manually recorded rows
without receiver counters remain unobserved.

OTLP has no vendor-neutral query API, so the runner receives the exports
directly for the duration of each eval command and projects the relevant fields
into the record. `ahu eval report` reads those compact records only; it never
queries or retains traces.

## Case files

A case is OKF Markdown. The YAML front matter declares `okf_version: "0.2"`,
`type: ahu:eval-case`, and `schema_version: 2`, then the case identity and
scoring:

| Field | Meaning |
| --- | --- |
| `id` | Stable case identifier. |
| `corpus_version` | Version of the authored content and scoring rules. |
| `state` | Scenario data shown to the candidate. |
| `questions` | Typed questions: `choice`, `score`, or `probability`. |
| `expected` | Deterministic reference answer, one key per question. |
| `scoring` | Per-question weight plus `exact_match_pass_threshold`. |
| `rubric` | One criterion per scored field; required by `--evaluator`. |
| `tool_expectations` | Optional `required`, `required_successful`, and `forbidden` tool name lists. |

`tool_expectations` names only tools ahu exposes over MCP: `ahu_agents_list`,
`ahu_tasks_list`, `ahu_task_get`, and `ahu_typed_decide`. Every list is
optional; a case with none makes no claim about tool use.

The three lists ask three different questions:

- `required` — the tool was *attempted* at least once. A call that returned an
  error satisfies it, because the expectation is about tool selection.
- `required_successful` — the tool was called at least once *without* an error.
  A run that only ever got errors out of a tool selected it but delegated
  nothing, so it fails this and passes `required`. Use this when the case is
  about work actually being delegated rather than about which tool was chosen.
- `forbidden` — the tool was not called at all.

`required_successful` needs the session summary to attribute every error to a
named tool, not just every call. Without that the status is **unknown**: an
error count nothing can attribute would let an error-only session read as
successful delegation.

The Markdown body is shown to the candidate, so it states the request and its
data and nothing else. `expected`, `rubric`, and `tool_expectations` stay in
front matter and are withheld. A body must not reveal the expected answer, and
must not hint at which tools the case wants or refuses — a body that says "use
the typed-decision tool here" measures instruction-following, not tool
selection. Keep every tool label in front matter and in the grader.

The authored cases pull against each other on the same tool:

- `cases/decision-routing.md` asks two judgement questions about a support
  ticket, neither of them answerable by copying text, and **requires**
  `ahu_typed_decide`.
- `cases/direct-extraction.md` asks two questions a single logistics record
  already answers in its own words, and **forbids** `ahu_typed_decide`.
- `cases/priority-triage.md` asks the agent to apply a short impact policy to a
  support ticket and **requires** `ahu_typed_decide`.

Neither body mentions a tool. A candidate has to read the shape of the task.

The candidate prompt does not tell every candidate to always use the
typed-decision tool. Choosing it is part of what the suite measures, so a case
that wants it says so in `tool_expectations` and an agent that reaches for it
everywhere loses the abstention case.

Add cases that probe separate skills and decision types rather than growing one
large benchmark prompt. Keep them synthetic, balanced, explicit, and
answerable from the state alone. Version any change to authored content or
scoring rules: bump `corpus_version` instead of silently moving the baseline.

## Measuring typed-decision efficiency

Compare two agents on the same local harness, model, permissions, MCP setup,
and suite: a baseline without the `typed-decisions` skill and a treatment with
that skill installed and available to the harness. Keep every other input
identical. `ahu setup` installs the bundled skill for detected harnesses; its
presence does not prove that a harness loaded it. Inspect the harness's actual
skill behavior and compare the recorded agent context fingerprints before
interpreting a run.

Run repeated trials and compare answer pass rate, required and forbidden tool
outcomes, local agent token fields, agent elapsed time,
`decision_service.input` and `decision_service.output`, and
`decision_service_duration_ms`. Decision service usage stays separate because
TypeSafe's token counts and local harness counters may come from different
models and are not interchangeable. End-to-end elapsed time includes the tool
round trip; service duration shows the portion spent in the decision service.

Use at least 10 runs per variant as an initial look, then increase repetitions
if results are close or variable. Alternate which variant runs first across
separate invocations to reduce warm-cache and time-order effects. Keep records
outside the repository and inspect per-case results, intervals, and telemetry
coverage. Treat a lower count or shorter time as an observed association until
repeated trials and human review support its interpretation. This synthetic
suite is a starting point, not proof of a general speedup.

The candidate agent may run locally while TypeSafe Jev evaluates each decision
over HTTPS. That is not a fully local inference path: decision state and
questions leave the machine, and TypeSafe-reported usage may incur separate
provider costs. Use only synthetic data for these cases. To evaluate a fully
local decision path, set `AHU_DECISION_URL` to the loopback Ollama adapter and
keep that provider choice constant across variants.

## Suite files

A suite collects cases and their weights. It is OKF Markdown with front matter
declaring `okf_version: "0.2"`, `type: ahu:eval-suite`, `schema_version: 1`, an
`id`, a `version`, and a `cases` list whose entries are
`{path: <relative markdown case path>, weight: <positive number>}`. Paths
resolve relative to the suite file. They may move up one directory, but cannot
escape that parent or traverse a symlink.

`suites/agent-tool-selection.md` collects the three authored cases at equal
weight. Two require the typed-decision tool and one forbids it, so an agent that
always calls it fails one case of three on tool behaviour and an agent that never
calls it fails two of three. That balance is about tool selection only: a
tool-expectation outcome is never folded into an answer score. The report keeps one row per case and records
each weight; it does not currently calculate a weighted suite-wide score.

## Running

Use a disposable Git repository for every run. That keeps ahu's task state,
worktrees, prompts, and harness-created artifacts out of the source checkout.
Configure the temporary project with:

- an agent manifest pinned to the local model under test;
- the same skill bundle the tested agents should receive;
- a local MCP server entry that launches `ahu mcp serve` and sets
  `AHU_DECISION_URL=http://127.0.0.1:8001/v1/decisions`;
- the candidate agents and, optionally, an evaluator agent.

Start Ollama and the decision service outside the checkout:

```sh
python3 examples/ollama_decision_service.py --model <installed-local-model>
```

Compare two registered agent variants over the whole suite by repeating
`--agent`. Each agent runs every case `--runs` times:

```sh
ahu --repo "$EVAL_WORKSPACE" eval run \
  --suite "$AHU_CHECKOUT/evals/suites/agent-tool-selection.md" \
  --agent @triage-baseline --agent @triage-candidate \
  --evaluator @judge --runs 5 \
  --records "$EVAL_HOME/runs.jsonl"
```

`--case` still runs a single case:

```sh
ahu --repo "$EVAL_WORKSPACE" eval run \
  --case "$AHU_CHECKOUT/evals/cases/direct-extraction.md" \
  --agent @triage-candidate --runs 5 \
  --records "$EVAL_HOME/runs.jsonl"
```

Omit `--evaluator` to score only against the case's deterministic expected
answer. With an evaluator, the candidate receives the case state and typed
questions, its `answer.json` is checked against the expected values, and the
evaluator scores the rubric from a prompt that withholds candidate identity,
model, harness, trace, and expected answer. The score artifact must match the
versioned schema.

Execution is sequential: one case, one agent, one trial at a time, each in a
clean ahu task. There is no concurrency and no automatic retry — a failed trial
is recorded as a failure rather than quietly run again. Repeated runs are
independent records, which is what makes a pass rate meaningful.

`ahu eval run` never widens manifest approvals implicitly. Pass
`--allow-widened-approvals` only when a manifest explicitly requires that
permission and you intend to grant it. Keep each run local: an Ollama model
name must have local weights and its provider endpoint must be localhost. ahu
pins the manifest's harness and model, but cannot prove that a provider alias
routes only to local inference.

## Reading reports

```sh
ahu eval report --records "$EVAL_HOME/runs.jsonl"
ahu eval report --records "$EVAL_HOME/runs.jsonl" --output json
```

Without `--output json` the readable report owns stdout. With it, the versioned
JSON comparison goes to stdout and the readable report to stderr, so a pipeline
and a human can each read one.

A report distinguishes two kinds of identity. The **case fingerprint** covers
the case and corpus version, so a case edit never averages into the baseline it
should be compared against. The **input fingerprint** covers what each agent
was given and run under — candidate and evaluator identities, models, harnesses
and versions, ahu build digest, target and evaluator repository heads, candidate
and evaluator skill digests, stage, and tool definitions. Any difference in
either is a separate row.

Each row reports the answer outcome and the tool-behaviour outcome separately,
retains the judge's per-criterion scores and its reason codes, and gives a 95%
Wilson score interval for binary pass rates.

The answer outcome is two figures, not one. **Reliability** is passes over every
attempt in the row, failed launches and timeouts included, because a
configuration that cannot produce an answer is not a reliable one. **Quality**
is passes over the attempts that produced a valid answer, as `scored/total`,
with its own Wilson interval — and no interval at all when nothing was answered,
because no answer is not a quality of zero. A failed attempt is never reported
as a wrong answer. The readable report also prints a `terminal` line counting
how each attempt ended by name, and a `failed attempts` count beside the
coverage figures. Reason codes are short identifiers,
which is what makes them safe to keep in a record that holds no prose.

The terminal report starts with a comparison table containing case, agent,
runtime, answer pass count, tool pass count, mean time, and mean token amount.
It fits the available terminal width without wrapping rows; columns are removed
when they do not fit. A dash means no run reported that measurement. The detail
blocks include Wilson intervals and one mean per reported token field, with the
number of runs that supplied that field. Token totals are shown only when the
recorder supplied a total; input and output amounts are never added to invent
one. The JSON report remains schema version 2 and adds optional token summary
fields under `observed`.

### What the interval does and does not tell you

A Wilson interval quantifies how much sampling uncertainty is left in an
*observed* pass rate at the number of runs you actually did. Five runs at 4/5
leave a wide interval; the interval narrows as runs accumulate.

It is not evidence that one configuration caused an improvement in another.
Non-overlapping intervals on two rows are a reason to look closer, not a result.
Sequential local runs share machine state, model loading, and time of day; a
case corpus of three is a small sample of behaviour; and an LLM judge has its
own bias and variance. Spot-check individual answers and judge scores by hand, and
calibrate the judge against answers you have scored yourself, before reporting
that a change helped.

### Coverage versus means

Pass rate is taken over every run in a row, including failed or unscored
trials. Mean score uses only trials that produced a score and reports its own
observation count. Token, timing,
decision-call, and MCP figures are not: each is reported as a coverage count
(`timing 1/2`) alongside a mean over only the runs that carried the
measurement, so a configuration that reported nothing is visibly uncovered
rather than silently cheap and fast.

Latency and token means are conditional on a run having reported the
measurement, so they are printed with the sample they were taken over and with
the number of attempts that produced no answer at all (`elapsed ms 4000 (1/2
runs, 1 unanswered)`). A mean over the attempts that survived is not the cost of
the matrix, and the counts beside it are what stop it being read as one. The
JSON contract carries the same figures as `coverage.timing_observations`,
`coverage.launch_timing_observations`, and `attempts_without_answer`.

`launch_elapsed_ms` is the runner's own wall-clock measurement of a launch,
separate from the harness `elapsed_ms` that arrives with the spans. It is
recorded for every terminal attempt, including one that failed or timed out
before exporting anything, so a failed attempt never looks free.

A reported zero is an observation; an absent field is not. Two runs where one
reports `elapsed_ms: 0` and the other reports no timing give `timing 1/2` with a
mean of `0`, never `timing 2/2`. In the terminal a metric nothing reported
prints as `none observed`; in JSON it is `null`, never `0`.

### `ahu eval report` is only a reader

It never launches a candidate or an evaluator, never queries a collector, and
never writes anything — not the record file, not the checkout.

`--records` is required and must resolve *outside* the repository. The path is
canonicalised before the comparison, so a relative path and a symlink pointing
back into the checkout are both refused, as are the primary checkout and every
task worktree beneath it. Summarizing a record file must not be a way to make it
repository content.

Records are external input, so every field is validated. A malformed line fails
as a usage error naming the file and the line number as your editor counts it —
blank lines are skipped but still counted. A record missing a field the report
groups or scores by is refused by name rather than guessed at. An unknown field
is ignored, so a newer recorder stays readable. A record declaring a
`schema_version` this ahu does not read is refused rather than reinterpreted
under the old field meanings. Record content is escaped before it reaches the
terminal.

The JSON contract deliberately carries **no local path**: the caller already
supplied the records location, and record content is free of paths and
transcripts. Agent, model, harness, and skill identifiers can still reveal
project details, so review a report before sharing it.

## Blinding is prompt-only

The evaluator's prompt omits candidate identity, model, harness, tool trace, and
the deterministic expected answer, and presents the candidate JSON as untrusted
data rather than instructions. That is the whole of the blinding.

It is not environment isolation. When the evaluator runs against the same
repository as the candidate, it can still read the agent registry under
`.agents/`, Git history, task state, and any other repository-visible material,
and could infer which configuration produced an answer. Treat the judge as
prompt-blinded, never as blind. If a comparison turns on the judge's
impartiality, run the evaluator against a checkout that carries the cases and
nothing identifying the candidates, and say in your write-up which of the two
you did.

## Longitudinal record

Store one compact JSON object per run in an external JSONL file. Keep it
numeric and non-sensitive: case and corpus identity, model (including
quantization or tag), harness and version, ahu version and build digest, target
repository head, skill digest, task ID and attempt, outcome, scores, tool-expectation outcome,
observed token fields, elapsed time, and the matching OTel trace ID — and no
prompts, subjects, answers, paths, issue references, or transcripts.

`ahu eval run` writes each accepted answer, task result, optional judge score,
and prompt into a private run folder (`0700` directory, `0600` files on Unix)
next to the external JSONL file. Keep that folder and the records outside the
checkout and treat it as private.

Report mean score, pass rate, and coverage separately, so a missing observation
never reads as a zero.

A candidate that launches but omits an answer, writes malformed JSON, or
produces an answer outside the declared question type is recorded as a failed,
unscored trial, and so is one that never launched or ran out of time. Such a row
carries `answer_status: "no_answer"`, which is what keeps it out of the
answer-quality rate while still counting against all-attempt reliability. The
remaining trials in the matrix still run. A scored mean therefore does not
disguise these failures; inspect `score_observations`, `attempts_without_answer`,
and the terminal-status counts.

A failed or timed-out attempt keeps whatever it did report: its launch wall-clock
time, its task ID and attempt, the harness outcome that ended it, any token
metrics the harness measured, and any telemetry that reached the receiver before
the attempt was stopped. None of that is invented — an observation the attempt
never made stays absent rather than becoming a zero.

Cases and JSONL run records use schema version 2. Earlier experimental schema
versions were never released and are not accepted by the runner or report.

For one-off manual scoring, `scripts/local_eval.py record` reads case front
matter with PyYAML — `python3 -m pip install -r requirements-evals.txt` — and
takes a structured answer plus an `ahu result --output json` envelope:

```sh
python3 scripts/local_eval.py record \
  --case "$AHU_CHECKOUT/evals/cases/decision-routing.md" \
  --answer "$EVAL_HOME/current/answer.json" \
  --result "$EVAL_HOME/current/result.json" \
  --agent '@triage' --agent-version 1.0.0 \
  --run-id "$EVAL_RUN_ID" --stage candidate \
  --model 'ollama/example-model:8b' \
  --harness opencode --harness-version 1.18.32 \
  --skill-digest '<digest-of-tested-skill-bundle>' \
  --trace-id '<matching-otel-trace-id>' \
  --elapsed-ms '<elapsed-ms-from-otel>' \
  --output "$EVAL_HOME/runs.jsonl"

python3 scripts/local_eval.py trend --records "$EVAL_HOME/runs.jsonl"
```

It writes schema 2 records with a partial fingerprint and the
`manual_unverified` prompt profile. If the case has tool expectations, their
status remains unknown because the helper does not evaluate MCP spans. The
helper's optional `--decision-calls` value is a manually supplied count; it does
not mark telemetry as observed or change `telemetry_coverage`, which remains
`none`. The manual record stays in its own report group, separate from fully
orchestrated runs. Use `ahu eval run` for comparable agent evaluations. The
recorder refuses to write its run file inside the ahu checkout.

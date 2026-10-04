# Evaluate agents with ahu

An eval measures a registered agent on the cases you name. `ahu eval run` requires
a case or suite and an agent; it does not select a hidden benchmark. Cases are
OKF Markdown with YAML front matter. The candidate receives the task, state and
questions, then writes `answer.json`. Expected answers, scoring weights, tool
expectations and judge rubrics stay out of its prompt.

Keep cases outside the candidate checkout if the candidate should not read their
answers. Prompt omission is not filesystem isolation. Run artifacts and records
must stay outside repositories.

## Start a comparison

Run `ahu setup` in your candidate project, choose its harness models, and commit
the generated context and lock. Confirm readiness with `ahu doctor` and use
`ahu agents` to select a registered identity. Native harness authentication,
workspace trust and tool approval must be configured before unattended runs.

Create an authored case such as this one outside the candidate checkout:

```markdown
---
okf_version: "0.2"
type: ahu:eval-case
schema_version: 2
id: support-owner
corpus_version: 1.0.0
state:
  policy: Billing handles duplicate charges; technical handles reproducible crashes.
  report: Saving a valid project crashes the application every time.
questions:
  owner:
    type: choice
    instructions: Select the owner under the supplied policy.
    options:
      billing: Duplicate charges
      technical: Reproducible crashes
expected:
  owner: technical
scoring:
  owner: 1.0
  exact_match_pass_threshold: 1.0
---

Assign an owner to this synthetic support report.
```

Set the paths and agent names below for your project. Use the same model and
harness when comparing instruction variants; use different models when model
choice is the question you want to evaluate.

```sh
ahu --repo "$CANDIDATE_REPO" eval run \
  --case "$CASE_FILE" --agent @baseline --agent @candidate \
  --runs 3 --timeout 180 --allow-widened-approvals \
  --records "$EXPERIMENT_DIR/runs.jsonl"
ahu eval report --records "$EXPERIMENT_DIR/runs.jsonl"
ahu eval report --records "$EXPERIMENT_DIR/runs.jsonl" --output json
```

The approval flag explicitly admits agent manifests that widen harness approvals;
omit it when no such manifest is used. It does not bypass workspace trust or
grant every MCP tool permission. Live Jev requests send their evidence and
questions to TypeSafe AI and consume provider usage; use authorized, synthetic
or minimized data and the [documented credential handling](typed-decisions.md#secret-boundary).

Use `--suite PATH` instead of `--case PATH` for a collection of cases with fixed
weights. See the [batch comparison suite](../evals/batching/README.md) for three
explicit delegation policies and a control that should not call Jev. Use
separate ordered `--runs 1` blocks when counterbalancing agent order; repeated
runs within a matrix are grouped by agent.

## Read quality and process separately

| Evidence | What it answers |
| --- | --- |
| Deterministic answer score | Typed result correctness against the expected values |
| All-attempt answer pass rate | Passing answers as a share of every attempt, including launch failures |
| Quality among valid answers | Answer correctness among results that could be graded |
| OTel tool expectations | Required or forbidden MCP behaviors, with evidence coverage |
| Time and native token observations | Work and delay reported by the harness |
| Harness-reported USD | Observed amounts and coverage, separate from tokens and provider billing |
| Decision service usage | Additional provider tokens and successful-call duration |
| MCP argument bytes | Measured decision request sizes before provider expansion |

A tool call does not earn answer credit. Add `tool_expectations` only when tool
behavior itself is a requirement. `required` means a received call;
`required_successful` also needs evidence of success. Missing telemetry is
unknown, not zero. Server spans cannot reveal native approval denials that
prevented dispatch; inspect native outcomes too.

Evaluation runs capture OTel locally and correlate MCP calls with candidate attempts.
Decision spans record request format, counts, usage and timing without recording
item text or returned decisions. Request-byte sums carry measured-call and
measured-run counts, including failed calls. They are not token or billing
estimates. Native input may include cached input and native output may include
reasoning; preserve each harness's semantics and keep provider tokens separate.

## Improve instructions, skills and tools

Use the bundled `ahu-agent-context-critic` skill to turn one observed problem into a
testable change. Commit each variant with its refreshed context lock. Freeze
cases and expected answers before running the comparison, retain failures, and
compare matched cases with the same model, tools and permissions. Include tasks
where the proposed behavior should not apply. Reserve fresh cases for the next
validation round instead of repeatedly tuning against the same examples.

For subjective criteria, add a case rubric and `--evaluator @judge`. An evaluator
is another registered ahu agent. Its prompt omits candidate identity and trace;
use `--evaluator-repo PATH` for a separate prepared checkout. Judge scores are
reported separately from deterministic answer scores. A single judge is
uncalibrated: review sample judgments and disagreements against human decisions.

Report observed improvements with the cases, model, harness, repetition count,
coverage and regressions. Small pilots can demonstrate a working integration
and expose overhead; they do not establish universal accuracy or efficiency.

## Compare skill-selection policies

An explicit `--skill-selection` option adds advisory skill paths before the
candidate starts. Use the same agent, committed skill catalog and cases across
`none`, `lexical` and `decision` arms. The default `none` leaves the prompt
unchanged. Decision mode discloses candidate-visible task purpose/state and
skill names/descriptions to the configured provider; it never receives hidden
answers or grading rubrics.

```sh
AHU_DECISION_MODEL=jev-1.13.0 ahu --repo "$CANDIDATE_REPO" eval run \
  --case "$CASE_FILE" --agent @candidate --skill-selection decision \
  --records "$EXPERIMENT_DIR/decision.jsonl"
```

This does not remove skills, rewrite their bodies, or choose a different
harness/model. The first experiment measures selection and task quality.
Token reduction requires observed changes in what the agent reads or does.

Records include policy/catalog/provider identity, suggested paths, abstention
or fallback, selection duration and separate provider usage. The report groups
selection policies by configured provider/model and catalog, including provider
failures in the same arm. Returned model identities are reported separately with observation counts. A
requested-model fallback is explicitly marked and does not count as a returned
model observation.
Complete provider usage and partial known subtotals have distinct coverage counts.
Candidate calls to `ahu_skills_suggest` retain their separate provider costs even
when the candidate later fails. Time marked `total` includes selection preparation,
telemetry export and launch wall time; unmarked time in older records is harness
elapsed. It excludes grading and optional evaluator execution. Failed agent attempts retain
selection cost. Failed provider calls may have unknown billed usage.

Prelaunch OTel spans use `ahu.skills.selection` with bounded purpose, policy,
mode, counts, outcome and timing. Their synthetic observation ID is separate
from the candidate task; the record states whether the local receiver observed
the span. The loopback exporter disables proxies and redirects and does not
inherit OTLP authorization headers or unrelated resource attributes. This does not count as a candidate MCP call or prove skill loading.
MCP skill suggestions carry the same bounded selection metrics on their tool
span. Task text, skill descriptions and bodies are absent from these attributes.

Freeze labels, policies, model versions and the analysis plan before comparing.
Counterbalance arm order, retain failed attempts, and evaluate unnecessary
suggestions on no-skill cases. Calibrate on development cases and evaluate on
fresh held-out tasks. Report selection accuracy separately from completed-task
quality, native usage, service usage and latency.

## Ablate committed project skills across harnesses

`ahu eval run` compares agents and records the target commit and skill bundle
digest, but no supported harness has a common, safe switch for disabling
project skills at launch. Prepare a second committed context in a linked
worktree, then run the same case or suite against both checkouts. Because both
are worktrees of the same repository, they retain the same repository identity
and local auth binding. The generated lock is current in each arm, so the
comparison does not bypass the context-drift gate.

Prepare a control arm with all project skills removed, or remove one skill by
directory name:

```sh
python3 scripts/prepare_skill_ablation.py \
  --repo "$CANDIDATE_REPO" \
  --worktree "$SKILLS_OFF_REPO" \
  --branch eval/skills-off \
  --all-project-skills
```

For a one-skill ablation, replace `--all-project-skills` with
`--skill ahu-typed-decisions`. The helper requires a clean source checkout
without ignored harness context and a new destination outside it. It creates a
reviewable local branch/worktree, removes selected paths from `.agents/skills`,
`.claude/skills`, and `.opencode/skills`, refreshes `ahu.lock`, and commits only
those changes. It uses disposable ahu state while refreshing the lock, so
per-user context fingerprints are not accepted into the user's normal ahu state. The branch and
worktree remain for review and evaluation; remove them after the experiment
when their ahu eval tasks are no longer needed. Use `--ahu /path/to/ahu` to
select the intended executable instead of the one on `PATH`. A failed setup
retains any created branch/worktree for inspection.

Run both arms with the same candidate agent, harness, model, permissions, case
suite, repetitions, and evaluation options. Use one harness-specific agent per
pair, then repeat the pair for every supported harness. For example:

```sh
ahu --repo "$CANDIDATE_REPO" eval run \
  --suite "$SUITE_FILE" --agent @dev-codex --runs 5 \
  --allow-widened-approvals \
  --records "$EXPERIMENT_DIR/skills-on.jsonl"
ahu --repo "$SKILLS_OFF_REPO" eval run \
  --suite "$SUITE_FILE" --agent @dev-codex --runs 5 \
  --allow-widened-approvals \
  --records "$EXPERIMENT_DIR/skills-off.jsonl"
cat "$EXPERIMENT_DIR/skills-on.jsonl" "$EXPERIMENT_DIR/skills-off.jsonl" \
  > "$EXPERIMENT_DIR/skill-ablation.jsonl"
ahu eval report --records "$EXPERIMENT_DIR/skill-ablation.jsonl"
```

Use unique arm record files and an external experiment directory. The report
keeps the arms separate by target commit and skill digest and reports answer
quality, tool expectations, token observations, latency, and telemetry
coverage. Alternate which arm runs first across repetitions or invocations to
reduce order effects. Keep failed attempts and unknown telemetry in the result.

This is a **project-skill** ablation. It controls committed files under the
standard project skill roots used by supported harnesses. It cannot turn off
user-level skills, global plugins, harness built-ins, or skill sources outside
those roots, and it does not prove that a harness loaded any available skill.
Keep user and harness configuration fixed, inspect actual skill invocation
telemetry where available, and describe the result as the effect of committed
project skill context in that harness/model/case set. The first run is a
bounded experiment, not a universal skill score.

### Answer encoding

The tool-neutral prompt template (version 3) explicitly asks for choice option
keys and numeric score/probability values. Option descriptions explain the
choices; they are not accepted in place of keys. Prompt version and profile are
part of the input fingerprint, so older and newer prompt runs remain separate.

## Typed decision evaluator

`ahu eval run --decision-evaluator` grades case rubrics through the configured
`ahu_typed_decide` provider boundary. It is opt-in and conflicts with `--evaluator`
and `--evaluator-repo`; the candidate still runs normally. This option sends case
state, questions, candidate output, and rubric instructions to the configured
provider. It adds no expected answers, scoring weights, candidate identity, or
traces. Evidence supplied within case data or candidate output remains untrusted.
Choice keys are also resolved locally into selected option text, supplied as
`selected_answers`; numeric answers retain their value. This projection uses
only candidate-visible options and the actual answer, never reference labels.

Each rubric field becomes one score question with range 0..1 and descriptive
levels **Does not satisfy the criterion**, **Partially satisfies the criterion**,
and **Fully satisfies the criterion**. All cases are checked before any candidate
launch, including the 20-question limit, instruction bounds, and 64 KiB request
capacity reserved for the largest valid answer. Grading uses the same weighted
judgement validation as registered evaluator agents; reason codes are empty.
Scores may be continuous between the described levels. The case's scoring
threshold also controls the weighted judge pass, so a threshold of 1 requires
an exactly perfect aggregate score. Choose and validate thresholds before an
experiment; retain continuous scores when comparing graders. Prefer deterministic
checks whenever they express the required outcome. Typed judging is an alternative
to an optional rubric evaluator, not a prerequisite for running evals.
Each candidate gets one provider attempt, with no retry or substitute judge.
Failed judging has no headline score or pass; deterministic answer checks remain separate.

Provider and credential selection are unchanged from `ahu_typed_decide`:
`AHU_DECISION_URL` selects a validated local endpoint, otherwise the configured
TypeSafe provider is used (`AHU_DECISION_MODEL`, default `jev-latest`). Only that
existing boundary accesses credentials. Configuration and grading policy version
are fingerprinted before execution, separately from returned service identity.
The evaluator is identified as `typed_decision`, never as a registered agent.
Blinding is `typed_request`: ahu supplies only the bounded request, without an
agent checkout; this does not certify provider behavior.

Records include `evaluator_metrics` and `evaluation_elapsed_ms`. The latter starts
before skill selection and ends after grading, including failed work;
`total_elapsed_ms` retains its selection-plus-candidate-launch meaning.
Evaluator-agent tokens and projected telemetry use the ID and attempt of that
evaluator task, including failed launches and invalid score artifacts. Typed grading
records safe returned provider/model and optional provider tokens/duration.
A provider may omit service metadata: valid scores still count, with unknown
provider identity and usage. Malformed supplied metadata fails grading.
Failed service calls have unknown usage, never an invented zero. Returned usage
and input/output completeness are reported separately from candidate usage.

Reports expose evaluator status counts, timing means, per-metric observation
counts, reported provider identities, usage completeness, and telemetry coverage
in JSON and text. A missing observation does not contribute zero to a mean.
The comparison table prefixes candidates with `typed:` or `@judge-name:` so the
grading arm stays visible in narrow terminals. It marks complete evaluation time
as `eval`; `total` continues to mean preparation plus candidate completion.
The table's `TOKENS` column shows candidate native usage; grader and provider
usage remain separate in the detailed observations below it.
Typed grading exports a genuine `ahu.eval.typed_decision` span to the run's local
receiver under a separate evaluator observation ID. It carries only status,
latency, and token counts, with no evidence, credentials, inherited OTel resource
attributes, or authentication headers. Coverage `typed_decision_span` means this
observation arrived; it does not claim MCP session coverage. Candidate MCP tool
counts are unaffected. Agent telemetry retains `none`, `partial_spans`, or
`complete_session` coverage.

## Observed trajectories and budgets

New records also carry a compact `trajectory` projection from the candidate's
native stdout stream. Existing records without that field remain readable and
count as missing trajectory observations. The text report and JSON report show
means with per-measurement observation counts, source counts, coverage, tracking
limits, case budget outcomes and CLI guard outcomes. Missing values are never
filled with zero. The tool error rate mean uses only runs with complete observed
coverage and valid counts, gives each run equal weight, and carries its observation
count. `tool_error_rate_coverage` counts complete, partial and unknown runs
separately; partial rates are not included in that mean.

The measurements have deliberately narrow meanings:

- `steps`: completed Codex turns, unique Claude assistant message IDs, or unique
  OpenCode step-finish IDs. These are different native units, identified by
  `source`, and do not measure hidden reasoning steps.
- `tool_calls`: unique recognized call IDs, including calls still pending when
  capture ends. Codex command execution, file change, web search and MCP items;
  Claude tool-use blocks and their correlated tool-result blocks; and OpenCode
  tool-use parts are supported. Started/completed updates are counted once.
- `completed_tool_calls`: unique calls with an observed, classified terminal
  outcome. A start alone never enters this denominator. Older trajectory records
  without this field have an unknown tool error rate.
- `tool_error_rate`: observed errors divided by completed tool calls. Zero
  completed calls gives zero only with complete coverage and zero errors.
  Missing counters, errors exceeding completions, or completions exceeding calls are
  unknown; complete coverage also requires all observed calls to have completed.
  The per-record `trajectory_tool_error_rate` includes value, coverage, numerator
  and denominator. A partial rate describes observed outcomes only and is not a
  lower bound on the final rate.
- `tool_errors`: explicit native errors, nonzero command exit codes, and MCP
  `result.isError` failures among the observed calls. This is separate from the
  existing server-side OTel MCP counters; the two are not added together.
- `repeated_tool_calls`: subsequent invocations of the same tool, regardless of
  arguments. Repetition does not imply waste or an identical request.
- `repeated_tool_errors`: further observed errors from a tool that already failed.
- `tool_recoveries`: a later observed successful completion from a tool after its
  error. This measures the order of tool outcomes, not whether the task recovered
  or whether the later call retried the same arguments.

`complete_observed_stream` means the adapter observed the supported start and end
shapes, with no unclassified events, malformed input, unfinished calls or capture
loss. It describes that captured stream only, not provider internals, commands
within a shell or native helper work. Unclassified shapes and known losses make
coverage `partial`; their counts are lower bounds (rates are not). Antigravity
trajectory coverage currently remains `none`: the inspected step-update shapes
do not establish a tool lifecycle with reliable duplicate detection. Its existing usage metrics are
unaffected. Helper messages are excluded and make coverage partial.

The projection retains no reasoning text, prompts, tool arguments, tool results,
raw names or call IDs. In-memory SHA-256 identity keys are bounded to 4,096 calls,
4,096 steps and 256 tools, with a 1,024-byte input identity limit. Reaching a bound
sets `tracking_limited`; further observations that cannot be tracked cannot make a budget
pass under the default coverage policy. Counters saturate. Existing 1 MiB event
and 64 MiB stream limits still apply. No usefulness score is calculated.

Cases can add optional front matter:

```yaml
trajectory_budgets:
  max_steps: 8
  max_tool_errors: 1
  max_tool_error_rate: 0.25
  unknown_coverage: unknown
```

At least one budget is required. Rates must be finite fractions in `0..1`.
An observed count exceeding a budget fails even
with partial coverage. Otherwise complete observed coverage passes; missing or
partial coverage defaults to `unknown`. Set `unknown_coverage: fail` to fail
inconclusive observations, or explicitly use `observed_only` to judge available
counts and rates. Even `observed_only` leaves a missing or invalid required
measurement unknown. A partial rate exceeding its maximum remains unknown unless
`observed_only` explicitly opts into judging it.
The run output and report show `trajectory_budget_status` separately from answer
correctness and tool expectations. These are post-run checks: they neither stop
a running agent nor change its answer score or the command's exit status. Budget
configuration stays out of candidate and evaluator prompts.

### Trajectory command guardrails

Opt into post-run CI checks with either or both flags:

```sh
ahu eval run --case "$CASE_FILE" --agent @candidate \
  --records /tmp/ahu-evals/runs.jsonl \
  --max-trajectory-steps 8 --max-tool-error-rate 0.25 --output json
```

`--max-trajectory-steps` accepts an integer from 0 to 4294967295, inclusive;
`--max-tool-error-rate` accepts a finite fraction from 0 to 1, inclusive. Repeated
flags, missing values and malformed values are refused before launching trials.
The flags are maximums: for each measurement, the effective maximum is the
smaller of the CLI and case values. Other case constraints, including
`max_tool_errors`, remain part of the effective guard. For example, a case step
budget of 4 with CLI maximum 8 still enforces 4; CLI maximum 2 enforces 2.

CLI guards always require complete observed evidence, even if the case chooses
`observed_only`. Missing, partial, unsupported or legacy evidence without a
required counter yields `unknown`, never an assumed zero or passing guard.
An observed count exceeding its maximum can already establish `fail`; partial
rates remain unknown. The original case budget and its coverage policy are
still evaluated separately.

Every trial record persists the requested `trajectory_guardrails`, tightened
`trajectory_effective_limits` and `trajectory_guard_status` (`pass`, `fail` or
`unknown`). Guard status stays separate from `answer_passed`, `passed`, answer
scores, case budget status and tool expectations. All planned trials run and
append their records before the command emits its summary and returns exit 1
if any guard failed or remained unknown. Ordinary execution or storage errors
still use the existing error contract. Without either CLI flag, the existing
exit contract is unchanged, including for failing case budgets. These checks
do not interrupt the agent or impose a live execution limit. No composite
trajectory efficiency index is calculated.

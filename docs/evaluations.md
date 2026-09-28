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

Use the bundled `agent-context-critic` skill to turn one observed problem into a
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
AHU_DECISION_MODEL=jev-1.13.0 ahu --repo "$CANDIDATE_REPO" eval run   --case "$CASE_FILE" --agent @candidate --skill-selection decision   --records "$EXPERIMENT_DIR/decision.jsonl"
```

This does not remove skills, rewrite their bodies, or choose a different
harness/model. The first experiment measures selection and task quality.
Token reduction requires observed changes in what the agent reads or does.

Records include policy/catalog/provider identity, suggested paths, abstention
or fallback, selection duration and separate provider usage. The report groups
selection policies by configured backend/model and catalog, including provider
failures in the same arm. Returned model identities are reported separately.
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

# Synthetic skill selection corpus

Version 1.0.0. This is a small, independent policy-workbook corpus, not evidence
that any selector improves an agent. All records and rules are fictional.
The reference labels were derived from the policy bodies before any selector or
candidate results were inspected. Do not tune tasks, descriptions, labels,
thresholds, or expected answers against results from these cases.

## Contents and boundary

Copy ONLY fixture/ into a new candidate repository. Commit that fixture and the
identical experiment-owned registration/configuration needed by all arms.
Do not copy this README, cases/, suites/, reference.json, or validate.py into it.
Do not run candidates in the source checkout, or give them its Git history,
a source path, a symlink to this corpus, or the reference mapping in their prompt.
The stock eval runner does not perform this fixture-copy step.

The candidate receives the ordinary schema-2 case prompt: purpose, state, and
questions, and writes answer.json. Expected answers, labels, scoring, and the
suite remain outside that repository. This is prompt blinding and repository
separation, not OS isolation: reachable external files can still leak answers.
Record actual filesystem permissions and candidate access evidence; do not
describe the setup as a security boundary.

There are 12 skills, 12 equal-weight cases, and 26 choice-valued decisions:
six single-skill cases, three multi-skill cases (two, two, and three skills),
and three no-skill controls. Every skill contributes a necessary policy fact
to at least one case. No tools, accounts, services, or real repositories are
needed to solve the workbook; local file reading and answer writing suffice.
No skill body refers to this experiment or its reference labels.

reference.json is analyst-only. Required means a policy source needed for the
requested decisions, not that a particular tool invocation is mandatory.
Useful means relevant optional assistance without an indispensable policy fact.
Useful lists are empty in this version: the bounded requests have no separately
identified optional policy requirement. The rationale for every label is included.
Successful guessing or pre-existing knowledge does not establish a skill read.

## Frozen three-arm protocol

Use identical registered Codex agents with the same exact model, instructions,
native skill catalog, permissions, harness version, context, and limits.
Registration identity differences must not introduce behavioral instructions.
Use a fresh session and clean copy of the same committed fixture for each trial.
The control has NO prelaunch suggestion. Leave native skill discovery, reading,
and choice unchanged in all arms. Never remove catalog entries or force reads.

The selection input is exactly the parsed purpose and state, plus each committed
skill's name and FULL description. Neither questions, expected answers, labels,
bodies, file names of cases, nor answers are selection features.
Use the same source extraction and ordering in both suggested arms.
Sort catalog records by relative skill path; preserve description text without
summaries or truncation. Freeze and hash extraction/serialization code.

The lexical arm uses policy 1 in `src/skill_selection.rs`: Unicode lowercase
alphanumeric token sets, the fixed stopword list in that file, distinct token
intersection between task and skill name/description, a minimum score of two,
and at most three suggestions. Rank descending with relative-path ties.
Zero qualifying matches produce no advice. Freeze the source digest and stopword
list with the experiment; do not tune the baseline against this corpus.

Jev asks one independent probability question per skill using the exact request
builder in `src/skill_selection.rs`. It asks whether the skill directly applies,
rejects mere keyword overlap, and allows a necessary part of a multipart task.
Task and catalog entries are evidence rather than instructions. Calls are made
before launch, in sequential batches of at most 20 questions and 64 KiB, without
retries. Retain probabilities at or above 0.8, descending with path ties, at most
three, and no forced minimum. Freeze the serialized requests and model/service
configuration externally. Do not infer calibration from the probability type.

Both suggested arms use the exact same `Selection::prompt_block()` implementation:

```text
Advisory task relevance suggestion: consider the following skills. Existing mandatory and explicit instructions take precedence. The user may ignore these suggestions if they are not applicable. These suggestions are not proof that any skill was loaded.
- .agents/skills/NAME/SKILL.md
```

Omit the block for an empty set. Paths are rendered in sorted order. No scores,
reasons, or bodies are appended. The ordinary full catalog remains available.

Before measurement, freeze corpus commit/digests, source and executable revisions,
registered manifests, all fixture files, prompts, selector code/configuration,
harness version, model identifiers, timeout, scheduling, and reporting rules.
Mutable model/service aliases limit reproducibility even when their text is pinned.
Keep registration/hook/global-context differences visible rather than assuming
that matching checked-in files prove identical loaded context.

A bounded pilot consists of two repetitions over all 12 cases and three arms:
72 candidate trials total. Serialize trials and balance the six arm permutations
across cases in each repetition, rotating case order in the second repetition.
Record the exact schedule before running. This balances ordering across cases;
it is not a full six-permutation counterbalance within each individual case.
Use a fresh candidate session and worktree for each trial. Record workload,
cold/warm state, and service ordering.
Stop on infrastructure failure, retain all attempts, and report the incomplete
matrix. Do not turn a failed selector into a control trial or retry only failures
without declaring a new block. No automatic selector retry in the pilot.
A stopped-before-launch selector trial is an end-to-end failure with no candidate
answer, and must remain visible alongside candidate-only counts.

## Stage 1: suggestion quality

Let S be suggested paths, R required paths, and U useful paths.
On nonempty S report strict precision |S intersect R|/|S| and useful-inclusive
precision |S intersect (R union U)|/|S|. On nonempty R report required recall
|S intersect R|/|R|. Empty denominators are undefined, not perfect scores.
Also report exact required-set matches, missed required skills, unrelated
suggestions, suggestion count, and counts of undefined observations.
Show micro totals and macro averages over eligible cases with denominators.
Report single/multi/no-skill strata and per-case sets, not just one average.

For no-skill cases report any-suggestion error rate and the number of unnecessary
paths suggested. For the control, prelaunch selection metrics are not applicable:
it has no selector. Report its native discovery behavior separately.
Report all skill probabilities and lexical scores externally with their provenance;
never feed reference labels or these analysis results back into measured prompts.
Selector failures and missing records are failures/unknown observations, not empty
successful selections. This corpus's labels do not calibrate 0.8 as a probability.

## Stage 2: task outcomes and overhead

Use existing deterministic expected answers: equal question weights and an
all-questions-correct case threshold of 1.0. Report per-question accuracy and
case pass rate separately; equal suite weights avoid extra weight for multi-skill
cases with more questions. Report completed-answer quality separately from
reliability over all attempted trials. Keep malformed/missing answers, denials,
timeouts, selector failures, and launch failures visible. No agent evaluator
is needed, and tool selection itself earns no answer credit.

Measure a monotonic end-to-end interval beginning BEFORE selection/catalog
extraction and ending after answer collection/grading. Record selector elapsed
time (including service wait, failures, and any declared retries), launch time,
candidate time, grading time, and total wall time. Report paired differences
against the control within case/block with coverage counts. The runner
`total_elapsed_ms` covers selection through candidate completion, including local
export overhead, but excludes grading. Measure the full eval command separately
for the interval through grading. The candidate elapsed field alone excludes
prelaunch service overhead.
For concurrent subrequests use elapsed wall time, not their summed durations,
as selector latency; report request counts and service durations separately.

Report native input/output/cache/reasoning observations and service usage/cost
separately, including measurement coverage. Sum only commensurate billed units
with a documented rule; missing usage is unknown, not zero. Bytes are not tokens.
The runner records prelaunch selection separately from candidate MCP selection
calls. Report complete usage and known partial subtotals separately; a failed
request can have unknown cost. Neither kind is native agent usage.

Count a skill load only from an attributable native event or successful read
whose content includes that body's text. Record session/event/tool provenance,
path, committed digest, success, and whether content was truncated. Distinguish
catalog availability, a suggestion, an attempted read, returned body content,
and an unobservable load. Self-report or successful answers alone prove no load.
Unknown native coverage cannot establish that no skill was read. Body exposure
also does not prove the model used it. Keep raw execution evidence outside repos.

## Design review and limits

These are constrained recognition decisions rather than open-ended implementation;
multiple-choice options can cue an answer, and common workflow knowledge can
help with several cases. Nonstandard body-only conventions (batch alias reset,
two reconciliation runs, fresh rollback namespaces, redirect lifetime) provide
stronger reading dependence, but cannot rule out guessing.
The mixed cases test composition, not only identification. All names and
descriptions are truthful and comparably informative, including neighboring
storage/wire, export/logging, tag/recovery, and analysis/MCP workflows.

The controls include both neutral mechanical work and workflow vocabulary, so
lexical distractors are represented. They are only three cases and do not
estimate a deployment-wide false-positive rate. Explicit transcription wording
makes abstention easier for every semantic selector; disclose that limitation.
Rationales are the author's independent labels, not a blinded second annotator's
agreement study. A reviewer should check them before freezing, without seeing
provider answers; disputed labels require a new version, not retrospective edits.

Twelve cases cannot support broad efficacy or calibration claims. Repeated runs
share tasks and are not independent new tasks. Report descriptive paired results
and case-level uncertainty; do not inflate sample size by treating 72 trials
as 72 distinct tasks. Ordering/caches, service drift, question cueing, incomplete
read telemetry, and residual external file access remain threats. Improvements
to a selector need new held-out cases. No improvement is claimed here.

## Local validation

Run python3 evals/skill-selection/validate.py with the supplied local ahu executable.
It checks file boundaries, counts, labels, options, and weights, then reaches a
verified unregistered-agent refusal AFTER the actual ahu case/suite parser.
It also rejects a deliberately malformed case. It launches no provider.
Do not register @corpus-parse-only; the validator refuses if that manifest exists.
Parser refusal validates structure, not the truth of policy labels or task quality.
The suite schema is 1; its cases use schema 2. JSON flow syntax in YAML front
matter is intentional and accepted by the existing YAML parser.

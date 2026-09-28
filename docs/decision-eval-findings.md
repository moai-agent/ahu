# What the decision evals show

The latest [typed rubric comparison](#typed-rubric-evaluation) found **46.3% less
complete evaluation time and 50.6% fewer native-model input tokens** when Jev
replaced the configured evaluator agent in four synthetic cases on an optimized
build. A separate 12-case calibration matched all 72 reference grading bands.
This demonstrates lower observed evaluation time for this configured rubric
workflow. Earlier skill-advice and batching
experiments did not establish candidate-task efficiency gains; their results
remain below.

The earlier batching experiment on September 28, 2026 exercised Codex 0.157.1 with
`gpt-6-astra` and TypeSafe Jev. Requests selected the mutable `jev-latest` alias;
all eight pilot decision calls reported `jev-1.13.0`.

## A working integration with measurable tradeoffs

The [batching corpus](../evals/batching/README.md) contains two 20-item judgment
batches and a six-item explicit-copy control. Three policies—direct answers,
inline decision questions, and one shared question—ran twice in reversed order.
The binary, candidate context, cases, model and permissions stayed fixed.

All **18 trials passed** their deterministic answer checks: 276 correct returned
fields across 46 unique authored items. Those fields are not independent trials.
All eight intended Jev calls succeeded. Neither the direct arm nor any copy
control called the decision service. Every trial had complete MCP session
telemetry with no receiver drops or errors; native token observations reconciled
against distinct fresh session usage records.

The shared-question API reduced serialized MCP arguments:

| Judgment batch | Inline arguments | Shared arguments | Reduction |
| --- | ---: | ---: | ---: |
| Support ownership, 20 items | 11,427 bytes | 3,824 bytes | 66.5% |
| Change review, 20 items | 11,239 bytes | 3,465 bytes | 69.2% |

Both repetitions produced those request sizes. Bytes describe the serialized
MCP request, not generated model tokens: a coding harness can construct repeated
schemas with a loop. ahu expands the shared rubric into each provider question,
so the smaller MCP form can use more provider input tokens than an inline form
that puts its policy in shared state.

Mean harness elapsed times, with two trials per cell:

| Judgment batch | Direct | Inline Jev | Shared Jev |
| --- | ---: | ---: | ---: |
| Support ownership | 32.6 s | 54.2 s | 45.6 s |
| Change review | 28.5 s | 46.8 s | 46.8 s |

Jev's eight successful service calls had a median recorded duration of **253 ms**.
The full agent task also includes constructing requests, receiving tool results
and continuing the model. Launch wall time, which includes additional harness
startup, is reported separately by ahu Evals.

Mean native input/output token observations:

| Judgment batch | Direct | Inline Jev | Shared Jev |
| --- | ---: | ---: | ---: |
| Support ownership | 70,666 / 358.5 | 95,752.5 / 1,159.5 | 94,254 / 1,067.5 |
| Change review | 71,031.5 / 350.5 | 94,783.5 / 1,115 | 97,972 / 1,120 |

Input includes cached input; output includes reported reasoning. Native totals
were unavailable. Provider tokens are additional, separate observations and
are not added to these figures or converted into a bill. These results do not
support a general claim that delegation makes agents faster or uses fewer tokens.

## Evals rejected a cheaper but less accurate change

A subsequent provider-only experiment moved the shared rubric into provider
state once, instead of repeating it in each question. Two old/new pairs per
judgment batch used identical external MCP arguments: eight calls total.

Provider input tokens fell from 5,987 to 3,820 for support ownership and from
6,369 to 3,765 for change review: reductions of 36.2% and 40.9%. However, the
modified representation made three support-label errors across its two
repetitions. Only two of its four batches passed; the retained representation
passed all four. All calls returned structurally valid typed answers, showing
why schema validation alone is insufficient.

**The optimization was reverted.** The release keeps explicit rubric instructions
in every provider question. This follow-up used already inspected cases and was
not a new held-out agent efficiency test. Its results remain separate from the
18-trial pilot. A fresh eight-item Codex smoke test then passed on the retained
binary, with one successful shared-question call and complete MCP telemetry.

## What this supports

The release provides a compact, provider-neutral decision interface and an eval
workflow that separates answer correctness, tool behavior, native usage and
provider cost. It can expose overhead and reject an apparent optimization when
quality regresses. Use it to test changes to agent instructions, skills and tools.

This small pilot covers one harness/model and three synthetic cases, with two
repetitions per policy. Inline occupied the middle position in both blocks;
provider drift, cache effects and shared-machine variability are not eliminated.
References were omitted from candidate prompts and checkouts, without an OS
boundary preventing outside reads. Forced arm policies do not test natural tool
selection. Two initial setup checks failed on disposable project setup and were
retained separately; no failed formal trial was replaced or dropped.

These bounded observations do not establish accuracy on other tasks, actual
bills, or cross-harness performance. The earlier six-item impact pilot also found
no agent efficiency gain. New benefit claims need fresh tasks and repeated
comparisons under the intended deployment conditions.

## Skill-selection advice

A second experiment on September 28, 2026 moved skill selection before agent
launch. It compared no advice, fixed lexical matching, and Jev relevance scores
using the [independently authored corpus](../evals/skill-selection/README.md).
All arms retained the same full skill catalog and registered agent instructions.
The selector received purpose/state and skill names/descriptions, without bodies,
expected answers, questions, or grading rubrics. Advice contained paths only.

The final matrix had **72 trials: 12 cases, three arms, two repetitions**. Each
trial used a fresh registered Codex session/worktree, `codex-cli 0.157.1`,
`gpt-6-astra` with observed high reasoning effort, and prompt profile
`tool_neutral_v3`. Jev requests pinned `jev-1.13.0`; all 23 successful requests
reported that model. Trials ran sequentially, with case and arm order rotated.
The candidate checkout omitted the cases, reference labels and source history;
this was repository separation, not OS isolation.

The implementation revision was `4d8e00df084c3ce78412ffe7c31a27bfddf90b70`.
The measured executable SHA-256 was
`27564c9f7fec6569178a946a6157a84c6f2cd179749c5294c2253d12e5559a3c`.
Raw records, prompts, plans and native evidence remain outside the repository.
These results are separate from the earlier batching experiment.

### Task results and cost

Means per trial, including the retained provider failure:

| Policy | Correct tasks | Preparation through candidate completion | Full command through grading | Native input | Native output |
| --- | ---: | ---: | ---: | ---: | ---: |
| No advice | 24/24 | 42.08 s | 43.69 s | 48,939 | 245.0 |
| Lexical | 24/24 | 42.29 s | 43.88 s | 48,572 | 228.8 |
| Jev | 24/24 | 43.38 s | 44.99 s | 45,885 | 232.5 |

Each arm returned all 52 question answers correctly. Input observations include
cached input; they are not unique context sizes or bills. Provider units remain
separate. Successful Jev calls averaged 1,999.3 input and 244 output tokens over
23 observations; the failed call's provider usage is unknown. Mean reported
successful provider duration was 239.2 ms. Full selection preparation averaged
1.848 s across all 24 Jev attempts, including catalog validation, transport and
the failure. Lexical preparation averaged 1.044 s. A loopback bridge let the
source-scoped MCP server read its credential without copying it into candidate
repositories; that bridge's overhead was included.

Relative to no advice, Jev had **6.24% lower observed native input** and **3.10%
higher preparation-through-completion time**. Neither establishes an improvement:
paired case-cluster bootstrap intervals for the ratio of arm means were **−14.49% to +1.93%** for input and
**−0.11% to +7.12%** for time. These descriptive percentile intervals used 10,000
resamples of the 12 cases, keeping both repetitions and paired arms together,
with seed 20260928. Repetitions were not treated as 72 independent tasks.
Full-command time was 2.96% higher, with interval −0.13% to +6.82%.

The declared efficiency criterion required observed quality nonregression and
at least 10% lower total time or native input, with the corresponding 97.5th
percentile ratio below one. This upper bound accounted for testing either of two
primary efficiency measures. **The criterion was not met.** The small,
pilot-informed and interrupted study does not support a general speed, token,
monetary-cost or deployment-quality claim.

### Selection quality

Reference labels were authored before provider results and reviewed against the
fictional policy bodies by the coordinator and an independent registered agent.
A skill counts as required only when its body supplies a needed policy fact.

| Measurement | Lexical | Jev |
| --- | ---: | ---: |
| Successful selector observations | 24/24 | 23/24 |
| Required paths among suggested paths | 26/58 (44.83%) | 26/26 (100%) |
| Required paths recovered on successfully observed applicable cases | 26/26 | 26/26 |
| Exact required sets on successful observations | 6/24 | 23/23 |
| Successful no-skill observations with unwanted suggestions | 4/6 | 0/5 |
| Failed/unknown selector observations | 0 | 1 |

The failed Jev request occurred on a no-skill control. It is **not** credited as a
correct abstention. On successfully observed nonempty suggestions, macro
precision was 48.48% over 22 lexical observations and 100% over 18 Jev
observations. Macro required recall was 100% over 18 applicable observations in
each arm. Jev precision has five successful empty-set observations with an
undefined denominator and one failed/unknown observation. Lexical precision has
two successful empty-set observations. These remain separate from successful
accuracy measurements.

This is positive evidence of more precise advice than this simple lexical policy
on these 12 tasks. It does not demonstrate that advice improved task answers:
all arms were already correct. Native tool returns exposed the same 26 required
skill bodies per arm, across 18 applicable trials, and no bodies on controls.
Body exposure does not prove use; absence of a returned body does not establish
absence of native loading. The observed equality is consistent with the agent
already finding the right skills without advice. The full catalog remained
available, so this experiment did not test reducing catalog context.

### Failures, revisions and measurement coverage

An initial 28-attempt pilot stopped after a literal-copy answer returned status
text where the grader required option keys. The original shared prompt had not
specified that encoding. The shared template now explicitly describes choice-key
and numeric answers, and its version/profile were incremented. All 28 pilot
attempts were retained separately; every arm restarted. No corpus, labels,
selection threshold, skill descriptions or expected answers were tuned. The
revised run is pilot-informed validation, not a pristine held-out study.

At trial 48 of the revised matrix, TypeSafe returned HTTP 520. The agent still
answered correctly after the recorded empty fallback. The schedule paused as
specified; a separate health probe succeeded, then only the 24 unattempted trials
continued. No failed trial was retried or replaced. This interruption and the
post-failure continuation limit confirmatory interpretation. An analysis-script
correction enforced the original rule that a failed selector is unknown, not a
successful empty set; it changed no primary timing/token comparison or threshold.

All 72 trials had complete MCP session telemetry. All 48 enabled prelaunch
selections had their separate OTel observation. Native input/output/cache totals
reconciled against the exact 72 owned native sessions with zero mismatches.
No audited tool request mentioned the external grading files; this is not proof
of OS isolation. Candidates made no ahu MCP tool calls in this workload: Jev was
used by prelaunch orchestration, while native tools read skills and wrote
answers. Synthetic integration tests separately exercise candidate-initiated MCP
selection costs on both successful and missing-answer paths.

The actual `ahu eval report` retained the failed and successful selections in
the same configured arm, reported the fallback, and showed only one complete
provider-usage observation for that case's two Jev attempts. Unknown billed work
was not represented as zero. Full post-fix validation passed 1,112 tests across
43 suites, plus formatting, clippy with warnings denied, skill validation and
corpus parser validation. Passing tests establish checked behavior, not efficacy. An independent registered
ahu reviewer reproduced the ratios and bootstrap endpoints, verified the preserved
48-row prefix and schedule, and checked all 156 saved answers against the references.

### Release disposition

Keep skill advice opt-in. A defensible bounded claim is: **Jev produced more
precise skill advice than the lexical baseline on this synthetic corpus, while
preserving the observed task accuracy.** Do not market proven agent speed or
token savings from these skill-advice results. This experiment did not meet its
positive agent-efficiency gate. The subsequent typed-rubric study below tests
removing a separate grading-agent invocation using a fresh corpus.

## Typed rubric evaluation

A third experiment on September 28, 2026 tested replacing the optional evaluator
agent with `ahu eval run --decision-evaluator`. ahu sends one bounded rubric
request directly to the configured typed-decision provider. The candidate still
runs normally. This measures the evaluation workflow with a configured Codex
judge; it does not establish faster coding or better candidate answers.

### Calibrating the grading stage

The [grading corpus](../evals/decision-grading/README.md) contains 12 synthetic
cases across six domains and two separate pilot cases. A registered ahu agent
authored the references and another reviewed them before provider runs; these
are model-reviewed references, not independent human annotations. Rubrics grade
selected option meanings against supplied evidence. They do not assess arbitrary
code changes or replace expert review.

The initial pilot returned four of six grading bands correctly from Jev. One
generic revision resolved choice keys into their exact selected option text
locally, keeping that text as untrusted data. The revised pilot returned six of
six correctly. Both pilots remain separate from the confirmatory dataset. No
reference labels, held-out cases, score cutpoints, or acceptance gates changed.
The native prompt retained its original structure, so the experiment compares
two grading workflows, rather than isolating a model-only effect.

The frozen comparison used a debug executable and ran **48 attempts: 12 cases, two graders, two repetitions**.
Each pair graded the same saved candidate answer. Runs were sequential, with arm
order reversed and case order rotated for the second repetition. Native grading
used registered Codex agents with `codex-cli 0.157.1`, `gpt-6-astra`, and observed
high reasoning effort. Typed grading pinned `jev-1.13.0`; every service response
reported that version. All attempts were retained, with no retries or substitutes.

| Grading-stage measure | Codex evaluator agent | Jev typed evaluator |
| --- | ---: | ---: |
| Correct reference bands | 72/72 | 72/72 |
| Continuous mean absolute error | 0 | 0.0240 |
| Critical false accepts | 0/18 | 0/18 |
| Mean complete grading time | 37.185 s | 0.301 s |
| Complete MCP session observations | 24/24 | 24/24 |
| Native input / output per attempt | 31,110 / 213.9 | No native grader invoked |
| Native cached input per attempt | 23,291 | No native grader invoked |
| Jev input / output per attempt | No Jev call | 1,689.6 / 43 |

Jev used **99.19% less grading time**. The paired case-cluster bootstrap 95%
interval for the reduction was **99.12%–99.24%** (10,000 resamples, retaining both
repetitions within each case). Timing includes prompt/request formation, launch
or provider transport, result parsing, artifact capture, and schema validation;
common case loading and collector setup are outside this stage timer. Debug-build
and harness-startup overhead is included. These are measured workflow latencies,
not a comparison of pure model inference or optimized release latency.

All predefined gates passed: band agreement at least 95% and no worse than the
native judge; continuous error at most 0.10 and at most 0.05 above native; no
critical false accepts or unknown tagged outcomes; mean time ratio at most 0.8
with an upper 95% bound at most 0.9; and complete attempt, telemetry, and usage
accounting. Bands use fixed cutpoints 0.25 and 0.75 on raw scores. The 72 judgments
are repeated criteria across 12 cases, not 72 independent tasks. Perfect observed
agreement does not establish a population-wide accuracy guarantee.

Native input includes cached input. Native and Jev token units remain separate;
these figures are neither a combined token total nor a subscription-bill estimate.
No provider failure occurred in the formal run, so this is not a reliability
estimate for outage conditions. Local tests cover failure accounting separately.

### Full CLI workflow

The initial CLI matrix used the same debug executable: four fixed cases, three
configurations, one fresh candidate run per cell. All 12 passed. It observed a
50.64% reduction in complete evaluation time and 50.60% fewer native input tokens
with Jev. Those debug timings are retained separately.

Before seeing optimized results, the same 12-trial schedule and acceptance checks
were frozen for a separate `cargo build --release` run. Native completion
reporting and narrow-table labels were corrected; grading requests, references,
models, and metric definitions were unchanged. Both CLI matrices used a temporary
loopback transport to the source-scoped MCP credential reader; its overhead is
included, and credentials were not copied into candidate checkouts.

**Optimized-build means per evaluation:**

| Configuration | Correct answers | Complete evaluation | Full command | Native input / output | Jev input / output |
| --- | ---: | ---: | ---: | ---: | ---: |
| Candidate + Codex grader | 4/4 | 29.23 s | 29.36 s | 60,118 / 310.5 | No call |
| Candidate + Jev grader | 4/4 | 15.69 s | 15.80 s | 29,706 / 133.3 | 1,695 / 43 |
| Candidate + deterministic checks | 4/4 | 13.73 s | 13.84 s | 29,747 / 138.3 | No call |

Jev reduced observed complete evaluation time by **46.33%**, full command time by
**46.19%**, and native-model input by **50.59%** against the configured Codex
evaluator workflow. Native output fell 57.09%. Native input includes cached input;
mean cached counts were 50,528, 23,808, and 20,960 respectively. Native candidate
and grader counters share the same configured model and units here, so their
input/output counts can be combined; Jev units remain separate.

Candidate answers were identical across all three arms for each case, and all
headlines passed. The two graders agreed on all returned criterion bands. Every
candidate had a complete MCP session; all four native graders had complete
sessions; all four typed graders exported their distinct grading observation.
Provider usage matched the four saved service responses. Native model, usage,
terminal completion, and unique task/session identities were checked against
native records.
No attempt was retried, replaced, or excluded; no receiver drops were observed.
The actual `ahu eval report` displayed all 12 configurations with separate
candidate and grader accounting.

The optimized run met its predefined observational checks: identical correct
answers and grade bands, at least 20% less complete evaluation time and native
input, and complete attempt/telemetry/usage accounting. This is an exploratory
four-case integration comparison, with one trial per cell. The calibration's
confidence interval does **not** apply to this full-workflow result. These cases
have exact references: deterministic checks avoid the optional model grader and
were the fastest observed control. Use a model grader when a validated rubric
requires it; this does not justify adding Jev to every evaluation.

The optimized measured revision was
`d59717741b82b4bdf05bf40a563c0f89813a02f5`; executable SHA-256
`59629a6164a3e39a802aea6a1a723b2fd1dd09873a87fcb62906b3db0d69961f`.


### What Evals and OTel established

The calibration exercised the real `ahu_typed_decide` MCP boundary. Every typed
call joined to exactly one complete session, with service identity and usage
matching the returned response. Native judges had complete MCP sessions with
zero typed calls; owned native session logs independently matched the configured
model, terminal completion, and input/output/cache counters. Saved requests and
prompts were checked against the frozen evidence, answers and rubrics. No
reference labels or expected answers were added to them.

Candidates and native graders made no ahu MCP tool calls in these matrices;
their native tools wrote answer and score artifacts. Typed grading was invoked
by ahu orchestration. This experiment did not measure agents choosing when to
call Jev. `complete_session` confirms an MCP session summary was received;
`typed_decision_span` confirms the separate local grading observation arrived.
Neither label claims a provider-side trace or complete capture of every native
tool action. The coordinator separately checked the exact owned native sessions.

A separate metadata defect made successful Codex tasks report that their native
terminal event was missing. Source and owned-session review traced it to dropped
`turn.completed` metadata. The measurement retained this warning and used direct
native completion evidence. The fix passed focused tests and all 16 native sessions in the optimized
comparison reported complete native evidence. Later compatibility work accepts
valid provider scores with absent service metadata, recording identity and usage
as unknown; malformed supplied metadata still fails. That absence-only change
does not change any measured Jev request or response.

The debug calibration and debug CLI measured revision was
`bb4d995872c048627a43f6674f899195ca542321`; executable
SHA-256 `0f8471b36f95c8bf08180e6c42fac7ec42cf005e3ec93c69bb9cb05672c7394d`.
Plans, raw requests, outputs, and native evidence remain outside the repository.
The candidate/judge checkout omitted the corpus and references; same-user OS
access was not isolated. Results support calibrated, bounded rubric evaluation
with this configuration. Broader tasks, other judges/harnesses, and richer
explanations need their own comparisons. Earlier batching and skill-advice
results remain unchanged.

### Validation and review

An independent registered ahu reviewer reproduced the aggregates, bootstrap
endpoints, schedule identities, frozen-input hashes, and saved-data joins. It
found no additional substantive production or security blocker in the inspected
paths. This was a bounded review; the reviewer checked saved native audits and
result envelopes without independently repeating the raw native-history audit.

Final validation passed **1,133 tests across 43 suites**, formatting, clippy with
warnings denied, repository skill checks, and corpus schema/hash/boundary checks.
The earlier full-suite failure was a stale mock expecting three request-state
fields after selected-answer projection added a fourth. Its correction asserts
the exact four-field boundary and resolved value; the final full suite passed.
The report now keeps grader labels visible in narrow terminals and explicitly
labels candidate token scope. Its JSON metrics remained unchanged.

Measurements belong to the revisions named above. Later changes cover optional
metadata absence, test expectations, and reporting clarity; final-head optimized
latency was not remeasured. Passing tests and this review do not authorize release
or publication.

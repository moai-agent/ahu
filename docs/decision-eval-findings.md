# What the decision evals show

The latest [skill-selection comparison](#skill-selection-advice) improved advice
precision, but did **not** demonstrate faster agents or reliable token savings.
The agent-efficiency release gate remains unmet. Earlier batching results follow.

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
token savings from these results. The requested positive agent-efficiency release
confirmation has not been obtained. A next experiment should remove a measured
source of work—such as unnecessary catalog context or a separate classification
turn—and use fresh tasks before making an efficiency claim.

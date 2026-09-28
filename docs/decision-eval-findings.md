# What the decision evals show

Local synthetic validation on September 28, 2026 exercised Codex 0.157.1 with
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

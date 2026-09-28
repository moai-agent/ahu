# Typed-decision impact checks

This corpus measures answer quality independently of tool use. It contains two
batches of six policy judgments and a control with six explicitly stated fields.
All data is synthetic. The expected answers are deterministic policy applications,
not an agent judge's opinion. No case requires a tool call to earn answer credit.

## Comparison

Use a disposable candidate repository without the case files or reference answers.
Keep records, prompts, reports, and native logs outside the source checkout. Keep
one binary, repository revision, skill bundle, MCP configuration, permission mode,
and exact model fixed within each harness comparison.

Compare two manifest policies with the same tool and skill availability:

- Baseline: answer locally within the candidate model; do not delegate judgments.
- Treatment: use the typed-decisions skill to batch multi-factor judgments into a
  single call when appropriate; check the returned choices against the policy.
  Copy explicit fields directly in both arms.

This measures the effect of a delegation policy. It does not isolate skill
installation or tool availability. Record the manifest diff. A tool call is a
manipulation check, never answer-quality credit. Show whether the treatment actually
delegated and whether the baseline obeyed its restriction.

Start with two repetitions of each case and arm on each harness: 24 candidate
trials. Reverse arm order in the second repetition. Use `--runs 1` in each ordered
block because the runner groups repeated trials by agent. Additional repetitions
must retain the same cases, policies, binary, and timeout. Keep pilot and later
validation results identifiable. Do not tune instructions on held-out results.

## Evidence required before a benefit claim

1. Complete a real candidate trial on each exact harness version. Confirm prompt
   delivery, answer artifacts, native usage semantics, and local OTel correlation.
2. Preserve every attempt. Report launch failures, timeouts, invalid answers,
   scored wrong answers, and telemetry gaps separately. All-attempt reliability
   and quality among valid answers are different rates.
3. Report paired differences within each harness and case. Print every raw pair,
   the number of matched observations, and missing or failed pairs. Conditional
   timing or token means alone cannot establish an improvement when failures differ.
4. Keep agent input, output, cache-read, cache-write, and reasoning observations
   separate. Validate whether native events are incremental or cumulative before
   aggregating. Do not sum repeated cumulative events or invent missing totals.
5. Show TypeSafe usage and successful-call duration beside agent measurements.
   Missing service usage after an error is unknown. Do not add tokens from different
   models, equate token counts with provider bills, or subtract summed service
   durations from wall time to infer model time.
6. Require full tool telemetry for abstention claims. Distinguish attempted calls,
   successful calls, and errors. Preserve receiver-loss diagnostics.

The initial small sample can establish operation and describe observed differences;
it cannot prove broad accuracy preservation or universal savings. A release claim
must name the tested tasks, harnesses, models, repetitions, observed tradeoffs, and
limitations. If quality falls, failures increase, or no useful saving appears,
record that outcome. A working integration alone does not establish efficiency.

Use the existing `ahu eval run --suite` and `ahu eval report --records` commands.
Run the suite from this source checkout against the disposable candidate repository;
the candidate receives only the visible state and questions. Files outside a
candidate checkout are not strong environmental blinding: keep reference paths out
of prompts and do not claim inaccessible answers without a separate read boundary.

# Shared-question decision batches

This synthetic corpus checks whether a compact request improves delegation for
homogeneous batches: support ownership, software change review, and an explicit
status-copy control. It complements the smaller cases in `evals/impact`.

## Fair comparison

Prepare a disposable candidate repository with the same model, harness version,
skill, MCP configuration, permissions, and committed context for all three agents.
Keep case files and expected answers out of that repository. Keep all execution
records outside every repository. See the [impact protocol](../impact/README.md)
for native trust, per-tool approval, token semantics, and failure accounting.

Use three agent policies, with identical common instructions:

| Arm | Judgment batches | Explicit copying |
| --- | --- | --- |
| direct | Answer within the candidate model; no decision call | Copy directly |
| inline | One `ahu_typed_decide` call with `state` and named `questions` | Copy directly |
| shared | One call with named `items` and a shared `question` | Copy directly |

All arms should read the installed typed-decisions skill, minimize unnecessary
commentary, and write `answer.json`. The inline and shared arms should delegate
before solving all items, then check coverage, valid values, and contradictions;
inspect exceptions without automatically repeating every judgment. Keep the
policy and relevant evidence identical, including IDs and option meanings.
The assigned arm overrides the skill's general format recommendation. Audit actual
requests for semantic equivalence and arm compliance; do not artificially pad the
inline form or strip facts to favor one arm.

These forced policies measure delegation and request format. They do **not**
measure whether an unconstrained agent chooses the tool well. Shared versus
inline measures request-format policy, including any resulting agent wording and
verification effort; it does not isolate pure serialization overhead. The control detects
one form of unnecessary tool use; it is not a comprehensive routing benchmark.

Freeze the binary, cases, manifests, skill, model, timeout, and analysis plan
before measured runs. Record the decision-provider configuration and reported
model too: `jev-latest` is a mutable provider alias, not a frozen weight version.
A bounded pilot uses two blocks, in orders
`direct, inline, shared` and `shared, inline, direct`: 18 trials total.
Use `--runs 1` in each block because the runner groups repetitions by agent.
The inline arm stays in the middle, so this is not full position counterbalancing.
Preserve failed trials; do not silently replace them. Stop on infrastructure
failure and report an incomplete matrix before changing its frozen inputs.

```sh
ahu --repo "$CANDIDATE_REPO" eval run   --suite "$AHU_SOURCE/evals/batching/suites/typed-decision-batching.md"   --agent @direct --agent @inline --agent @shared --runs 1   --timeout 180 --allow-widened-approvals   --records "$EXPERIMENT_DIR/first.jsonl"
ahu --repo "$CANDIDATE_REPO" eval run   --suite "$AHU_SOURCE/evals/batching/suites/typed-decision-batching.md"   --agent @shared --agent @inline --agent @direct --runs 1   --timeout 180 --allow-widened-approvals   --records "$EXPERIMENT_DIR/second.jsonl"
cat "$EXPERIMENT_DIR/first.jsonl" "$EXPERIMENT_DIR/second.jsonl" > "$EXPERIMENT_DIR/combined.jsonl"
ahu eval report --records "$EXPERIMENT_DIR/combined.jsonl"
```

The environment variables above are paths selected by the operator, not secret
values. The approval flag requires intentionally configured candidate manifests.
Use synthetic inputs only for live provider calls. Do not copy credentials into
candidate repositories or bypass native trust.

## Read the outcome

- Score every item against its deterministic expected answer; a tool call earns
  no answer credit. Show failed attempts and incorrect answers.
- Compare shared against inline to examine request-format overhead, and each
  against direct to examine the end-to-end delegation tradeoff.
- Report native input/output/cache/reasoning observations with coverage and
  provider usage separately. Do not equate request bytes with tokens or bills.
- OTel records `ahu.mcp.decision.arguments.bytes` and request format without
  recording payloads. Eval records preserve measured argument byte sums, measured
  call counts, and format counts, including calls that fail. The CLI report
  shows mean measured bytes per run with its measurement counts. Missing call
  spans remain missing evidence, even if a session summary arrived.
- Inspect fresh native session IDs and tool outcomes. Complete server telemetry
  alone cannot rule out approval denials before a call reaches the server.
- Two repetitions describe this pilot only. New cases should be held out before
  tuning the next skill version; do not tune these cases into a release claim.

Compact requests are useful even when delegation remains slower. Claim measured
request reduction separately from agent time, tokens, and answer quality.

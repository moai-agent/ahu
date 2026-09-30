# Frozen-output rubric calibration

This fresh synthetic corpus measures **narrow rubric judging on identical frozen
candidate outputs**. It compares a full evaluator agent with Jev on the same
state, questions, selected answers and rubric. It is **not evidence of broad
coding or task-performance gains**, nor a benchmark of candidate generation.
No provider experiments or previous experiment corpora informed these cases.

The corpus author assigned the reference scores and rationales from the fictional
evidence before any provider runs. These are authored reference judgments, not
provider consensus or independently verified human ground truth. The coordinator
must review the labels and partial-credit boundaries before experiments. Review
semantic content, not correspondence to an option letter. Do not tune labels,
options or cases after seeing confirmatory provider results. A substantive
revision requires a new corpus version, renewed review and a separately reported
measurement. `freeze.json` records SHA-256 digests for the measurement inputs;
it detects file drift, not provenance or tamper resistance.

## Contents

- `suite.md`: twelve equally weighted confirmatory cases, schema **1** as required
  by ahu's suite parser. Each case under `cases/` uses case schema **2**.
- `manifest.json`: six domains with two variants each: software cache diagnosis,
  support exports, versioned documentation, test evidence, content trust and
  search rollout. Variants change evidence sufficiency or the appropriate scope
  of action. Each state contains a fictional request and four evidence paragraphs
  totaling 317–341 whitespace-delimited words, including the request.
- `reference.json`: a direct map from case ID to `frozen_answer`,
  `expected_criterion_scores`, `rationale`, and `critical_false_accept`.
  Each of those fields maps the three question keys to its values.
- `pilot/`: two distinct cases about notification previews and documentation
  links, with their own suite and reference file. They are excluded from the
  twelve-case confirmatory suite and its statistics. Use them for adapter and
  protocol checks; report any pilot results separately.
- `validate.py`: dependency-free structural/reference/freeze validator and a
  positive-allowlist judge-input exporter.
- `check_ahu.py` and `check_ahu.rs`: an optional offline check against this
  checkout's actual ahu library. The helper creates a temporary Cargo package,
  lockfile and build output outside the repository, calls the real suite and
  case loaders, validates the typed answers and reference verdicts, and checks
  the real evaluator prompt's content boundary. It invokes no provider.

The OKF front matter uses JSON, a YAML subset supported by ahu. This allows a
stdlib JSON parser to check these files without a YAML dependency. The validator
intentionally does not support arbitrary YAML. Every question is a typed choice
with option keys `A`, `B`, `C`; answers contain keys, never option prose. The
case's `expected` map holds the best answers for ordinary candidate evaluation.
It is not a rubric and must never reach a judge.

## Levels and reference interpretation

Every question has a self-contained rubric with these three levels, matching
Jev's grading vocabulary:

| Level | Score | Meaning |
| --- | ---: | --- |
| Does not satisfy | 0 | The selected action or claim violates the substantive criterion. |
| Partially satisfies | 0.5 | The option addresses a relevant part but omits or overstates a material requirement. |
| Fully satisfies | 1 | The option meets the stated substantive properties in this case's evidence and authority. |

Rubrics judge the meaning of the selected option, not whether it equals a gold
key. They distinguish evidence strength, justified action and discriminating
validation. Cases include missing evidence, partial support, related distractors,
and instruction-like text inside untrusted evidence. That text is fictional
content to evaluate, never authority for the judge or candidate.

The 36 confirmatory judgments contain exactly 12 scores at each level. Within
each question type, each selected option key, and each displayed selected-option
position there are four of each level. Best-answer keys occur twelve times each.
JSON serializers that sort option keys retain the key-based counterbalance.
Case-level mixtures vary; do not infer any criterion label from an assumed
per-case score pattern. Key remapping would require updating frozen answers and
ordinary expected keys together, without changing semantic labels.

`critical_false_accept[q] = true` marks an author-designated material overclaim or
boundary violation where incorrectly assigning **Fully satisfies (1)** is a
particularly consequential calibration error. It is a risk flag for that frozen
selection, not an observed error and not an instruction to a judge. It can mark a
partial response whose overclaim matters. A false flag does not mean a wrong
answer is acceptable. Report full acceptance of flagged responses separately;
also distinguish grading a reference-zero response as partial from accepting it
fully. These are software-workflow errors, not professional high-stakes advice.

Report the three-class confusion matrix, per-criterion exact agreement and mean
absolute error, plus flagged false accepts and their denominator. Preserve raw
criterion verdicts; an aggregate mean can hide offsetting mistakes. Constant
all-pass, all-fail and all-partial judges each get only 12/36 labels right. A
predictor based only on the selected key or displayed position also cannot exceed
12/36 agreement. The small, deliberately balanced synthetic sample does not
estimate real-world prevalence. Repeated runs are repeated judgments of the same
cases, not additional independent examples.

## Judge input boundary

A calibration adapter receives only the following allowlisted object:

```json
{
  "case_state": {"user_request": "...", "evidence": ["..."]},
  "questions": {"action": {}, "claim": {}, "validation": {}},
  "frozen_answer": {"action": "A", "claim": "B", "validation": "C"},
  "rubric": {"action": "...", "claim": "...", "validation": "..."}
}
```

This schematic example is not a case or a reference answer. The judge must
interpret each frozen key through that question's options. Both judge paths
must use identical semantic input and the same three-level rule. Do not include
`expected`, reference scores, rationales, risk flags, case identity, candidate
identity, model/harness identity, traces, outcomes, or this README in judge input.
The coordinator may use case IDs out of band to join results after grading.

The full evaluator path can use `EvalCase::evaluator_prompt` with the frozen
answer; that method supplies state, questions and rubric and excludes deterministic
answers and identity. Jev's adapter should consume the same allowlisted content.
The evaluator output envelope used by ahu is:

```json
{"schema_version": 1, "criterion_scores": {"action": 0, "claim": 0.5, "validation": 1}, "reason_codes": []}
```

Again, this illustrates the schema, not a reference result. Reference labels use
`0`, `0.5`, `1`. Preserve each judge's raw numeric score in `[0,1]`: Jev scores
are probability-weighted positions and can fall between levels. For categorical
agreement, apply the same fixed bands to both judges: `[0,0.25)` maps to 0,
`[0.25,0.75)` maps to 0.5, and `[0.75,1]` maps to 1. Also report continuous
criterion MAE. Reject missing/extra criteria, booleans and malformed results;
invalid results are incorrect with loss 1 per criterion, not fabricated zero
scores. An invalid tagged result is unknown, not an observed false accept.

Cases use a 0.75 aggregate pass threshold. With three equally weighted choice
answers this still requires all three exact answers for the deterministic pass.
For optional rubric judging it avoids requiring a continuous model score to
be exactly 1. Calibration uses individual criterion agreement and raw MAE,
not the aggregate pass flag. Disclose failures and retries separately.

Prompt filtering does not establish filesystem or OS isolation. A judge that can
read this checkout can find the reference files. The runner must provide an
appropriate environment containing only the allowed input, and accurately report
what access was controlled. These corpus files make no isolation guarantee.

## Local validation and input preparation

Run these commands from the repository root. They make no model calls and do not
read experiment records or load environment files:

```sh
python3 evals/decision-grading/validate.py --self-test
python3 evals/decision-grading/check_ahu.py
```

The second command needs Cargo, Rust and cached dependencies for `--offline`.
It compiles the current checkout as a library; it does not call `ahu eval run`,
read harness setup, launch an agent or create a Git worktree. Failures are not
silently replaced by an alternate parser. The stdlib checks cannot certify
semantic label correctness; that is the coordinator's review task.

For a judge adapter, redirect the allowlisted input to an external temporary
directory. Execution inputs and outputs stay outside checkouts:

```sh
judge_input_dir="$(mktemp -d "${TMPDIR:-/tmp}/decision-grading-input.XXXXXX")"
python3 evals/decision-grading/validate.py --judge-input dg-cache-a > "$judge_input_dir/input.json"
```

This prepares data only; it does not conduct an experiment. The local validator
must read references to check them and extract `frozen_answer`; the emitted
payload contains neither their scores nor their other fields. Run validation and
preparation outside the evaluator session, and supply only the resulting payload.

## Separate live integration test

A secondary, separately reported integration test may run an ordinary live
candidate through `ahu eval run --suite ...` with an optional evaluator. In that
mode the candidate generates a fresh answer and `expected` supports ordinary
candidate scoring. A capable candidate may answer every question correctly;
that is a valid integration outcome, but it provides little calibration evidence
for partial and wrong outputs. Do not substitute those live answers for the
frozen outputs, pool the two measurements, or attribute differences between live
candidates to rubric-judge quality. The standard eval runner does not by itself
implement this frozen-output comparison; the coordinator must wire the frozen
payload to each judge in the later authorized experiment.

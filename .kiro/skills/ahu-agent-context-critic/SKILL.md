---
name: ahu-agent-context-critic
description: Use ahu evals and available telemetry to diagnose and improve an agent's committed context without overstating what the harness loaded.
---

# Agent context critic

Use this skill to investigate a concrete agent failure or improvement goal. Treat
context edits as hypotheses to test, not as cleanup. The user reviews proposed
changes and commits them; never commit or push as part of this workflow.

## Boundaries

- `ahu.lock` fingerprints recognized repository context. `ahu lock` checks it;
  `ahu lock --update` refreshes it. Agent launches require the lock and all
  recognized project context inputs to be tracked, clean, and committed.
- The lock does not capture harness built-ins, user or managed settings, provider
  memory, caches, or context that ahu cannot inspect. Call these unknown; do not
  describe repository files as the complete context the model received.
- A skill or instruction being present does not prove the harness loaded it.
  Telemetry can show MCP tool discovery and calls, errors, and typed decisions;
  it does not show all native skill or prompt loading.
- Eval records and run artifacts belong outside the repository. Do not copy
  prompts, traces, transcripts, or private run artifacts into repository files.

## Workflow

1. **Name the behavior.** Ask which agent, harness, task, and observed failure or
   success matter. Separate task outcome, tool behavior, latency, and token use.
   If there is no concrete behavior to measure, collect an example with the user
   before proposing a context edit.
2. **Read committed inputs.** Check `ahu lock`, inspect `ahu.lock`, the registered
   agent manifest, the referenced instruction file, and relevant repository
   context. Use Git to establish their committed paths and revisions. Label what
   is declared, what ahu fingerprints, and what remains unknown to the harness.
3. **Build a fair eval.** Use OKF Markdown cases and a suite for the target
   behavior. State a clear task and success criterion. Prefer deterministic
   checks for exact answers and OTel-backed tool expectations; use an agent
   evaluator only for rubric dimensions that need judgment. Balance cases where
   a tool or instruction should apply with cases where it should not. Verify the
   task and grader with a known-good solution when practical.
4. **Run a baseline.** Use `ahu eval run --suite PATH --agent @name --runs N
   --records OUTSIDE_REPO.jsonl`. Choose enough repetitions to see variability.
   Run the same harness, model, tasks, environment, and resource limits for each
   context variant. Keep scoring independent from telemetry coverage.
5. **Critique the evidence.** Inspect `ahu eval report --records
   OUTSIDE_REPO.jsonl`, case-level results, OTel coverage, and available external
   run artifacts. Distinguish an agent error from an ambiguous task, incomplete
   telemetry, harness limitation, and grader error. Treat the evaluator's
   reasoning as a hypothesis. Human-review a sample of judgments and failures.
6. **Propose one change.** State the failure evidence, the smallest context edit
   likely to address it, a counter-hypothesis, and a regression risk. Avoid
   adding broad rules for a one-off case. Do not remove context merely because it
   exists.
7. **Commit the experiment before running it.** After the user reviews the
   proposal, edit the context on an experiment branch, run `ahu lock --update`,
   inspect the context and lock diff together, then have the user commit both.
   ahu will refuse candidate launches while either context or lock is uncommitted
   or the lock is stale. This makes every evaluated context variant reproducible
   by its Git revision.
8. **Compare and decide.** Append candidate runs to the same external JSONL and
   run `ahu eval report`. Compare per-case outcome and tool-use results, repeated
   trial variability, token/timing coverage, and regressions. Keep the change
   only when the evidence supports the intended improvement without unacceptable
   regressions. If the suite is saturated or fails to distinguish variants,
   improve the suite before drawing a conclusion.

## Eval quality checks

- Keep task intent and candidate-visible information separate from grading
  answers and rubrics. Prompt hiding is not filesystem isolation: if case files
  live in a candidate checkout, assume the candidate can read them. Use cases
  whose hidden answers are not security secrets, and do not claim a blind test
  unless its grading data is actually inaccessible to the candidate.
- Prefer grading the result and required tool behavior over an exact tool-call
  sequence. Multiple valid strategies should pass unless the tool choice itself
  is the behavior under test.
- Keep a regression suite as well as low-scoring capability cases. A change that
  improves one case can make the agent over-trigger instructions or tools on
  unrelated tasks.
- Use `ahu eval report` to compare fingerprints, not to infer causation from a
  score alone. Change one context component per experiment where possible.
- OTel is process evidence. A missing observation is unknown coverage, not proof
  that an agent did not use a tool. A tool call is not proof the task succeeded.

Maintain `.agents/skills/ahu-agent-context-critic/SKILL.md` as the canonical
repository skill. `ahu setup` distributes the exact bundled bytes to detected
harnesses. After editing this skill, run `python3 scripts/check-skills.py`.

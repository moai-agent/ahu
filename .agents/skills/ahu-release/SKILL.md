---
name: ahu-release
description: Prepare, verify, merge, and tag a repository release candidate with hermetic CI and explicit publication gates; use with Codex or OpenCode, never Claude Code.
---

Use this skill when a maintainer wants to turn the current repository state into
a release candidate, get its hosted checks green, merge it, and publish a tag.
It covers release mechanics and verification; it does not decide what product
scope belongs in a release.

Treat roadmap records, work items, milestones, and release objects as a
provider-neutral planning layer. A project may map them to GitHub issues and
milestones, another tracker, or an MCP task service. Discover the active
provider's fields and visibility at runtime; do not embed fixed issue numbers,
repository names, label sets, milestone IDs, project IDs, or a requirement that
one work item maps to exactly one pull request. Follow repository policy for
whether release work items are required and how they link to ahu task IDs. If no
tracker is configured, keep coordination in the release workflow and ahu task
records; do not create an assumed GitHub or private-roadmap workflow.
Use the bundled `ahu-direct-agents` skill for task IDs, provider-side links, and
tracker lifecycle rules; this release skill adds release-specific local task
cleanup without changing the provider's records.

This skill must not run under Claude Code. If the active harness is Claude Code
or identifies itself as `claude`/`claude-code`, stop before making changes or
external mutations and report that the maintainer must rerun the workflow with
Codex or OpenCode.

Read the repository's `AGENTS.md` and the current release notes before changing
anything. Start from a clean checkout and record the exact base commit. Keep
release work on `release/vX.Y.Z` (or the repository's documented equivalent),
and make a separate post-release branch from updated `main` for improvements to
this workflow or its skill. Do not mix that follow-up work into the release PR.

Establish publication scope from the user's existing instructions. Honor an
already authorized full autonomous release without requesting the same approval
again. Preparation or testing alone does not authorize publication. If authority
is missing, finish the candidate and its checks first, then request one concrete
authorization naming the exact candidate commit, destination branch, merge,
version tag, remote pushes, and artifacts to publish. Include only operations
still requiring approval. Push only explicitly authorized branches and tags;
never force-push or rewrite an existing tag.

Bind every validation result to the exact candidate commit, command, environment,
and outcome. Commit candidate changes before the final checks. Any subsequent
edit creates a new candidate that needs its required checks and review; do not
carry forward a green status from another SHA. Historical candidate notes are
dated evidence, not the current publication status. Query the actual review,
remote branch, tag, and release objects before reporting what is published.

Make CI prove the release boundary without paid services:

- Run formatting, lint, unit and integration tests, documentation and knowledge
  checks, dependency policy, and package/archive checks on the candidate.
- Install the pinned coverage tooling with
  `rustup component add llvm-tools-preview` and
  `cargo install cargo-llvm-cov --version 0.9.1 --locked`, then run
  `cargo llvm-cov --all-targets --locked --summary-only` and review the
  coverage summary before release. Inspect gaps in security-sensitive, setup,
  MCP, telemetry, and evaluation paths; record material gaps with the release
  checks. CI runs this command on Linux and macOS, then checks the JSON report
  with `python3 -B scripts/check-eval-mcp-coverage.py`. That guard requires 95%
  line coverage for each of the eval/OTel and MCP/OTel groups. Release policy
  requires whole-project line coverage strictly greater than 90% and each group at
  least 95%. Check the underlying counts, not just rounded display values.
  The current CI guard enforces the two group thresholds; separately verify
  the whole-project threshold before accepting a release.
- Serialize heavyweight coverage and timed integration suites on a shared host.
  Give each build a separate `CARGO_TARGET_DIR` outside every repository and
  coordinate suite ownership across agents. Separate target directories avoid
  build locks, but do not prevent CPU contention from invalidating timed runs.
  Keep prompts, logs, reports, coverage exports, and traces outside repositories.
- Use hermetic stubs for harness executables when tests only need executable discovery
  or version output. Stubs must not contain credentials, contact providers, or
  invoke models.
- Add release-branch triggers so the candidate receives the same checks as
  `main`.
- Gate live integrations explicitly. A cmux probe may run only when the runner
  provides a reachable cmux instance; otherwise emit a visible notice and skip
  it. Never turn a missing optional terminal integration into a false pass.
- Inspect security scans. Treat production findings as release blockers. A
  finding in a deterministic fixture or test-only diagnostic may be dismissed
  only after reviewing its exact location and recording why it cannot affect
  shipped behavior.

For live compatibility claims, record a separate evidence row for each harness
version, exact model, and execution mode (interactive or headless). Each row
needs the actual MCP tool call and result, observed OTel export at the collector,
and the task/attempt and ahu revision that produced them. Record missing
observations, telemetry that is off, native refusals, execution failures, and
successful observations separately. A CLI listing,
MCP configuration, or tool discovery is not an MCP invocation; configured
telemetry is not proof of export. Native admission refusal verifies a refusal,
not successful execution. Interactive success does not establish headless
success. Keep native controls intact and report blocked modes; do not change
accounts, hooks, plugins, or approvals to manufacture a pass. Fixture tests
verify their synthetic protocol behavior, not live provider compatibility.
Skill presence or discovery proves availability only; distinguish observed
invocation and completion from claims that its instructions were followed.

After local checks pass, push the candidate branch only when authorized and
watch every required hosted check to completion, confirming each check's SHA
matches the candidate. Fix failures on the candidate and repeat the checks.
Use the repository's configured change-review process
for the exact candidate commit and scope, and merge only after required checks
are green. This may be a pull/merge request or a documented local review when
no hosted review provider is configured.
Fetch the protected release branch, verify the merge commit, version, and its
relationship to the reviewed candidate. If merging changes the validated tree,
repeat the required validation on that tree before tagging; satisfy any required
merge-commit checks as well. Then create an annotated `vX.Y.Z` tag pointing at
that merge commit. A completed release must push that tag; use the publication
authorization established earlier for this operation. Verify
the remote tag resolves to the intended commit.

If local installation is authorized, install from the intended release commit
or verified release artifact, then resolve the executable actually used by the
shell and check its version. Record both the installed path/version and source
commit or artifact identity. A candidate binary in a build directory does not
prove the installed `ahu` was replaced. Report a mismatch or unavailable install
permission separately from publication state; do not modify native user accounts
or harness configuration as part of this check.

Before handoff, reconcile the ahu tasks used to implement this release. Run
`ahu tasks --all` from the repository and use provider-side task links, branch
names, and commits to identify release tasks; do not guess from agent names or
task titles alone. Inventory the exact owned task IDs, worktrees, branches, and
recorded cmux workspace/group IDs before cleanup. Confirm each task is terminal,
its result was reviewed, and its changes are integrated. For each completed
headless release task, run `ahu cleanup <task>` and then `ahu remove <task>` from
another checkout only when its worktree is clean and its branch is merged into
the primary checkout's current HEAD (or already absent). A cherry-pick alone
may not satisfy that ancestry check. Do not bypass a refusal by deleting work.
Interactive tasks have no headless captures; run `ahu remove <task>` directly
after the same checks.

`ahu remove` checks the recorded cmux workspace's group/window ownership and
closes that workspace when present; it refuses an unverifiable relationship.
It does not generally remove repository group anchors or native harness session
stores. Empty fixture anchors can remain after task removal. Inspect remaining
groups and their members against the inventory before separately closing an
anchor: it must belong to this release's disposable fixture, contain no needed
session, and have no unrelated members. Never close a group based on its title
or apparent emptiness alone. Preserve unrelated sessions and anything whose
ownership, terminal state, or integration is uncertain. Verify the owned tasks
are absent from `ahu tasks --all`, their worktrees/branches and recorded cmux
workspaces are gone, and unrelated sessions remain. Report partial cleanup and
retained anchors with reasons; retain provider-side work items and acceptance
evidence according to project policy.

Keep public release files free of private tracker material, local paths,
credentials, execution traces, ignored state, and device-specific
configuration. Keep tracker details and release findings in a verified private
record when repository policy uses a private tracker. If it does not, do not
invent one. Report skipped checks, unresolved findings, publication state, and
the exact commits and tag in the final handoff; never claim a hosted or
published result that was not verified.

The final release summary must include:

- version, candidate branch, change-review reference (or documented local
  review disposition), merge commit, and tag reference;
- local and hosted checks, including skipped optional integrations and why;
- security-scan findings and any reviewed test-only dismissals;
- publication state and links verified after the tag push;
- per-harness/mode MCP and OTel evidence, including native refusals and unknowns;
- installed executable path/version and provenance when installation was authorized;
- release-related ahu tasks cleaned up, and any preserved tasks with reasons;
- follow-up work intentionally left outside the release.

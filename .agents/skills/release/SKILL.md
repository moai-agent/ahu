---
name: release
description: Prepare, verify, merge, and tag a repository release candidate with hermetic CI and explicit publication gates; use with Codex or OpenCode, never Claude Code.
---

Use this skill when a maintainer wants to turn the current repository state into
a release candidate, get its hosted checks green, merge it, and publish a tag.
It covers release mechanics and verification; it does not decide what product
scope belongs in a release.

Treat roadmap records, work items, milestones, and release objects as a
provider-neutral planning layer. A project may map them to GitHub issues and
milestones, another tracker, or an MCP task service. Discover the active
provider's fields and visibility at runtime; do not hardcode issue numbers,
repository names, label sets, milestone IDs, project IDs, or a requirement that
one work item maps to exactly one pull request.

This skill must not run under Claude Code. If the active harness is Claude Code
or identifies itself as `claude`/`claude-code`, stop before making changes or
external mutations and report that the maintainer must rerun the workflow with
Codex or OpenCode.

Read the repository's `AGENTS.md` and the current release notes before changing
anything. Start from a clean checkout and record the exact base commit. Keep
release work on `release/vX.Y.Z` (or the repository's documented equivalent),
and make a separate post-release branch from updated `main` for improvements to
this workflow or its skill. Do not mix that follow-up work into the release PR.

Before external mutations, require explicit authorization for each operation
that needs it: pushing a branch, merging a pull request, creating or pushing a
tag, and publishing artifacts. A request to prepare or test a release does not
authorize publication by itself. Never push a working branch, force-push, or
rewrite an existing tag.

Make CI prove the release boundary without paid services:

- Run formatting, lint, unit and integration tests, documentation and knowledge
  checks, dependency policy, and package/archive checks on the candidate.
- Use hermetic stubs for harness CLIs when tests only need executable discovery
  or version output. Stubs must not contain credentials, contact providers, or
  invoke models.
- Add release-branch triggers so the candidate receives the same checks as
  `main`.
- Gate live integrations explicitly. A CMUX probe may run only when the runner
  provides a reachable CMUX instance; otherwise emit a visible notice and skip
  it. Never turn a missing optional terminal integration into a false pass.
- Inspect security scans. Treat production findings as release blockers. A
  finding in a deterministic fixture or test-only diagnostic may be dismissed
  only after reviewing its exact location and recording why it cannot affect
  shipped behavior.

After local checks pass, push the candidate branch only when authorized and
watch every required hosted check to completion. Fix failures on the candidate
and repeat the checks. Open the provider's change-review object with the exact
candidate commit and scope, and merge only after required checks are green.
Fetch the protected release branch, verify the merge commit and version, then
create an annotated `vX.Y.Z` tag pointing at that merge commit. A completed
release must push that tag; obtain explicit
authorization before the push if it is not already part of the request. Verify
the remote tag resolves to the intended commit.

Keep public release files free of private tracker material, local paths,
credentials, execution traces, ignored state, and device-specific
configuration. Keep tracker details and release findings in a verified private
record. Report skipped checks, unresolved findings, publication state, and
the exact commits and tag in the final handoff; never claim a hosted or
published result that was not verified.

The final release summary must include:

- version, candidate branch, change-review reference, merge commit, and tag
  reference;
- local and hosted checks, including skipped optional integrations and why;
- security-scan findings and any reviewed test-only dismissals;
- publication state and links verified after the tag push; and
- follow-up work intentionally left outside the release.

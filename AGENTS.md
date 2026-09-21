# Repository collaboration instructions

When providing a patch for the user to apply, always include complete,
copy-pasteable shell commands. Include the repository directory, the exact patch
path, a validation command, and the apply command. If committing or merging is
part of the authorized task, include those commands with explicit file paths and
a commit message as well. A patch link alone is not sufficient.

Never include or execute a remote push without the user's explicit permission.

## Issue completion is part of the work

These rules apply to every agent working in this repository: every agent
registered under `.agents/ahu/agents/` (run `ahu agents` for the current set) and
any coordinating agent.

For work assigned through issues, the user authorizes agents to comment on the
associated issues and close those resolved by the work. Do this as part of the
task, without asking for separate permission or leaving it for the user.

- Read each assigned issue and its acceptance criteria before implementation.
  Track the associated tasks and parent stories through completion.
- Before the final handoff, post a substantive comment on each associated issue
  describing what changed, the validation actually performed and its results,
  skipped checks, remaining limitations, and the issue's disposition. State
  whether changes are uncommitted, committed locally, merged, or published;
  reference a commit or pull request only when it exists and is accessible to
  the intended readers. Do not claim local changes are available remotely.
- Close resolved issues after their acceptance criteria and required delivery
  steps are satisfied. If required tests, review, applying a patch, or
  merging remain outstanding, comment on that status and leave the issue open.
  Do not close an issue merely because an agent finished its assignment.
- For partial, blocked, or unreproduced work, comment with the evidence, remaining
  work, and blocker, and leave the issue open. A review that discovers a defect
  does not resolve the defect: keep the remediation issue open.
- Close a parent story or epic only after checking all its acceptance
  criteria and required child work. Completing one child does not close the
  parent. Keep unrelated issues out of the update.
- When delegating, assign responsibility for issue updates explicitly. The
  coordinating agent verifies the comments and final issue states and completes
  any missing updates, avoiding duplicate comments from multiple agents.
- Verify each comment and state change through the tracker before reporting
  success. If access or a write fails, report the tracking blocker and the
  affected issues in the private handoff; do not silently omit tracking work.
- Keep private roadmap contents, finding-to-fix mappings, and private references
  in the private tracker and user conversation. Verify the destination's
  visibility before writing; a private project does not make a linked public
  issue private. Never copy private context into public repository artifacts or
  use local ignored reports as the durable handoff.

Issue comments and closure do not authorize a Git push. The final response must
state which associated issues were closed and which remain open, with reasons.

## Repository privacy boundary

Do not store execution traces anywhere in this repository, including ignored paths.
Do not put private tracker-derived content in public files without user confirmation
of the exact material and destination. This includes tracker locations, IDs,
references, issue titles and bodies, and private project metadata. The issue-type
vocabulary in discover-requirements is explicitly authorized for public skill and
agent documentation; that authorization does not extend to tracker content.
Verify both project and linked issue repository visibility before tracker writes.
Keep discovery records, skill evolution proposals, and rejected evolution records
private, never in repository logs or docs/skill-evolution. Setup of a skill does
not authorize conducting a discovery session or creating live discovery records.

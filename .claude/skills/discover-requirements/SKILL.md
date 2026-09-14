---
name: discover-requirements
description: Clarify ambiguous requirements into agreed outcomes, scope, and acceptance criteria in a private tracker. Use for fuzzy requests or explicit requirements discovery; do not interview users whose tasks are already specified.
---

# Discover requirements

Read the request, existing decisions, relevant code, tests, and observable behavior
before asking questions. Gather accessible facts yourself within the authorized
scope. Separate verified facts, assumptions, proposals, and unresolved questions.
Preserve settled choices and existing authorization; discovery does not require
reconfirming an already specified task or suspend authorized independent work.

Ask only questions that materially affect the outcome. Offer a recommendation and
concrete alternatives in the user's preferred channel, with tradeoffs they can
react to. Explore the affected user, problem, current workaround, scope, constraints,
and observable success as needed, without a mandatory questionnaire. Never invent
human experiences, testimony, quotations, or agreement. Keep unavailable evidence
explicitly unknown and identify who can resolve it.

## Private record and tracker access

The private tracker is the durable record for discovery, decisions, acceptance
criteria, and parent/child relationships. Obtain its location from the user or
coordinator at runtime. Use authenticated `gh api graphql` metadata discovery to
resolve the project, repositories, fields, options, and record identifiers; never
hardcode IDs. Verify both project visibility and each linked issue repository's
visibility before writing. A private project can contain public issues. Treat
unknown visibility as a blocker to that write and report it privately.

Reuse relevant records before creating new ones. The custom project single-select
field **Issue Type** uses the following vocabulary:

| Value | Use |
| --- | --- |
| Idea | Exploration with an unresolved outcome or scope |
| Epic | Work spanning multiple stories |
| Story | A user outcome with acceptance criteria |
| Task | Bounded implementation work |
| Bug | An observed defect |

Native GitHub organization issue types are **Task**, **Bug**, and **Feature**;
they are separate from the custom project field. Discover current metadata for
each before choosing or writing a value. Do not mechanically create every level
or change field definitions to fit this table.

Carry the agreed outcome, scope and non-goals, constraints, testable acceptance
criteria, open questions, and necessary relationships into the appropriate existing
private records. Preserve AGENTS.md authorization and completion rules, including
explicit coordinator ownership of tracking. Verify writes through the tracker.
If access is unavailable, report the blocker in the private conversation; do not
create a public fallback. Discovery completion does not complete implementation:
keep delivery work open until its acceptance criteria and required checks are met.

## Public context and maintenance

Do not store execution traces anywhere in this repository, including ignored
paths. Keep raw evidence, interviews, private history, rejection memory, and
finding-to-fix mappings private. Before publishing any private tracker-derived
content or references, obtain user confirmation of the exact material and its
public destination. Existing setup or implementation authorization does not grant
that publication permission. Public skill and agent prose may use the issue-type
vocabulary above; that authorization does not cover tracker locations, IDs, issue
titles or bodies, or private project status/version fields.
Public knowledge describes current code using public source evidence; ordinary
README and reference documentation stays outside the OKF bundle.

This skill does not mutate itself. Propose small changes separately when observed
use supplies evidence of a concrete improvement. Keep that evidence and rejected
proposals in the private tracker. Promotion requires coordinator review and
independent evaluation. Before evaluating, agree on baseline and candidate cases
and a success criterion. Include privacy and scope regression checks; simulation
alone does not demonstrate improvement in real use. Keep evaluation records private.
The documentation agents own authored skill prose and current knowledge; the
development agents own code and tests and hand prose needs to their documentation
counterpart on the same harness. Preserve existing delegation and approval
boundaries.

Maintain the canonical `.agents/skills/discover-requirements/SKILL.md` and the
repo-local `.claude/skills/discover-requirements/SKILL.md` as identical regular
files through a separately authorized edit; no symlinks, global sync, or installs.

After skill edits, run `python3 scripts/check-skills.py` to check the repository
copies byte-for-byte. Keep all metadata identical in both copies.

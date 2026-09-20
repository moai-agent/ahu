---
name: context-hygiene
description: Interpret `ahu inventory` and `ahu hygiene` output and decide what to do about a skill, memory, or instruction source that can influence an agent. Use when reading a hygiene review, a context inventory, or a control key such as repository-edit or harness-setting; do not use it to justify deleting a source ahu did not inspect.
---

# Context hygiene

`ahu inventory` and `ahu hygiene` report facts: which sources ahu found, their
category and scope, whether they are shared, what ahu can digest, and what ahu
cannot see. They do not recommend anything and they change nothing. This skill
is where the recommendations and the interpretation live.

ahu never deletes, disables, purges, prunes, stages, or commits on its own, and
nothing in this skill authorizes an agent to do so either. A cleanup is a change
the operator reviews and makes.

## Reading the visibility labels

| Label | What it means |
| --- | --- |
| `loaded` | ahu passed it to the harness, or the harness reports it loaded |
| `available` | discoverable by the harness; whether it reached the model is not observable from outside the session |
| `disabled` | present but turned off through a native control |
| `opaque` | a source of this kind exists and ahu cannot read its contents |
| `absent` | nothing found |

`available` is the common case and the one most often misread. It is not
evidence that the content reached the model, and `absent` for an unscanned area
is not evidence of nothing being there. Read the "What ahu cannot see" section
before drawing a conclusion about coverage; the inventory is never complete.

## Control keys

A hygiene review tags each source with a control key. The key says what kind of
operation is available, not that the operation should be performed.

| Key | What it means | Recommended handling |
| --- | --- | --- |
| `inventory-listing` | ahu can list every skill and memory source it sees, with scope and sharing | Start here; establish the facts before proposing a change |
| `cleanup-preview` | an exact cleanup scope can be previewed before anything changes | Preview and confirm the scope with the operator first |
| `repository-edit` | the source is repository-scoped | Remove, move, or edit it as an ordinary Git change the operator reviews and commits |
| `harness-setting` | the source is shared beyond this project | Detach it from this project through the harness's own settings; do not delete a machine-wide source to solve a project-local problem |
| `skill-disable` | disabling one skill for one agent | Not offered per agent: the available setting applies more widely than the agent |
| `memory-toggle` | switching memory reading or writing per agent | Not observable or switchable per agent from outside a session; do not report "memory off" |
| `shared-source-clear` | clearing a source shared with other agents or projects | Not offered per agent; never present a global deletion as a local one |
| `none` | no per-agent control for this source | Say so plainly and name the scope that does control it |

Keys listed as unsupported are absent capabilities, not failures. Report them as
what this harness does not expose rather than working around them.

## Deciding what to clean up

Prefer the narrowest change that addresses the observed problem. A source that
is merely present is not evidence of harm; name the behavior you observed and
the source you believe caused it. Leave a source in place when the evidence is
only that it exists.

Repository-scoped sources are the ordinary case: propose the edit, show the
scope, and let the operator review and commit. Personal, machine, managed, and
provider-scoped sources belong to a wider boundary; recommend detaching them
from the project rather than removing them, and say which other projects or
agents the change would reach.

Hooks are executable code that the harness runs on its own events. Treat a hook
as a higher-impact change than a skill or an instruction file, and never propose
one as routine tidying.

## After a change

Editing a file does not remove content already loaded into a running session, a
compaction summary, or the harness's caches. Start a fresh task after a change
and rerun `ahu inventory` before treating the new session as clean. A review
recorded in `ahu`'s state is a timestamp, not evidence that anything was cleaned.

The review cadence is a project setting (`context_hygiene.review_interval_days`
in the project configuration). There is no personal interval and no permanent
personal dismissal; an overdue review runs on the next load.

## Scope of this skill

This skill covers reading and acting on context inventory and hygiene facts. It
does not authorize tracker writes, pushes, or publishing private tracker content.
Keep discovery records and finding-to-fix mappings private, per AGENTS.md.

Maintain `.agents/skills/context-hygiene/SKILL.md` as the canonical repository
skill file. Claude loads the same `.agents/skills/` tree directly; do not create
a duplicate `.claude/skills/` copy, symlink, global sync, or install.

After skill edits, run `python3 scripts/check-skills.py` to validate the
canonical repository skill tree.

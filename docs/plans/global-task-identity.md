---
type: Architecture
title: Global task identity
description: Plan for globally unique task IDs, cross-checkout resolution, and task communication.
tags: [tasks, identity, planning]
status: draft
sources:
  - id: task
    resource: ../../src/task.rs
    title: Task states, record schema, and ID generation
  - id: commands
    resource: ../../src/commands.rs
    title: ID resolution and task commands
  - id: launch
    resource: ../../src/launch.rs
    title: Task planning and runtime checks
  - id: state
    resource: ../../src/state.rs
    title: State paths and confinement
  - id: headless
    resource: ../../src/headless.rs
    title: Headless runtime store
  - id: broker
    resource: ../../src/broker.rs
    title: Broker owner-attempt binding
  - id: orchestration
    resource: ../../src/orchestration.rs
    title: Nonce and prompt assembly
  - id: reference
    resource: ../../docs/reference.md
    title: State and compatibility rules
---

# Global task identity

Status: plan only. Nothing described here is implemented, migrated, or tested in
this change. This document is the developer-ready handoff for a future
implementation task.

## Problem

Task discovery is per-repository. `ahu focus <task-id>` run from a different
checkout of the same repository, let alone from an unrelated checkout, prints `no task matching "<id>"` and exits 2 (`src/commands.rs:884`), because
`focus` resolves IDs only against the invoking checkout's task listing
(`src/commands.rs:858-884`). Task IDs are 18-hex local names
(`src/task.rs:214-227`), not global identities, so no other invocation can name
a task unambiguously.

## Verified current state

All statements below were read from the source on branch
`ahu/arch-glm/006aaeb5f72d07dff8` (HEAD 51786d5), 2026-09-19.

- **Task states** (`src/task.rs:36-42`): `Starting`, `Running`, `Exited`,
  `Failed`, `Cancelled`. The vocabulary has **no `Completed` state**. Doc comment
  (`src/task.rs:30-33`): `Exited` means the harness process ended and is *not*
  a claim of success; `Cancelled` is likewise terminal and not a success claim.
  This corrects the assignment brief, which described a `Completed` state.
- **ID generation** (`new_task_id`, `src/task.rs:214-227`): 18 hex chars =
  10 hex seconds + 4 hex fractional-second bits + 4 hex (`pid ^ counter`). No operating
  system entropy source is used. Explicitly documented as time-ordered and
  collision-resistant for concurrent launches of the same agent.
- **Record schema**: `TASK_SCHEMA_VERSION = 2` (`src/task.rs:208`); schema-1
  records are refused by version on load (the split of `source_digest` from
  `instructions_digest` makes cross-schema reading unsafe, `src/task.rs:203-207`).
  `TaskRecord.task_id` is a stored field (`src/task.rs:133-135`).
- **Naming couplings**: the worktree directory name *is* the task ID
  (`scan_worktrees`, `src/task.rs:708`, worktrees under
  `<primary>/.worktrees/<task_id>`), the branch is
  `ahu/<agent-segment>/<task_id>` (`src/launch.rs:223`), and `remove` looks up
  branches by pattern `ahu/*/<task_id>` (`src/commands.rs:650`). `drift.rs:48`
  persists `previous_task_id`. Changing ID format therefore touches worktree
  naming, branch naming, drift records, and cmux group mapping.
- **Store layout**: interactive tasks store `task.json` + `prompt.txt` under
  `<task-worktree>/.ahu/state/repos/<repo-identity>/tasks/<task-id>/`
  (`src/state.rs:173-187`). The **owner rule**: each managed worktree store
  accepts only its owner's task ID, matching repository identity and canonical
  worktree path, checked on every scan (`docs/reference.md:812-816`).
  Headless tasks use a separate runtime store selected by `AHU_RUNTIME_DIR` or
  defaulting to `~/.local/state/ahu/runtime` (`runtime_root()`,
  `src/headless.rs:289`; task.json read at `src/headless.rs:510,552`; written
  by `src/launch.rs:752`).
- **Resolution semantics**: `inspect_task` (`src/commands.rs:691-722`) tries
  exact IDs before unique prefixes and errors `no task matching {id:?}` listing
  candidates on ambiguity. `focus` (`src/commands.rs:858-884`) matches by
  equality-or-prefix and takes the **first** match with no ambiguity check, a
  subtle inconsistency the implementation should reconcile (make `focus` use
  `inspect_task`'s ambiguity discipline).
- **Legacy and failure rules** (`docs/reference.md:818-838`): legacy records
  stay readable from the primary checkout, an invoking plain checkout, or an
  explicit external store; a managed worktree's owned record wins duplicates;
  misplaced child records are reported, not migrated; unreadable or
  refused-owner rows remain visible for inspection; worktrees without records
  are reported incomplete; listing never deletes or vouches for anything.
- **Entropy helper** (`new_nonce`, `src/orchestration.rs:162`): an
  opportunistic 16-byte `/dev/urandom` read (failure ignored) mixed with
  nanosecond timestamps, the process ID, and a counter. Call sites:
  `src/broker.rs:104,343`, `src/orchestration.rs:276`,
  `src/commands.rs:1683`, `src/headless.rs:524`.
- **Broker binding** (`src/broker.rs`): registered-child dispatch binds
  capability to one live owner attempt via a `parent_task` check;
  `AHU_BROKER_TOKEN` gates broker access.
- **Doc placement**: no design-doc or plans convention existed in this
  repository. `docs/` contains `dependencies.md`, `knowledge/` (OKF v0.2
  invariants; execution traces and plans are explicitly not allowed there),
  `reference.md`, and `releases/`. This document therefore introduces
  `docs/plans/` as an OKF v0.2 bundle for planning handoffs, configured
  alongside `docs/knowledge` in `.agents/ahu/config.toml`; `docs/knowledge/`
  was ruled out by its own charter.

## External reference points (verified 2026-09-19)

**MCP Tasks extension** (specification proposal 2663, Final; primary docs:
modelcontextprotocol.io/extensions/tasks/overview, specification repository
github.com/modelcontextprotocol/ext-tasks):

- `CreateTaskResult` carries `resultType: "task"`, `taskId`, initial status,
  `ttlMs`, `pollIntervalMs`; the task must be **durable before the response is
  sent**.
- Methods: `tasks/get`, `tasks/update`,   `tasks/cancel`. No `tasks/list` method exists; `tasks/result` (from an earlier
  draft) was removed.
- Statuses: `working`, `input_required`, `completed`, `failed`, `cancelled`.
- Client capability `io.modelcontextprotocol/tasks`; **the server is the sole
  decider of task completion**; polling is the default consumption model;
  optional notifications exist (`notifications/tasks` via
  `subscriptions/listen`).

**Harness convergence scan** (2026-09-19, from the assignment): Claude Code
2.1.278 (pre-redesign draft), Codex 0.155.1 (final shape), OpenCode 1.18.31
(older core draft), Antigravity 1.2.7 (nothing). The direction is shared
vocabulary (opaque server-issued IDs, durable task records, status polling,
no list method), but no single wire format is stable enough to couple to.

**Position (per the assignment, and agreed here)**: the near-term target is
ahu's own CLI and on-disk model adopting the Tasks vocabulary and durability
rules; an MCP server surface is deferred and decided separately. Rationale: the
durable properties that matter (records durable before the launch response,
opaque globally unique IDs, poll-style reads, sole-decider status semantics)
map onto on-disk records plus the existing `ahu tasks`/`ahu task` surface
without adding an MCP client-capability negotiation and server lifecycle that
nothing in this repository consumes yet. Counter-case for the record: if ahu
must work with MCP clients in the near term, building the wire surface early
would force the honest status mapping (`completed` vs `Exited`) that this plan
deliberately defers; revisit when a concrete consumer exists.

## Decision points

Each item lists the compared alternatives and a recommendation for the
implementation task to confirm or overturn.

### 1. Where global resolution lives

- **A. Scan known checkouts at lookup time.** No registry; on lookup, walk
  candidate repositories under `$HOME`. Rejected: unbounded filesystem
  scanning is invasive, slow, and has no principled candidate set.
- **B. Launch-registered index (recommended).** ahu controls every launch, so
  it can register each new task in a small user-state index, for example
  `~/.local/state/ahu/task-index/<uuid>`, storing only: the task's Universally
  Unique Identifier (UUID), owning checkout root path, repository identity,
  store kind (interactive worktree / headless runtime). `ahu focus
  ahu:task:<uuid>` (and `task`, `diff`, `remove`) consult the index to locate
  the store, then run the **existing** read logic, including the owner rule,
  against it. The index is a pointer, never an authority: entries are
  validated on use, so a stale or hostile index can only name candidate
  stores that must still pass the normal checks. A dead entry (checkout
  removed) reports the recorded location and fails like today's
  incomplete-worktree case.
- **C. Repo-local stores with global names only.** Keep stores exactly as-is
  and rely on the Uniform Resource Name (URN) grammar for naming. Rejected as
  the sole mechanism: it does not fix the pain point, since resolution still
  cannot cross checkouts.

Privacy constraint (from `AGENTS.md` and the assignment): the index must
contain only pointers (no task titles, prompt text, or branch names) and must
live in user state space, never inside any repository, including ignored
paths.

### 2. Disk layout vs owner rule

The assignment's on-disk layout is `tasks/<uuid>`. Interpretation adopted
here: the task directory *within each store* becomes `<uuid>`; the
  `repos/<repo-identity>/` level that holds `tasks/` **survives**, and records
  remain worktree-local. The owner rule (repository identity + canonical
  worktree path
checked on every scan) is unchanged, removing a worktree still removes its
state, and every existing compatibility rule in `docs/reference.md:797-873`
keeps its current meaning. The global index from decision 1 is the only new
global artifact.

The alternative (a true global store, for example `~/.local/state/ahu/tasks/<uuid>/`)
was considered and rejected for now: records would leave the worktree, the
owner rule would need redesign (worktree removal would no longer remove
state, or would need explicit index/store cleanup), the legacy and rollback
rules would all need rework, and the migration would be far larger. If the
implementation finds the pointer index insufficient, this alternative should
be re-proposed with those costs stated.

### 3. Identifier version and entropy

- **v7 (recommended)**: time-ordered, matching the current ID's documented
  time-ordering intent (`src/task.rs:210-213`) and preserving listing order;
  74 random bits from the operating system.
- **v4**: fully random, no ordering. Available if time-ordering is deemed
  non-essential, but it discards an existing property for no gain.

**Entropy posture: prerequisite relationship.** Branch
`ahu/defsec-glm/006aaeb2f5bc6882d7`, commit 23f1fe3 (refuses to launch when
fence-nonce entropy is unavailable) touches `src/orchestration.rs` (111 lines)
plus `src/broker.rs`, `src/commands.rs`, `src/headless.rs`, and
`tests/delegation.rs`, and is **not merged into this branch's HEAD**. That fix
replaces the opportunistic `new_nonce()` entropy with fail-closed behavior.
Today's `new_task_id()` does **not** use `new_nonce()` and has no OS-entropy
input at all. Recommendation: the implementation's identifier generation must
adopt the same fail-closed posture: random bits come from the OS entropy
source, and launch **refuses** when they are unavailable. This stays consistent
with that fix without re-planning it here. If 23f1fe3 is not yet merged when
implementation starts, coordinate the shared helper rather than duplicating it;
the implementation depends on the fix's *posture*, not its commits.

### 4. Identifier grammar, normalization, and CLI acceptance

- Canonical form: `ahu:task:<uuid>`, lowercase, canonical 8-4-4-4-12
  hyphenation. The grammar is fixed-scheme; there is no `urn:` prefix variant.
- Acceptance forms, all normalized before matching: the full URN; the bare
  UUID (case-insensitive input, normalized to lowercase); a unique prefix of
  the UUID including its hyphens; and, during and after migration, the legacy
  18-hex form for records created before the change.
- **Ambiguity must error, naming candidates, never guess.** This is
  `inspect_task`'s existing discipline (`src/commands.rs:691-722`); the
  implementation must also fix `focus`'s first-match behavior
  (`src/commands.rs:858-884`) to use the same discipline.
- Record storage: store the full URN in `TaskRecord.task_id` (self-describing,
  matches "task IDs become `ahu:task:<uuid>`"); the directory name is the bare
  UUID (path-safe; colons are not portable in paths). The equivalence rule is
  explicit: directory `<uuid>` and record field `ahu:task:<uuid>` are the same
  identity. If the implementation prefers a bare UUID in the record field,
  that is acceptable as long as every CLI surface accepts and normalizes both
  spellings; pick one and state it in the implementation.

### 5. Authorization, because Task IDs are not capabilities

Task IDs already appear in branch names, worktree directory names, and user
commands; they are not secrets, and global resolution makes them more
guessable in principle (v7 embeds a timestamp). The existing authorization
boundary is the broker's owner-attempt binding (`parent_task` check plus
`AHU_BROKER_TOKEN`, `src/broker.rs`). Constraints for the implementation:

- Knowing a task ID must not grant any write. All reads through the global
  index re-run the owner rule; all writes (status changes, removal, inbox
  delivery) must be performed by the owning ahu process or a broker-registered
  child bound to that task.
- The index itself is pointer-only and must never be treated as evidence;
  unreadable or mismatched index entries degrade to the existing
  unreadable/incomplete reporting rather than granting acceptance.
- `remove` keeps its gated, terminal-only semantics (`src/commands.rs:912`).

### 6. Communicating through tasks

Adopt the Tasks-extension shape at the CLI/on-disk level:

- **Status reads**: `ahu task <id>` already exists; extend it to resolve
  globally (decision 1) and to display the new artifacts below. Polling is the
  model; no push mechanism is planned.
- **Result artifact**: a single `result.md` in the task directory, written by
  the working agent, read via   `ahu task <id>` (or an `ahu result <id>` command; naming is an open
  question). Bounds: display refuses results larger than
  a fixed cap (suggest 1 MiB) with an explanatory note rather than truncating
  silently. Confinement: the artifact lives in the task directory, so the
  existing path rules apply; there is no cross-task channel.
- **Operator-to-agent inbox**: an append-only `inbox/` in the task directory.
  Operator writes via an ahu command (naming is an open question); ahu numbers
  and bounds entries (suggest: refuse beyond 100 entries or 1 MiB total; no
  unbounded message log). The agent reads but never rewrites inbox entries.
  Delivery is by the owning ahu process or a broker-bound child (decision 5);
  a bare task ID is never sufficient to deliver.
- **`input_required` analog**: recommendation: implement it as a **separate
  signal**, for example a bounded `question.md` the agent writes, surfaced by
  `ahu tasks`/`ahu task <id>`, **not** as a new `TaskState` variant.
  `TaskState` is process-observation semantics (`is_live()`,
  `src/task.rs:58-60` drives cancellation validity); mixing a content-level
  "needs input" into it would entangle cancellation with a communication
  signal. The alternative (a real `input_required` state) is listed for the
  implementation to weigh; if adopted, `is_live()` semantics and cancel
  behavior must be specified for it first.

### 7. Status vocabulary mapping

ahu keeps its five states. Mapping against the Tasks extension:

| ahu state | MCP Tasks status | Note |
|---|---|---|
| `Starting` | `working` | |
| `Running` | `working` | |
| `Exited` | **no honest mapping** | Terminal, not a success claim |
| `Failed` | `failed` | |
| `Cancelled` | `cancelled` | |

ahu must **not** gain a `Completed` state in this work: `Exited` exists
precisely because ahu observes process exit and refuses to claim the work
succeeded (`src/task.rs:30-33`). Collapsing `Exited` into MCP's `completed`
would be dishonest, and into `failed` likewise. This is the concrete reason
the MCP server surface is deferred (see the Position paragraph earlier): when
a consumer exists, the mapping of `Exited` (for example, expose exit
observation plus artifacts and let the client decide) must be designed
against that consumer, not
abstractly. Nothing in the CLI/on-disk work requires the mapping.

### 8. Migration and compatibility

- **Coexistence, not renaming (recommended).** Each new task gets a UUID;
  existing records, worktree directory names, branches (`ahu/*/<18-hex>`), and drift
  references keep their 18-hex identities. IDs are immutable identity;
  renaming worktree directories and branches would break cmux group mapping,
  launch locks, `previous_task_id` links, and human references. Both forms
  are accepted everywhere (decision 4) for as long as legacy records exist.
- **In-flight tasks at upgrade**: continue under their old IDs; no live-record
  rewrite. The upgrade boundary is launch time.
- **Schema**: bump `TASK_SCHEMA_VERSION` (2 → 3) so new-field/ID-format rules
  are version-gated; schema-2 records remain readable under the existing
  legacy rules (`docs/reference.md:818-822`). The schema-1 refusal lesson
  (`src/task.rs:203-207`) applies: refuse cross-schema interpretation rather
  than tolerating it.
- **Stores**: the change applies to interactive worktree stores, managed-store
  scans, and the headless runtime store alike (`AHU_RUNTIME_DIR` /
  `~/.local/state/ahu/runtime`), since they share the ID vocabulary.
- **Unreadable records**: keep today's behavior: retain, report, never
  delete (`docs/reference.md:824-831`). A new-format record that is unreadable
  degrades identically; a corrupt index entry never hides a task.
- **`focus` consistency**: the per-repo vs global split disappears for IDs
  that the index can resolve; `focus` remains interactive-cmux-scoped for
  what it *attaches to*, but its ID resolution becomes global.

### 9. Interaction with planned prompt tags

Noted only, per the assignment: a separate effort plans XML-shaped prompt
tags and a metadata section. The task identity URN is a natural resident of
that metadata section (a launched agent should be told its own global task
ID). No design is done here; the implementation should coordinate so the URN
spelling this document fixes (`ahu:task:<uuid>`) is the one the prompt
metadata carries.

## OKF and documentation strategy

The implementation change must keep the documentation set coherent:

- **New `docs/knowledge/` concepts.** *Task identity*: the `ahu:task:<uuid>`
  grammar, acceptance and normalization forms, the pointer-only index and its
  privacy constraint, and IDs-are-not-capabilities authorization.
  *Task communication*: the `result.md` artifact and its size cap, the
  append-only `inbox/`, and the `question.md` signal.
- **Updated `docs/knowledge/task-state.md`.** UUID task directories, the
  schema 3 version gate, and the coexistence of legacy 18-hex records.
- **Updated `docs/reference.md`.** The "State and compatibility" section
  gains the index and migration rules (criterion 7).
- **Plans-bundle lifecycle.** Implementing this plan removes this file from
  `docs/plans/` and adjusts the configured bundles in
  `.agents/ahu/config.toml` so no configured bundle lacks concept files;
  `docs/knowledge/` then records the resulting invariants.
- **Validation path.** Per bundle: `okf validate` then `okf lint` (lint
  suppresses error findings, so validate must run first). Repository: `ahu
  knowledge lint`, `scripts/lint-prose.sh`, `python3 scripts/check-skills.py`.
  The `okf` binary is now listed in README.md's Install requirements, so
  `ahu knowledge lint` no longer depends on an undocumented tool.

## Acceptance criteria for the implementation task

1. `ahu focus ahu:task:<uuid>` succeeds from any checkout on the machine,
   including checkouts unrelated to the task's repository, resolving via the
   pointer index and re-verifying the owner rule; a dead index entry reports
   the recorded location and fails safely.
2. New task IDs are UUID v7 with fail-closed OS entropy: launch refuses when
    entropy is unavailable (posture aligned with the fail-closed entropy fix,
    commit 23f1fe3).
3. Acceptance covers the full URN, bare UUID, unique prefix, and legacy
   18-hex forms; ambiguity errors name candidates; no first-match guessing in
   `focus`.
4. The owner rule, managed-store precedence, legacy readability, and
   unreadable/incomplete reporting are preserved as specified in
   `docs/reference.md:797-873`.
5. The pointer index contains only UUID, checkout path, repository identity,
   and store kind; no titles, prompts, or branch names; it lives outside every
   repository.
6. Result and inbox artifacts are size-bounded, confined to the task
   directory, and writable only via the owning process or broker-bound child.
7. `docs/reference.md` "State and compatibility" is updated in the same change.
8. Full CI validation passes: `cargo fmt --check`;
   `cargo clippy --all-targets --locked -- -D warnings`;
   `cargo test --locked` (the commands `.github/workflows/ci.yml` runs).
9. The OKF and documentation strategy described earlier is executed in the
   same change: the new and updated `docs/knowledge/` concepts land, this file
   is removed from `docs/plans/`, and the configured bundles are adjusted.

Tests to add: cross-checkout focus via index (live, dead-entry, hostile-entry
cases); entropy-unavailable launch refusal; acceptance-form normalization and
ambiguity errors; coexistence of legacy and UUID records in one listing;
inbox and result bounds.

## Open questions for the implementer (decided defaults in parentheses)

1. Record field spelling: full URN vs bare UUID (full URN).
2. `input_required` analog: signal file vs state variant (signal file).
3. Global index registration: automatic on launch vs explicit command
   (automatic).
4. Command surface names for result/inbox/message operations (unresolved;
   pick at implementation).
5. Branch naming for UUID tasks: accept long `ahu/<agent>/<uuid>` names vs a
   short-name scheme with a stored mapping (accept long names; they are
   functional, and a mapping adds state to lose).
6. MCP server surface timing (deferred; revisit with a concrete consumer).

## Out of scope

Implementation, refactoring, record migration/renaming, any MCP wire surface,
GitHub issue actions, pushes, and the fail-closed entropy fix itself.
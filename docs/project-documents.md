# Shared project documents

ahu v0.11 writes shared project policy and context pins as OKF 0.2 Markdown
with YAML frontmatter. Native harness files keep their native formats.

| Path | Format and role |
| --- | --- |
| `.agents/ahu/config.md` | `type: ahu:project`; strict project settings in `ahu` |
| `.agents/ahu/lock.md` | `type: ahu:lock`; generated shared pins in `ahu` |
| `.agents/ahu/plugins/<id>.md` | `type: ahu:plugin-binding`; inert typed binding foundation |
| `.agents/ahu/config.toml`, `ahu.lock` | v0.11 bounded legacy readers; explicit migration only |
| `.ahu/migrations/project-okf-v1/journal.json` | Ignored local transaction, source digests, modes, repository/base identity and exact backups |
| Task/runtime JSON, private auth bindings and local acceptance | Existing private storage and formats |

The declarative-source inventory is broader than these two migrated files:

| Source | Owner and migration treatment |
| --- | --- |
| `.agents/ahu/agents/*.md` | `ahu` agent identities and optional nested input/output schemas; existing OKF `ahu:agent` schema stays compatible |
| `ahu-plugin.md` | `ahu` package declaration, already OKF `ahu:plugin`; its existing flat schema stays compatible |
| `evals/**` case and suite Markdown | `ahu` evaluation definitions, already `ahu:eval-case` / `ahu:eval-suite`; no implicit schema conversion |
| `.agents/skills/*/SKILL.md`, `docs/knowledge/*.md` | Skill and knowledge Markdown with their existing frontmatter conventions; migration never rewrites instructions |
| `src/catalog.rs` | Compiled compatibility catalog; `ahu.catalog_version` selects its revision, with no independent TOML source |
| Evaluation reference, freeze and manifest JSON | Experiment inputs and provenance, outside project launch policy and lock migration |
| Native harness settings, MCP registrations, native agents and hooks | Harness-owned TOML, JSON, JSONC, Markdown or scripts, retained at native paths |
| `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `deny.toml`, workflow YAML | Cargo manifests, Rust version, dependency checks and CI settings; not `ahu` project policy |
| Task, attempt, plugin-install and migration JSON | Local runtime records; existing formats and frozen task inputs remain exact |
| Auth bindings, credentials, private context acceptance | Private host storage; never promoted into shared OKF documents |

The migration operates only on project settings and their context lock. It does
not infer a common operational schema from Markdown elsewhere in the repository.

New project policy, context lock and project plugin-binding documents have
`okf_version: '0.2'`, the corresponding `type`, and an `ahu` mapping. Project settings retain schema version 1, all existing
field names, defaults, harness/model identities, permissions behavior and ranking
order. Unknown fields inside `ahu` are errors. Unknown top-level metadata and
the Markdown body are descriptive; they cannot enable execution or override
settings. The document loader retains nested metadata and body text when
rendering, and lock refresh retains existing descriptive metadata, including
signed and unsigned integers through 128 bits. Rendering checks the same
structural limits and verifies a semantic round trip before publishing. YAML
comments and formatting are not preserved; semantic metadata values and the body are.

Project configuration retains the existing 1 MiB input limit. Other shared
documents are limited to 2 MiB of UTF-8, 32 nested levels, 100,000 expanded
nodes and 2 MiB of expanded string/key data. Mappings require unique string keys
at every depth. Custom tags, numbers that are not finite, and YAML merge semantics are
unsupported. Aliases are charged against the same expansion limits. Parsing
errors do not echo document contents. The strict typed projection, not metadata,
determines operational settings.

For example, add settings beneath the existing `ahu` mapping, preserving the
rest of the document:

```yaml
ahu:
  # Other existing required project fields remain here.
  telemetry:
    enabled: false
    local_metrics: false
    endpoint: http://127.0.0.1:4318
```

## Explicit migration

From a regular project checkout:

```sh
ahu migrate --preview
ahu migrate --apply
ahu lock
```

Preview is read-only. It shows the exact deterministic targets, their digests,
and verified effective typed-policy parity. Apply recomputes the preview,
refuses duplicate formats for the same document, symlinks in any checked path
component, managed task worktrees and changed source/context inputs. Any existing
lock must match current shared inputs. Repository identity and HEAD are read
again at apply time; shared context is checked again before completion. Review
and commit both new documents and legacy source removals before launching; `ahu lock` reports their uncommitted
status until then. Neither command stages or commits anything.

The transaction saves bounded exact source backups and provenance before writing
targets, then removes the legacy sources. An existing canonical lock may be
refreshed after its exact bytes are backed up; an existing canonical policy is
retained byte-for-byte. Readers refuse an unfinished journal.
If applying it fails, rollback attempts to restore original bytes. After an
interruption, use:

```sh
ahu migrate --rollback
```

Recovery checks all four fixed document paths before changing any of them. A
concurrent edit stops recovery and remains intact; reconcile it against the
journal backups before retrying. Completed and rolled-back journals retain
backups. A completed journal is never reused; a rolled-back migration can retry
only with the same original sources. The journal contains shared configuration
bytes and pins, never raw execution captures. It lives under owner-only,
self-ignored `.ahu` storage.
Rollback publishes restored bytes with their recorded source mode atomically.
For journals created by older builds, restored source bytes at mode `0600` are
ambiguous with an intentional permission change; rollback refuses that state.
Inspect the journal and restore the recorded mode before retrying.
Concurrent migrations use an exclusive local file lock. These checks detect
ordinary concurrent edits; they are not isolation from a hostile same-user
process replacing ancestors between filesystem operations.

Setup, launch, inspection and lock refresh never migrate legacy documents.
Lock refresh and setup refuse an existing root legacy lock with an explicit
migration diagnostic. If both policy formats or both lock formats exist,
loading and migration refuse them even if their effective values agree;
reconcile duplicate sources before retrying. Supported starting states are:

| Policy | Lock | Result |
| --- | --- | --- |
| Legacy TOML | Legacy TOML or absent | Convert policy and generate canonical lock |
| Legacy TOML | Canonical OKF | Convert policy and refresh lock, retaining metadata and Markdown |
| Canonical OKF | Legacy TOML | Retain exact policy and convert lock |
| Absent | Legacy TOML | Refuse: no policy against which to establish parity |
| Canonical OKF | Canonical OKF or absent | No legacy migration; use explicit lock refresh if needed |

Mixed-format migration supports projects that previously refreshed their lock
while retaining legacy policy. All starting states share the same interruption,
conflict and rollback checks.

If preview refuses because a legacy lock no longer matches the current context,
`ahu lock --update` will also refuse while `ahu.lock` remains. Preserve the old
lock outside the checkout, then explicitly move or remove `ahu.lock` and rerun
`ahu migrate --preview`. With no existing lock, the preview builds a new lock
from the current context while still checking policy parity. Review the exact
preview and resulting Git diff before applying and committing; keep the backup
until the new lock is accepted. This procedure explicitly accepts the current
context as the new baseline and does not silently discard the old pins.

Existing tasks keep their exact recorded configuration and snapshot byte pins.
Their task worktrees and task records are never rewritten. Bounded legacy
readers preserve task inspection, but ahu v0.11 refuses schema-1 task resume or
dispatch; use the original runner or submit a new assignment. The generated
`lock.md` is excluded from the snapshot it fingerprints and arrives in new task
worktrees through the required committed base. Configuration and plugin binding files
are ordinary recognized context inputs and participate in drift detection.

## Plugin-binding foundation

A binding is not an installed or executable plugin. `ahu plugins --project`
provides an offline inventory of committed bindings and compatibility blockers;
see [project plugin inspection](reference.md#committed-project-plugin-bindings).
It does not install packages, verify source pins or load plugin code. A later
host must separately authorize capabilities and verify compatibility/source pins.

The strict `ahu` schema contains `schema_version: 1`, a lowercase plain `id`
matching the filename, exact semantic `version`, `source` with a credential-free
HTTPS or absolute local Git `repository`, full lowercase hexadecimal Git
`revision` (40 or 64 digits) and `sha256` (64 digits), and `compatibility` with an exact semantic `ahu` version
and `host_api: 1`. `enabled` defaults to false; `grants` defaults to empty. The
foundation recognizes `interactive-backend` and `read-project-context` grants;
unknown or duplicate grants are errors. Grant settings remain inert data.
Optional `source.subdirectory` selects a plain relative package directory.
`source.kind` defaults to `git`; `local` instead selects an absolute directory
snapshot and requires `revision` to equal `sha256`. See the
[installation lifecycle](reference.md#project-local-installation-lifecycle) for the
content digest format, verification rules and explicit enable/disable commands.
Private credentials, account bindings and local acceptance never belong here.

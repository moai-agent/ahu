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

Every new shared document has `okf_version: '0.2'`, the corresponding `type`,
and an `ahu` mapping. Project settings retain schema version 1, all existing
field names, defaults, harness/model identities, permissions behavior and ranking
order. Unknown fields inside `ahu` are errors. Unknown top-level metadata and
the Markdown body are descriptive; they cannot enable execution or override
settings. The document loader retains nested metadata and body text when
rendering, and lock refresh retains existing descriptive metadata. YAML comments
and formatting are not preserved; semantic metadata values and the body are.

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
refuses existing target files, symlinks in any checked path component, managed
task worktrees and changed source/context inputs. If a legacy lock exists, its
pins must match current shared inputs. Review and commit both new documents and
legacy source removals before launching; `ahu lock` reports their uncommitted
status until then. Neither command stages or commits anything.

The transaction saves bounded exact source backups and provenance before writing
targets, then removes the legacy sources. Readers refuse an unfinished journal.
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
Concurrent migrations use an exclusive local file lock. These checks detect
ordinary concurrent edits; they are not isolation from a hostile same-user
process replacing ancestors between filesystem operations.

Setup, launch, inspection and lock refresh never migrate legacy documents.
Lock refresh and setup refuse an existing root legacy lock with an explicit
migration diagnostic. If both policy formats or both lock formats exist,
loading refuses them even if their effective values agree. Migration also
refuses existing canonical destinations; reconcile conflicting sources before
retrying. The supported migration starts from legacy project policy, optionally
with its current legacy lock. A standalone legacy lock without legacy project
policy needs manual reconciliation; it is not silently accepted for migration.

Existing tasks keep their exact recorded configuration and snapshot byte pins.
Their task worktrees and task records are never rewritten. The bounded legacy
reader allows existing legacy worktrees to be inspected and resumed,
subject to their existing schema, identity and drift checks. The generated
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
HTTPS `repository`, full lowercase hexadecimal Git `revision` (40 or 64 digits)
and `sha256` (64 digits), and `compatibility` with an exact semantic `ahu` version
and `host_api: 1`. `enabled` defaults to false; `grants` defaults to empty. The
foundation recognizes `interactive-backend` and `read-project-context` grants;
unknown or duplicate grants are errors. Grant settings remain inert data.
Private credentials, account bindings and local acceptance never belong here.

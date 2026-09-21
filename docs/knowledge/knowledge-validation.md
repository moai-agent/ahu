---
type: Architecture
title: Knowledge validation
description: How configured bundles are checked and where validator trust ends.
tags: [knowledge, validation]
status: draft
generated: { by: docs-astra/1.1.0, at: 2026-09-13T00:38:18Z }
sources:
  - id: knowledge
    resource: ../../src/knowledge.rs
    title: Bundle preflight, validator invocation and report handling
  - id: config
    resource: ../../src/config.rs
    title: Knowledge configuration validation
  - id: tests
    resource: ../../tests/knowledge_lint.rs
    title: Synthetic validator integration tests
---

# Knowledge validation

`ahu knowledge lint` reads repository-relative bundle directories from
`knowledge.bundles`. Configuration rejects ambiguous or escaping path syntax and
duplicate entries. No configured bundles is a missing prerequisite, not a
successful empty check.[^config][^knowledge]

Before invoking the installed `okf` executable, ahu checks that each bundle holds
at least one concept Markdown file and contains only regular files and
directories. Symlinks, special files and trees beyond the inspection depth limit
are refused. The executable resolver excludes repository-local programs. The
validator receives arguments directly, without shell interpretation.[^knowledge]

For every bundle, `validate` runs before `lint`. ahu requires the OKF 0.5 JSON
report contract and checks fields, counts, command identity, and exit status.
Findings are combined and deduplicated. Errors fail the check; warnings fail only
when `knowledge.fail_on_warnings` is true. Human output escapes validator-derived
text; schema 1 JSON preserves the underlying values through JSON escaping.
Synthetic tests cover these decisions and malformed validator responses.[^knowledge][^tests]

The validator runs with the caller's privileges. Preflight does not prevent a
concurrent writer from changing files before OKF opens them. The 16 MiB report
limit bounds parsing after output collection, not subprocess output or memory.
Format validation does not verify the truth of a concept's claims or record human
acceptance.[^knowledge]

The project settings naming bundles travel under the rules described in
[task configuration inheritance](task-configuration-inheritance.md). The bundle
files themselves arrive through Git unless they independently qualify as agent
configuration; ordinary dirty documentation is not implicitly copied.

[^knowledge]: Preflight, invocation, report checks and renderers in src/knowledge.rs.
[^config]: Knowledge configuration and bundle path validation in src/config.rs.
[^tests]: Validator fixtures and CLI assertions in tests/knowledge_lint.rs.

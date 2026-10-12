---
okf_version: "0.2"
type: ahu:plugin
title: github offline synthetic provider
description: Offline synthetic GitHub issue action provider with revision tracking and refusal decisions
tags: [github, issues, offline]
schema_version: 1
id: github
package: github-plugin
host_api: 1
cargo_manifest: Cargo.toml
entrypoint:
  path: entrypoint
  protocol_version: 1
  timeout_ms: 5000
---

# github offline synthetic provider

This package provides a bounded offline synthetic GitHub provider. It never
opens network connections, never reads credentials, and never touches real
GitHub objects. Every invocation applies a comment or close action to a
synthetic issue state carried in the request itself.

## Input

- `version`: integer, must be 1.
- `action`: string, either `comment` or `close`.
- `operation_id`: string of 1 to 128 characters identifying the operation.
- `issue`: object with string `project` (1 to 64 characters), integer `number`
  (at least 1), and string `visibility` (`public` or `private`).
- `tracker_visibility`: string, `public` or `private`.
- `scopes`: list of granted scope strings, each 1 to 64 characters.
- `expected_revision`: nonnegative integer the host expects the synthetic
  state to be at.
- `state`: object with nonnegative integer `revision` and object `applied`
  mapping operation ids to digests, each a string of 1 to 128 characters.
- `body`: string of 1 to 16384 characters for `comment`, null for `close`.

## Decisions

The provider applies the first matching decision, in order:

1. If `applied` maps `operation_id` to the digest of this exact operation, the
   outcome is `idempotent` and the state is returned unchanged.
2. If `applied` maps `operation_id` to a different digest, the outcome is
   `refused` with reason `duplicate-ambiguous`.
3. If `scopes` does not include `issues:write`, the outcome is `refused` with
   reason `missing-scope`.
4. If `tracker_visibility` is `private` and the issue `visibility` is
   `public`, the outcome is `refused` with reason `visibility-mismatch`.
5. If `expected_revision` differs from the state `revision`, the outcome is
   `refused` with reason `stale-revision`.
6. Otherwise the outcome is `applied`, and the state revision advances by one
   with the operation digest recorded under `operation_id`.

## Result

The result object contains `version`, `provider`, `operation_id`, `outcome`,
`reason`, `reference`, and `state`. `reason` is null for `applied` and
`idempotent` outcomes. `reference` is an object with `project` and `issue`
for `applied` and `idempotent` outcomes, and null for refusals.
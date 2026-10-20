---
okf_version: "0.2"
type: ahu:plugin
title: github issue provider
description: Optional read-only GitHub issue inspection through gh and legacy offline simulation
tags: [github, issues]
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

# github issue provider

Version 1.1.0 adds one real capability: read-only inspection of an explicitly
named GitHub.com repository issue through the user's installed `gh` CLI.
Legacy V1 synthetic actions remain compatible and never call `gh`.

## Live inspection (input version 2)

Install this package through the pinned plugin lifecycle and commit a binding
with `enabled: true` and `grants: [execute, github-cli]`. Updating from 1.0.0
requires updating the version and source digest pins and reinstalling. Neither
installation nor inventory looks for `gh` or authenticates. Disabled, missing,
and uninstalled bindings cannot execute the provider. No core workflow requires
GitHub. A V2 request without either `github-cli` or `github-cli-write` returns
`github_cli_denied`.

The `github-cli` grant explicitly permits the trusted package to receive the
host's `HOME`, `XDG_CONFIG_HOME`, `GH_CONFIG_DIR`, `GH_TOKEN`, and `GITHUB_TOKEN`
environment values when present, and its PATH as `AHU_GITHUB_PATH`. These are
forwarded only after binding authorization and installed-package verification;
core does not read credentials or resolve `gh`. This is native authentication
access, not a read-only credential scope or an OS sandbox. Grant it only to a
reviewed, pinned package. The package makes only the fixed GET calls below.

The distinct `github-cli-write` binding grant also permits inspection and forwards
the same native auth environment with `AHU_GITHUB_CLI=1`, while additionally
setting `AHU_GITHUB_WRITE=1`. `github-cli` alone never sets the write marker.
With neither grant, the host forwards no GitHub auth environment or capability
markers. The write marker is an explicit trust/intent signal to same-user package
code, not OS or network enforcement; Ahu does not narrow the underlying token
scopes. This package still implements no live write action.

Write this exact shape to an input file, then invoke it explicitly:

```json
{"version":2,"action":"inspect","repository":"example/project","issue_number":7,"expected_visibility":"private"}
```

```sh
ahu plugins invoke github --input-file /absolute/issue-input.json
```

All five fields are required; unknown fields are refused. `repository` is a
plain `owner/name` (owner: 1–39 ASCII letters, digits or hyphens, starting with
a letter or digit; name: 1–100 ASCII letters, digits, dots, underscores or
hyphens, excluding `.` and `..`). URLs, alternate hosts and inferred repositories
are unsupported. `issue_number` is an integer from 1 to 2147483647, never a
boolean or float. `expected_visibility` is exactly `public` or `private` and
must match observed visibility in either direction. Input and the full host
request envelope must each fit within 64 KiB; duplicate JSON keys are refused.

The provider resolves `gh` only for a valid, authorized V2 request. It skips
relative/empty PATH entries, nonregular/nonexecutable/setid candidates, and
canonical executables beneath a `.git` marker. Lookup is not executable attestation
or filesystem confinement. It launches the canonical executable with an argv
vector, never a shell, from the host's temporary snapshot directory. The calls
use `gh api --hostname github.com --method GET`, first for `repos/OWNER/NAME`,
then for `repos/OWNER/NAME/issues/NUMBER`, with fixed `--jq` projections.
The repository response must match the requested full name (case-insensitively),
canonical GitHub URL and expected visibility. Both `private` and `visibility`
must agree. The issue must match its number, repository API URL and canonical
issue URL. Pull requests are refused. Redirects/renames to a different repository
are refused. Authentication and native credential storage remain `gh`'s concern.
There is no login, credential probe, retry, repository search or write operation.

Each `gh` process has a two-second deadline and a 64 KiB stdout limit, including
whitespace. stderr is discarded; nonzero status fails even with valid stdout.
Only the allowlisted native auth environment reaches `gh`, with a fixed system
PATH, `GH_PROMPT_DISABLED=1` and `GH_PAGER=cat`. `GH_HOST`, proxy variables, shell
hooks, arbitrary environment values and caller-selected commands are not forwarded.
The existing five-second host timeout and process-group cleanup also apply.

Successful inspection returns only this bounded result inside the usual CLI
`{schema_version:1,status:"ok",result:...}` envelope:

```json
{"version":2,"provider":"github","outcome":"read","code":null,"reference":{"repository":"example/project","issue_number":7,"url":"https://github.com/example/project/issues/7"},"state":{"visibility":"private","status":"open"}}
```

`status` is `open` or `closed`. Titles, bodies, comments, identities and arbitrary
provider output are neither requested in the projection nor returned to ahu.
Provider refusal returns `outcome: "error"`, a fixed `code`, and null `reference`
and `state`. Codes are `invalid_input`, `github_cli_denied`, `gh_unavailable`,
`provider_failed`, `provider_timeout`, `provider_output_limit`,
`provider_malformed`, `repository_mismatch`, `visibility_mismatch`, and
`issue_mismatch`. These are domain results: the generic transport succeeds and
exits zero, so callers **must check `outcome`**, not just the exit status.
Authentication failures use `provider_failed`; stderr, token values and OS error
text are never included. Host authorization, installation and transport errors
retain the generic host's fixed codes and exit statuses.

This is a pair of sequential observations, not an atomic visibility/issue
snapshot. It supplies no mutation, comments, closure, revision synchronization,
concurrency guarantee, task association or automatic issue completion. Tests use
fake executables offline; live GitHub and native keyring integration are not
validated by that coverage.

## Legacy offline simulation (input version 1)

V1 only applies a comment or close action to synthetic issue state supplied in
the request. It never calls `gh`, reads credentials or touches GitHub objects.
The following revision, scope and idempotency behavior is simulation only.

### Input

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

### Decisions

The provider applies the first matching decision, in order:

1. If `scopes` does not include `issues:write`, the outcome is `refused` with
   reason `missing-scope`.
2. If `tracker_visibility` is `private` and the issue `visibility` is
   `public`, the outcome is `refused` with reason `visibility-mismatch`.
3. If `applied` maps `operation_id` to the digest of this exact operation, the
   outcome is `idempotent` and the state is returned unchanged.
4. If `applied` maps `operation_id` to a different digest, the outcome is
   `refused` with reason `duplicate-ambiguous`.
5. If `expected_revision` differs from the state `revision`, the outcome is
   `refused` with reason `stale-revision`.
6. Otherwise the outcome is `applied`, and the state revision advances by one
   with the operation digest recorded under `operation_id`.

Scope and visibility checks apply to every request, including retries with a
matching receipt. Refusal leaves the supplied state unchanged. These checks
use synthetic input values; they do not verify live permissions or visibility.

### Result

The result object contains `version`, `provider`, `operation_id`, `outcome`,
`reason`, `reference`, and `state`. `reason` is null for `applied` and
`idempotent` outcomes. `reference` is an object with `project` and `issue`
for `applied` and `idempotent` outcomes, and null for refusals.

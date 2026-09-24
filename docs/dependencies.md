# Release dependency policy

This policy governs dependency review and release checks. Complete the scan and
maintainer review before release; exceptions require the approval described below.

[`deny.toml`](../deny.toml) checks the locked dependency graph with all features and no target
filter, including build and development dependencies.

| Area | Rule |
| --- | --- |
| Licenses | Accept MIT, Apache-2.0 and Unicode-3.0 expressions; reject other or unidentified licenses. For an `OR` expression, an allowed alternative suffices. Unicode-3.0 covers the Unicode data license required by `unicode-ident`. Only unpublished workspace packages are exempt; this does not license ahu itself. |
| Sources | Accept crates.io only; reject Git dependencies and other registries. Review local path dependencies and Cargo source replacement configuration manually. |
| Advisories | Fail for known vulnerabilities regardless of severity or missing CVSS, unsoundness, unmaintained crates and yanked releases, including transitive dependencies. No advisory exceptions are configured. |
| Duplicates | Warn, keeping dependency paths visible. Review compatibility, maintenance and size costs; duplicate versions alone are not evidence of a vulnerability. Do not force arbitrary downgrades to remove warnings. Wildcard dependency requirements fail. |
| Review | A repository maintainer reviews every manifest, lockfile, source override and policy change before merging, including upstream provenance, new build scripts/proc macros, licenses, advisories, and duplicate warnings. The author supplies the scan result and rationale. CI does not enforce reviewer identity or branch protection. |
| Exceptions | Require maintainer approval, exact crate/version or advisory scope, a named accountable maintainer, rationale, compensating measures and a calendar expiry or explicit upstream review condition. Record these alongside a narrowly scoped configuration entry; keep confidential evidence in private review records. Revisit on each affected lockfile update and before release, and remove resolved exceptions. |

Run the same scan locally from the repository root:

```sh
cargo install cargo-deny --version 0.20.2 --locked
cargo deny --version
cargo deny --locked check --deny index-failure
```

The [dependency workflow](../.github/workflows/dependencies.yml) uses the upstream cargo-deny **0.20.2** Linux release archive, verified against
a checked-in SHA-256, and an immutable checkout action revision. Tool upgrades
must update the version and digest together after verifying the upstream release
and rerunning the scan and failure probe. Rust 1.96.0 supplies Cargo metadata in
this workflow; this is not a declaration of the project's minimum Rust version.

Each normal scan fetches the current
[RustSec advisory database](https://github.com/RustSec/advisory-db) and registry
data. The command promotes `index-failure` warnings to errors so an incomplete
yanked-release lookup also fails the check. Advisory fetch/check failures fail the
check. The database intentionally moves independently of the tool and lockfile so newly disclosed advisories can fail an unchanged tree.
The workflow runs on pull requests, pushes to `main` and `release/**`, weekly,
and on manual dispatch.
Offline runs use cached information and are diagnostic only;
the configured seven-day staleness limit does not make them equivalent to an
online scan. Record the database revision when preserving scan evidence.
A separate `cargo audit` gate is not required: it adds no required advisory
coverage to this RustSec-backed check. Neither tool proves absence of unknown
vulnerabilities or substitutes for dependency review.

To verify rejection without changing the real policy, run this local probe.
It adds a ban for the existing direct dependency `serde`, uses cached metadata,
and requires both a nonzero exit and the expected diagnostic:

```sh
set -eu
probe_dir="$(mktemp -d)"
trap 'rm -rf "$probe_dir"' EXIT
awk '{ print; if ($0 == "[bans]") print "deny = [\"serde\"]" }' \
  deny.toml > "$probe_dir/deny.toml"
if cargo deny --locked --offline --config "$probe_dir/deny.toml" check bans \
    > "$probe_dir/output" 2>&1; then
  cat "$probe_dir/output"
  echo 'Expected the serde ban to fail' >&2
  exit 1
fi
cat "$probe_dir/output"
grep -F 'banned' "$probe_dir/output"
grep -F 'serde' "$probe_dir/output"
```

Configuration semantics are documented by
[cargo-deny](https://embarkstudios.github.io/cargo-deny/).

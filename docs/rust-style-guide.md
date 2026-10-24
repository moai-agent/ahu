# Rust style guide

This guide applies to Rust code in ahu, its workspace crates, tests, and examples.
Use it for new code and substantive changes to existing code. Improve nearby
code when it helps the change; keep unrelated refactors in separate commits.
Existing code is not automatically an example of the desired style.

Use the [Rust Style Guide](https://doc.rust-lang.org/style-guide/) for formatting
and the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/) for
API design. The rules below adapt those references to ahu. Prefer clear code
and explicit invariants over clever abstractions or arbitrary size limits.

## Formatting and naming

- Use the toolchain pinned in `rust-toolchain.toml` and default rustfmt settings.
  Let the formatter settle layout; do not hand-align code or add formatting
  exceptions without a concrete reason.
- Use descriptive domain names: `task_id`, `attempt`, `merge_target`, and
  `session_ref`. Avoid abbreviations that require reading another module.
- Follow Rust naming conventions: `snake_case` for functions and modules,
  `UpperCamelCase` for types, and `SCREAMING_SNAKE_CASE` for constants.
- Prefer early returns and explicit `match` expressions when they clarify
  failure paths. Use iterators for straightforward transformations; use loops
  when state changes, error handling, or cleanup are easier to follow.

## Modules and architecture

- Give each module a coherent responsibility. Separate parsing and validation,
  lifecycle decisions, and filesystem or process effects so each can be tested.
  Split a module when its responsibilities diverge, not at a fixed line count.
- Keep functions focused on one operation. Name meaningful intermediate values
  rather than nesting long expressions or passing many unrelated flags.
- Default to private items. Use `pub(super)` or `pub(crate)` when sufficient;
  expose `pub` APIs only for actual consumers. Avoid expanding visibility just
  to test an implementation detail.
- Keep ahu focused on harness coordination, MCP, task state, and worktrees.
  Provider credentials, issue schemas, and terminal-specific mappings belong
  in external plugins or skills. See [plugin architecture](plugin-architecture.md).
- Introduce traits, generic parameters, macros, or shared helpers for a concrete
  repeated need or boundary. Do not build a framework for hypothetical providers.
  Keep domain helpers near their callers instead of growing a generic utility module.
- Review new dependencies against the [dependency policy](dependencies.md).
  Prefer existing facilities when they fit; do not reimplement security-sensitive
  behavior merely to avoid a dependency.

## Types and ownership

- Represent distinct states with enums and validate inputs at boundaries.
  Prefer a named type over interchangeable strings or boolean flags when it
  prevents mixing identities, units, or lifecycle states.
- Keep invalid combinations out of internal types where practical. Do not use
  `Option`, empty strings, or zero as undocumented failure or unknown sentinels.
- Borrow read-only inputs with `&str`, `&Path`, or slices when ownership is not
  needed. Return owned values when the caller must retain them. Use `PathBuf`
  and `OsString` for filesystem paths and OS arguments; do not assume Unicode.
- Make clones intentional. An owned snapshot can simplify lifetimes and lock
  handling; explain non-obvious ownership decisions rather than adding lifetime
  complexity solely to remove a small allocation.
- Prefer ordinary ownership to shared mutable state. Use `Arc` and locks only
  when sharing is required; keep lock scope and acquisition order explicit.
- Keep wire formats separate from internal invariants. Validate deserialized
  values before use. Add compatibility behavior only for a documented supported
  contract, not speculative legacy schemas.

## Errors and diagnostics

- Return `Result` for failures callers must handle. Use the existing module or
  crate error types and preserve error categories and source chains. Add context
  that identifies the failed operation and an actionable next step.
- Do not use `unwrap`, `expect`, indexing, or assertions for fallible user input,
  filesystem state, process output, or network responses. A production panic
  requires a documented invariant that external input cannot violate. Tests may
  use `expect` or `unwrap` for fixture setup and expected success.
- Do not turn an error into an empty result, success, or a fabricated default.
  If an operation is best effort, make that policy explicit and retain a bounded
  diagnostic when useful. Cleanup failure must not hide the original failure.
- Keep library logic separate from terminal presentation. Return structured
  results; render user messages at command boundaries. Do not parse prose errors
  to recover structured state.
- Do not expose credentials, prompts, native transcripts, or private configuration
  through errors, `Debug`, or telemetry. Report unknown observations as unknown.

## Side effects and lifecycle

- Pass command arguments separately with `std::process::Command`. Avoid shell
  interpolation. Where a host contract requires a shell command, use the existing
  reviewed quoting and prompt transport instead of inventing another escape layer.
- Bound subprocess duration, output, and reads of untrusted files. Reuse existing
  process ownership and cancellation mechanisms; do not leave detached workers.
- Treat process exit, accepted results, integration, and cleanup as distinct facts.
  Never infer observed liveness solely from a persisted running state.
- Make state transitions and their failure order explicit. Use existing atomic
  persistence and ownership checks; a prior existence or ancestry check alone
  does not protect a later mutation from concurrent changes.
- Use resource acquisition is initialization (RAII) for resource management
  where appropriate. Provide explicit fallible finalization
  when cleanup errors matter; `Drop` cannot return a failure to the caller.
- Keep blocking I/O and locks out of asynchronous suspension paths. Do not add an
  async runtime to code that is adequately served by the existing blocking design.
- Prefer safe Rust. Keep necessary `unsafe` small, with a `SAFETY` comment that
  explains the actual preconditions, lifetimes, and synchronization guarantees.
  A worktree or harness approval setting is not an OS security boundary.

## Tests and documentation

- Test externally meaningful behavior and invariants. For a bug fix, add a focused
  regression that fails without the fix. Avoid tests that merely repeat the code.
- Use temporary repositories and synthetic harnesses for ordinary tests. Do not
  depend on personal credentials, installed providers, a real home directory, or
  live network services. Pass environment settings to child commands instead of
  mutating process-wide environment in parallel tests.
- Exercise failure, interruption, and recovery where state changes or resources
  are involved. Use deterministic synchronization instead of timing sleeps to
  prove ordering. Keep live compatibility checks separate from fixture coverage.
- Document public contracts, units, side effects, and failure conditions with
  rustdoc. Explain surprising invariants and tradeoffs in comments; avoid narrating
  syntax or preserving obsolete implementation history.
- Keep raw traces outside repositories. Follow the privacy and bounded evaluation
  storage rules in [repository instructions](../AGENTS.md); never use real captures as fixtures.

## Review and checks

Before accepting a change, check that ownership and state transitions are clear,
errors remain actionable, new APIs are necessary, and side effects have failure
coverage. Record justified exceptions near the code or in the review.

Run focused tests while working. Before handing off Rust changes, run the checks
below from the repository root, plus applicable checks in the
[CI workflow](../.github/workflows/ci.yml):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --no-default-features --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Use narrow, explained lint exceptions. Do not turn off warnings globally or enable
all opinionated Clippy groups as a substitute for review. See the
[Clippy lint guidance](https://doc.rust-lang.org/clippy/usage.html#lint-configuration).
Formatting and Clippy are automated checks; the design rules require review.
This guide does not claim that all existing code already conforms.

For documentation-only edits, check links and run
`scripts/lint-prose.sh --no-sync docs/rust-style-guide.md AGENTS.md README.md`
when Vale styles are installed. Use the [development instructions](../README.md#development)
for tool setup and additional validation.

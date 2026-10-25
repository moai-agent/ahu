# Rust style guide

This guide applies to Rust code in ahu, its workspace crates, tests, and examples.
Use it for new code and substantive changes to existing code. Improve nearby
code when it helps the change; keep unrelated refactors in separate commits.
Existing code is not automatically an example of the desired style.

Use the [Rust Style Guide](https://doc.rust-lang.org/style-guide/) for formatting
and the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/) for
API design. The rules below adapt those references to ahu. Prefer clear code
and explicit invariants over clever abstractions or arbitrary size limits.


## Customer runtime language policy

All ahu-maintained customer-facing executable code must be implemented in Rust.
This includes the CLI, shipped harness adapters, executable plugins, session
hosts, and setup or runtime helpers. Installing and using these components must
not require Python. Python is allowed only for development tooling, tests,
fixtures, and experiments that are not shipped as supported customer features.
A Rust launcher that delegates its implementation to Python does not satisfy
this rule.

Skills and declarative configuration remain text. External tools such as `gh`,
cmux, and native harnesses retain their own implementations; this rule governs
code maintained and distributed by ahu. Plugin wire protocols remain independent
of implementation language, but ahu-maintained supported plugins must use Rust.

Existing Python adapters require migration before their next supported release.
Keep them external and optional during migration, preserve their behavior and
security checks, and verify the Rust replacements through protocol tests and
live integration checks. Do not claim existing adapters already meet this policy.

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

## CLI behavior and plugin contracts

The CLI is an API for operators, scripts, and agents. Keep command handlers thin:
parse arguments, call domain operations, and render results. CLI and MCP entry
points should use the same domain validation rather than maintain parallel rules.
Do not call the ahu CLI from core Rust code to reuse its implementation.

- Keep machine output separate from human presentation. In structured modes,
  stdout contains only the documented result or protocol frames, without colors,
  progress messages, or banners. Follow each command's stderr and exit-status
  contract; do not silently change it to a new global convention.
- Require explicit input sources for commands that read stdin. Headless and
  structured operations must not wait for a terminal prompt. Missing required
  input should produce an actionable error rather than select an implicit default.
- Define ordering for user-visible lists when order has meaning; do not expose
  accidental hash-map iteration order. Test semantic JSON values rather than
  incidental whitespace, except where exact framing or byte limits matter.
- Handle broken pipes, cancellation, and output limits deliberately. A failure
  to deliver a result does not prove the underlying operation had no effects.
  See the Rust CLI book's [machine communication guidance](https://rust-cli.github.io/book/in-depth/machine-communication.html).

Use skills for workflows that existing CLIs already support. Use executable
plugins for required machine contracts, keeping the process boundary independent
of the plugin's implementation language. Do not introduce dynamic Rust library
loading or require plugins to link the entire ahu core. Shared contract crates
should contain protocol types and validation, not provider discovery, credentials,
or workflow orchestration.

- Keep the [one-shot protocol](executable-plugins.md) and
  [session-host protocol](executable-hosts.md) distinct. Reuse their transport
  implementations; do not merge their framing or privileges to reduce duplication.
  Keep exact versions, limits, and error codes in those contract documents.
- Separate package identity, protocol version, capabilities, and authorization.
  A declared capability does not grant permission. Construct grants from trusted
  host decisions, never from a plugin response or caller-supplied identity.
  Treat plugin results as data, not instructions to execute or expand scope.
- Make executable selection, working directory, environment, and all three
  standard streams explicit at process boundaries. Rust's
  [`Command` defaults](https://doc.rust-lang.org/std/process/struct.Command.html#method.new)
  include inherited environment and working directory. Follow the selected
  transport's environment policy; do not copy that policy onto native harness
  launches, which have different configuration requirements.
- Discovery, help, and inventory must not activate plugins or probe their accounts.
  Do not fall back to an unpinned PATH executable when an installed package fails
  verification. Keep provider references opaque and preserve frozen host identity
  for existing tasks; removal must not redirect their cleanup to another host.
- Validate framing, versions, request identity, and payload limits before using
  a response. Define unknown-field behavior in the contract; do not use permissive
  deserialization defaults to conceal an incompatible peer. Reject unsupported
  versions with an actionable error rather than speculative compatibility layers.
- Distinguish refusal before dispatch, confirmed completion, and unknown effects
  after dispatch. A timeout is not proof that a write failed. Do not automatically
  retry or reconnect after mutations; any retry needs an explicit idempotency or
  reconciliation contract. Provider recovery belongs in the plugin; a tracker
  error must never restart an agent task.
- Keep telemetry outside protocol output. Use existing bounded events and
  allowlisted attributes; do not attach input, output, environment, or native
  diagnostics to spans. Telemetry failure must not turn successful execution
  into a retry of its effects.

For a contract change, include a short review note: the concrete consumer,
why a skill or existing contract is insufficient, inputs and outputs, authority,
side effects, failure/unknown outcomes, and the tests proving the boundary.
Update the contract documentation and independent authoring example together
when their behavior changes. Do not expand the core solely for one provider.

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
- For plugin transport changes, use an independent executable fixture that
  speaks the documented wire format. Include hand-authored invalid frames so
  matching host/client serialization mistakes cannot hide a protocol defect.
  Cover missing or revoked grants, absent providers, malformed and partial
  output, blocked streams, timeout after a side effect, and child cleanup as
  applicable. Verify refused activation does not execute the fixture.
- Exercise CLI contracts through the built executable, including exit status,
  stdout, stderr, and non-terminal input. Test operation without optional provider
  binaries or credentials. A live provider test belongs in its plugin repository;
  a core fixture pass is not a claim of provider compatibility.
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

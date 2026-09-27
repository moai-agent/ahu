---
name: ahu-harness-upgrade
description: Update the installed Codex, Claude Code, Antigravity CLI, and OpenCode harnesses and validate ahu compatibility. Use for ahu development and maintenance; this internal skill is not installed for ahu users.
---

# Upgrade ahu development harnesses

Keep the locally installed harness CLIs current and keep ahu's adapters honest
about the exact versions they support. This skill is for maintaining ahu itself;
do not add it to the skills bundled by `ahu setup` or `ahu mcp setup`.

## Upgrade workflow

1. Identify which supported harnesses are installed with `command -v` and their
   version commands (`codex --version`, `claude --version`, `agy --version`,
   and `opencode --version`). Record the executable path as well as the version;
   PATH shims and package-manager installs can point at different copies.
2. Find the actual installation source before updating. Prefer the harness's
   documented updater or the package manager that owns the installed binary.
   Do not install a second copy over a managed install, change authentication,
   or replace user configuration. If provenance is unclear, report it and ask
   before choosing a new installation method.
3. Check the vendor's current release notes and installation documentation for
   the supported stable release and update procedure. Do not treat a model name,
   ahu agent semantic version, IDE version, or prerelease as the CLI version.
4. Do not update a harness while an ahu task is actively using that executable.
   Let tasks finish, then update one harness at a time through its owning
   installer. Re-read `command -v` and the version after every update. Stop if
   the update changes PATH resolution unexpectedly or fails.
5. Recheck authentication status without displaying, reading, or copying
   credentials. An upgrade must not print environment values or auth files.

Use each installation's current official instructions rather than assuming
these commands apply to every machine. Common current mechanisms are:

- Codex: update through the package manager that owns `codex` (for example,
  Homebrew Cask when the executable is in its Caskroom).
- Claude Code: `claude update` for its managed installation; package-manager
  installs should be updated by that package manager.
- Antigravity CLI: the official Antigravity CLI installer supports upgrading
  the native `agy` binary; verify its documented flags before running it.
- OpenCode: `opencode upgrade` for self-managed installs, or its detected
  package manager when that manager owns the binary.

## Validate ahu compatibility

Updating a CLI does not establish that ahu supports it. Validate each adapter
against the exact installed version before changing `src/catalog.rs`:

1. Inspect that adapter's command construction, version parsing, output/event
   parsing, cancellation, and result collection in the current source and tests.
2. Run the current repository binary's focused adapter tests and a read-only
   launch preview for the harness. Prefer `cargo run -- ...` when `ahu` on PATH
   is older than the checkout.
3. For an interactive profile, verify a minimal session starts with the pinned
   model and receives the prompt. Record observed behavior rather than inferring
   it from `--help`.
4. For headless support, run a bounded smoke task in an isolated temporary Git
   repository and inspect its durable result. Verify the actual argv, prompt
   delivery, completion signal, exit status, cancellation behavior, and output
   parsing. Do not use a real project task or allow writes outside that temporary
   repository.
5. Add an exact headless version to the catalog only after the adapter behavior
   has passed those checks. Keep untested versions rejected. Update interactive
   compatibility claims only when their behavior has been observed too; do not
   infer support for every intervening version.
6. Update catalog tests and evidence/comments with the precise validated version
   and observed behavior. Run formatting and focused tests, then run
   `ahu lock --update` (using the checkout's current binary) for agent-context
   edits and review the lockfile diff.

Keep upgrade and adapter compatibility separate in reports: list the installed
version, the version validated by ahu, which checks ran, and any blocked or
unverified behavior. A newer CLI can be installed while ahu correctly refuses
to launch it headlessly until compatibility is validated.

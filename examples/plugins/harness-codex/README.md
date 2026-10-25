# Experimental Codex launch planner

This independent Rust executable translates launch settings into Codex arguments.
It has no ahu linkage or Python runtime requirement. It does not launch Codex,
access accounts, receive prompts, or manage task state.

See the [pilot contract](../../../docs/harness-plugin-pilot.md) for scope,
permissions, limitations, and test commands. This is a local experiment, not a
published supported harness plugin or live compatibility certification.

Prepare a platform-specific package explicitly **before computing its digest**.
From the repository root, with the pinned Rust toolchain installed:

```sh
CARGO_TARGET_DIR=/tmp/ahu-rust-codex-agent-target cargo build --locked --release --manifest-path examples/plugins/harness-codex/Cargo.toml
package_dir="$(mktemp -d /tmp/ahu-codex-package.XXXXXX)"
mkdir "$package_dir/bin"
cp examples/plugins/harness-codex/Cargo.toml examples/plugins/harness-codex/Cargo.lock examples/plugins/harness-codex/ahu-plugin.md "$package_dir/"
install -m 755 /tmp/ahu-rust-codex-agent-target/release/ahu-experiment-harness-codex "$package_dir/bin/ahu-experiment-harness-codex"
printf 'Prepared package: %s\n' "$package_dir"
```

The staging directory is an installable package, not a source checkout. The
source crate has its own workspace and lockfile; only serde/serde_json and their
transitive dependencies are linked. Installation never builds the crate. Do not
commit binaries. Build on the destination OS/architecture; repeat preparation
and digest pinning whenever any package bytes or executable permissions change.
The commands reproduce the preparation procedure, not a cross-platform binary
hash guarantee. Retain required dependency license notices for distribution.

To try the installation in a disposable project, build the pilot binary as
instructed in the contract. Use its absolute path below. Create an ordinary Git
repository, then follow the [local package binding example](../text-stats/README.md),
with these substitutions:

- Package directory: the absolute `$package_dir` prepared earlier.
- Binding ID and filename: `harness-codex` and `harness-codex.md`.
- Package version: `0.1.0`.
- Grant: `[harness-launch]` instead of `[execute]`.
- Compute the package digest after finalizing its contents; set both local source
  revision and SHA-256 to that digest. Commit the enabled binding and context lock.

Install with `ahu plugins install harness-codex`. Do not use `plugins invoke`:
registered Codex task planning selects this package in the pilot build.
A registered Codex agent can then run with `ahu @agent --headless --work-mode --allow-widened-approvals --prompt 'TASK'`.
Use a synthetic task, a known working native sign-in, and an explicitly reviewed
MCP configuration pointing to the pilot binary. Work mode grants unattended native
tools and does not provide OS confinement.

After reviewing and integrating the task result, clean up its task and worktree
through normal ahu commands. Turn off the plugin, commit that policy change, and
remove it with `ahu plugins remove harness-codex`. Keep evaluation records and
raw telemetry in the locations required by the repository privacy policy. Do not
remove uncertain work or work awaiting integration just to finish the experiment.

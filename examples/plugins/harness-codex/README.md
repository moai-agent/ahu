# Experimental Codex launch planner

This independent Python package translates launch settings into Codex arguments.
It requires `/usr/bin/python3` and uses no third-party Python libraries. It does
not launch Codex, access accounts, receive prompts, or manage task state.

See the [pilot contract](../../../docs/harness-plugin-pilot.md) for scope,
permissions, limitations, and test commands. The Cargo manifest supplies identity
metadata only; installation does not build the package. Keep `codex.py` executable.
This package is a local experiment, not a published supported harness plugin.

To try the installation in a disposable project, build the pilot binary as
instructed in the contract. Use its absolute path below. Create an ordinary Git
repository, then follow the [local package binding example](../text-stats/README.md),
with these substitutions:

- Package directory: `examples/plugins/harness-codex`.
- Binding ID and filename: `harness-codex` and `harness-codex.md`.
- Package version: `0.1.0`.
- Grant: `[harness-launch]` instead of `[execute]`.
- Compute the package digest after finalizing its contents; set both local source
  revision and SHA-256 to that digest. Commit the enabled binding and context lock.

Install with `ahu plugins install harness-codex`. Do not use `plugins invoke`:
registered Codex task planning selects this package in the pilot build.
A registered Codex agent can then run with `ahu @agent --work-mode --prompt 'TASK'`.
Use a synthetic task, a known working native sign-in, and an explicitly reviewed
MCP configuration pointing to the pilot binary. Work mode grants unattended native
tools and does not provide OS confinement.

After reviewing and integrating the task result, clean up its task and worktree
through normal ahu commands. Turn off the plugin, commit that policy change, and
remove it with `ahu plugins remove harness-codex`. Keep evaluation records and
raw telemetry in the locations required by the repository privacy policy. Do not
remove uncertain work or work awaiting integration just to finish the experiment.

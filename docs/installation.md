# Install from reviewed source

Run the shell examples in Bash or a POSIX shell (`sh`); keep variables in the same
shell between blocks.

The release set is ahu **0.11.1**, five harness packages **0.1.1**, GitHub
packages **0.1.1**, and cmux host **0.2.1**. See the
[publication checklist](publication.md) for their repositories and release gates.
Until those tags are published, use an explicitly reviewed candidate checkout;
do not substitute the repository's default branch and call it this release.

## Install the CLI

You need Git, rustup, Cargo, and the toolchain pinned by `rust-toolchain.toml`.
On a supported Unix platform (macOS or Linux), clone the published source tag:

```sh
git clone --branch v0.11.1 https://github.com/moai-agent/ahu.git
cd ahu
git rev-parse HEAD
```

For a prepublication rehearsal, start in a clean checkout of the exact reviewed
candidate instead. The remaining commands are identical. Build and install from
that checkout, with Cargo output inside its ignored `.ahu/`:

```sh
umask 077
ahu_source=$(pwd -P)
test ! -L .ahu && test ! -L .ahu/build
mkdir -p .ahu/build
test ! -L .ahu/build/install
export CARGO_TARGET_DIR="$ahu_source/.ahu/build/install"
cargo install --path "$ahu_source" --locked --force
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
command -v ahu
ahu --version
```

Expect `ahu 0.11.1`. Cargo's installed executable and registry metadata remain
in Cargo's normal installation directory; ahu's project data lives under each
project's `.ahu/`. Cargo replaces the executable with a fresh file. Do not copy
over an executable in place on macOS. Finish old tasks before an upgrade and
retain the old CLI/package identity until their cleanup is complete.

## Build only the harness packages you need

Installing ahu builds the CLI and MCP server, with no native harness adapters.
Each production adapter has an independent source repository, version, lockfile
and package. No sibling source checkout is required. See the
[harness repository inventory](harness-plugins.md#packages).

The new harness repositories are local release candidates, not published remote
repositories yet. Use the reviewed local source checkout supplied for validation.
After publication, use that repository's verified release tag or source archive.
Follow its INSTALL.md to build and stage its package with the Rust `package`
command. Install the native CLI and sign in separately.

For example, choose just Codex and Claude Code. Build each in its own checkout:

```sh
codex_source=/absolute/path/to/ahu-plugin-codex
claude_source=/absolute/path/to/ahu-plugin-claude-code
for plugin_source in "$codex_source" "$claude_source"; do
  (
    cd "$plugin_source"
    umask 077
    test ! -L .ahu
    mkdir -p .ahu/build .ahu/packages
    export CARGO_TARGET_DIR="$PWD/.ahu/build/packages"
    cargo run --locked --bin package -- "$PWD/.ahu/packages/v0.1.1"
  )
done
codex_package="$codex_source/.ahu/packages/v0.1.1/harness-codex"
claude_package="$claude_source/.ahu/packages/v0.1.1/harness-claude-code"
ahu plugins digest "$codex_package"
ahu plugins digest "$claude_package"
harnesses="codex claude-code"
```

Run multi-package blocks in a POSIX shell such as `sh` or Bash; zsh does not split
the space-separated list by default. Choose only the packages required by your
project. Each destination must be new, and each binary is OS/architecture specific.
The package digest covers all staged contents. Installation never builds source
or installs unrelated plugins. A client that only connects to `ahu mcp serve`
does not need a native harness plugin; a task launched by ahu does.

## Prepare and activate in the consuming repository

In the same shell, set `project` to your existing Git checkout. For a disposable
rehearsal, create an ordinary repository under the source checkout's ignored
`.ahu/` instead:

```sh
project="$ahu_source/.ahu/install-rehearsal"
mkdir "$project"
git -C "$project" init
cd "$project"
printf '.ahu/\n' > .gitignore
git add .gitignore
git commit -m 'Ignore repository-local ahu state'
```

Git needs your configured author identity. In a real project, merge `.ahu/` into
its existing ignore rules instead of replacing `.gitignore`. Use canonical
absolute paths; symlinked package path components are refused.

Prepare disabled bindings without executing the plugins, then review them:

```sh
for package_dir in "$codex_package" "$claude_package"; do
  ahu setup --harness-plugin "$package_dir"
done
git diff --no-index /dev/null .agents/ahu/plugins/harness-codex.md
```

The diff command normally exits 1 because it displays a new file. Inspect every
generated binding. It must pin the intended package path, digest, version 0.1.1,
and exact CLI compatibility 0.11.1. Commit the disabled bindings, then install:

```sh
git add .agents/ahu/plugins
git commit -m 'Pin reviewed harness packages'
for harness in $harnesses; do
  ahu plugins install "harness-$harness"
done
ahu plugins installed --output json
```

Edit each binding's `ahu.grants` from `[]` to `[harness-runtime]` only after
reviewing the package's security notice. Commit the grant edits before running
`enable`; activation refuses an uncommitted binding. Then enable and configure
the project:

```sh
git diff -- .agents/ahu/plugins
git add .agents/ahu/plugins
git commit -m 'Grant reviewed harness runtime authority'
for harness in $harnesses; do
  ahu plugins enable "harness-$harness"
done
git diff -- .agents/ahu/plugins
git add .agents/ahu/plugins
git commit -m 'Authorize reviewed harness runtimes'
ahu setup
```

Setup asks for exact models, proposes project MCP entries and skills, and creates
registered development agents. Choose models available to your native accounts.
Review its output; it preserves existing project files and may refuse conflicts.
Do not overwrite working native account configuration to force a test to pass.
Inspect each generated MCP command: it must resolve to the intended installed
`ahu`. Antigravity supports workspace `.agents/mcp_config.json`; if a global
server has the same name, use a distinct project server name and explicitly
choose it when validating the intended executable. Native trust prompts belong
to the native CLI. Kiro may require its default workspace trust; it is optional.

```sh
git status --short
ahu lock --update
git add .agents
# Also stage the native project MCP/skill files listed by setup after review.
git diff --cached
git commit -m 'Configure project agents and MCP'
ahu lock
ahu agents
ahu doctor --work-mode
```

Context-lock errors identify additional recognized files that need review and
commit, or local acceptance that must be refreshed. Global Git ignore rules can
hide generated skills even when `git status` looks clean. For example:

```sh
git check-ignore -v .claude/skills/ahu-direct-agents/SKILL.md
# After inspecting the generated skill, override the ignore for this file only:
git add -f .claude/skills/ahu-direct-agents/SKILL.md
```

Repeat for each reviewed generated skill named by `ahu lock`, refresh the lock,
and commit those exact paths. Do not force-add a whole native configuration
directory; it may contain private settings or credentials. Never commit those
secrets or `.ahu/`. The ahu development Git checkout can contain development
bindings; do not reuse them as portable project setup. Release source archives
omit its workstation bindings, configuration and lock. Create reviewed bindings
for each consuming project as described here.

## Run, inspect, resume and remove a task

Select an actual name printed by `ahu agents`; setup does not promise the names
used by the ahu development repository. This work-mode test requests automatic
native approvals. A worktree is not a sandbox, and native controls may still deny
tools. Guarded mode is separate and may refuse the installed native configuration.

```sh
agent=dev-codex  # replace with the name printed by ahu agents
ahu "@$agent" --work-mode --name install-smoke --timeout 120 \
  --prompt 'Call the actual ahu_skills_list MCP tool, compute 17 plus 25 with a shell command, and report 42. Do not change files or launch other agents.' \
  --output json
ahu result @install-smoke --output json
ahu resume @install-smoke --prompt 'Call ahu_skills_list again and report completion. Do not change files.' --output json
ahu wait @install-smoke --output json
ahu task @install-smoke
```

Require a successful outcome and actual tool result; discovery alone is not a
tool call. For telemetry/eval validation follow [evaluations](evaluations.md) and
[local traces](reference.md#local-traces). Raw evidence stays in the owning
repository's ignored `.ahu/`. Repeat with unique task names for other harnesses.
OpenCode 2.x mini does not support the direct automatic coordinator shortcut;
its headless work mode is a separate supported path. Kiro's trust prerequisite
does not make Kiro mandatory for other users.

Inspect the task worktree and branch printed by `ahu task`. Preserve or integrate
useful changes. Once the task is terminal, its worktree is clean, and its branch
is integrated (the no-change smoke is already based on HEAD), remove only that task:

```sh
ahu cleanup @install-smoke
ahu remove @install-smoke
ahu tasks --all
```

Native session histories are retained by their native tools. To remove a plugin,
finish tasks using it, disable its binding, refresh/review/commit the lock and
binding, then run `ahu plugins remove ID`. Do not delete `.ahu/` while tasks or
registered worktrees are active.

## Optional companion plugins

Use the complete installation instructions in the separate
[GitHub plugins](https://github.com/moai-agent/ahu-plugin-github) repository.
GitHub is not required for task execution. The
[cmux host](https://github.com/moai-agent/ahu-plugin-cmux) is a standalone Host V2
package for compatible consumers; ahu 0.11.1 does not execute session-host plugins.
Do not grant `interactive-backend` expecting to restore removed core functionality.

## Independent harness updates

Follow the selected plugin's INSTALL.md update sequence. Finish and clean tasks
that require the old package, stage the new version, review/commit only that
binding's version/path/content pin, and run `ahu plugins update ID`. Refresh and
review context pins, then repeat its smoke. Do not replace the core executable or
unrelated plugins. Existing tasks retain frozen package identity: an update cannot
silently redirect their resume or cleanup. Protocol compatibility and the binding's
CLI-version requirement must both hold.

Core's `tests/fixtures/harnesses` adapters are hermetic development fixtures, not
production packages. The former all-harness development installer is retired.

# ahu

Launch repository-defined coding agents in fresh Git worktrees, with each session
in its own cmux workspace. Agent manifests pin the harness and model; ahu
previews the configuration, delivers instructions, and records the launch.

## Install

With Rust and Cargo installed:

```sh
cargo install --git https://github.com/moai-agent/ahu --locked
```

Task sessions require Git, cmux, and the selected harness installed and
authenticated: Claude Code (`claude`), Codex (`codex`), or Antigravity CLI (`agy`).
ahu installs none of these and holds no provider credentials. Read-only help and
launch previews do not require a cmux connection.

## Everyday use

From a Git checkout in a cmux terminal:

```sh
ahu doctor
ahu
```

On first use, setup writes the project's harness order, model rankings, and
catalog pin to `.agents/ahu/config.toml`. Share that policy through Git; ahu
does not stage, commit, or push it.

In the launcher, select an agent by number, `@name`, or bare name. Leave the
selection blank for the project's automatic harness/model choice. The resolved
pair appears before you enter the task. Finish the prompt with a line containing
only `.`, then review the preview and type its confirmation code. Pasting never
submits. Use `.cancel` on its own line to abandon the prompt.

For a scriptable preview in this repository:

```sh
ahu agents
ahu launch @dev-astra --prompt 'Review configuration validation.' \
  --dry-run --allow-widened-approvals
```

To submit an assignment, write it to a UTF-8 file:

```sh
printf '%s\n' 'Review configuration validation and report findings.' > assignment.txt
ahu launch @dev-astra --prompt-file assignment.txt \
  --title "Configuration review" --summary "Check validation and report findings" \
  --allow-widened-approvals
```

`launch` submits without interactive confirmation and keeps focus on the caller.
All five agents in this repository declare `permissions = "auto"`, so this path
requires the explicit approval-widening flag, even for previews. The flag controls
the child's requested settings; it grants no extra access to the caller.

Inspect work from the checkout that launched it:

```sh
ahu tasks
task_id=abc123  # replace with a full task ID from ahu tasks
ahu task "$task_id" --output json
ahu diff "$task_id"
ahu focus "$task_id"
```

A process exit does not prove completion. Review the changes and findings. Tasks
keep their branches, worktrees, and records after the harness exits.

## This repository's agents

| Agent | Harness / model | Role |
| --- | --- | --- |
| `@offsec-astra` | Codex / `gpt-6-astra` | Investigates security issues and reports evidence without editing source. |
| `@defsec-astra` | Codex / `gpt-6-astra` | Reviews defensive programming; implements requested hardening and focused refactoring. |
| `@dev-astra` | Codex / `gpt-6-astra` | Validates reported issues and implements fixes with regression coverage. |
| `@dev-opus` | Claude Code / `claude-opus-5` | General development and code review; hands product documentation to docs-astra. |
| `@docs-astra` | Codex / `gpt-6-astra` | Maintains tracked documentation, specifications, and current project context. |

[Manifests](.agents/ahu/agents/) reference versioned
[instructions](.agents/ahu/instructions/). Named launches use their manifest's
pair independently of automatic project selection. This repository's automatic
choice is Codex / `gpt-6-astra`; it does not select a named agent.

Automatic selection walks the configured harness order, takes the first harness
with an available adapter and a nonempty project model ranking, and selects that
ranking's first model. Local prerequisites are checked afterward. A missing
harness produces an error, never a different selection or catalog fallback.

## What travels with a task

Each task starts at the invoking checkout's HEAD on `ahu/<agent>/<task-id>`, in
`.worktrees/` under the primary checkout. Recognized agent configuration is copied
from the invoking checkout, including uncommitted and ignored files and local
deletions. Unrelated dirty source files stay behind. State defaults to the
launching checkout's ignored `.ahu/state/`; sibling worktrees share a launch lock
and cmux group mapping in the primary checkout.

ahu delivers its delegation contract and agent instructions as prompt text.
They are not an enforced system prompt. The harness controls the running session,
including model changes, approvals, and context loading. ahu reports visible
settings and gaps, does not alter hooks, and never claims its inventory is complete.
Private tracker material stays out of public files and reports; remote pushes
require explicit user permission.

## More commands and reference

| Command | Purpose |
| --- | --- |
| `ahu help` | Options, prompt sources, and exit codes |
| `ahu onboard` | Read-only preview of native definitions available for registration |
| `ahu inventory [@agent]` | Inspect context sources, settings, and coverage gaps |
| `ahu hygiene [@agent]` | Propose context cleanup without changing files |
| `ahu explain` | Built-in architecture overview |
| `ahu explain --open` | Open the overview in cmux's Markdown viewer |
| `ahu codex` | Open a coordinating Codex session in the current terminal |

See the [CLI and context reference](docs/reference.md) for registration, JSON
contracts, prompt transport, hooks, state, and delegation boundaries.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets --locked --offline -- -D warnings
cargo test --locked --offline
```

Tests use temporary repositories and stand-in harness executables. Live cmux
tests are opt-in through `AHU_TEST_CMUX=1`; ordinary test runs do not open cmux
workspaces. Once opted in, unreachable cmux is an error. Run live checks only
with authorization.

```sh
cargo test --locked --offline --doc
cargo test --locked --offline --test explain
cargo run --locked --offline -- explain --mermaid > /tmp/ahu-diagrams.md
```

The explanation tests check diagram structure, not rendering. Full validation
requires parsing the emitted blocks with Mermaid; `ahu explain --open` displays
them in cmux.

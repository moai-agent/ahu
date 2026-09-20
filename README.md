# ahu

Launch repository-defined coding agents in fresh Git worktrees, interactively
in cmux or unattended in headless mode. Agent manifests pin the harness and
model; ahu previews the configuration, delivers instructions, and records the launch.

## Install

With Rust and Cargo installed (this checkout pins Rust in
[`rust-toolchain.toml`](rust-toolchain.toml)):

```sh
cargo install --git https://github.com/moai-agent/ahu --locked
```

On macOS, update an installed `ahu` with `cargo install` or by replacing the
destination with a fresh file. Do not copy a new build over a path while an
`ahu` process may still be running: in-place replacement can leave the path
unable to execute because macOS caches code-signature validation by file identity.
For a locally built release, remove the destination before copying:

```sh
rm /Users/you/.cargo/bin/ahu
cp target/release/ahu /Users/you/.cargo/bin/ahu
```

The running process keeps its old file open, and new invocations use the fresh
file. If an overwritten path already exits with status 137 and no output,
replace it using one of these fresh-file procedures before troubleshooting ahu.

Tasks require Git and the selected harness installed and
authenticated: Claude Code (`claude`), Codex (`codex`), Antigravity CLI (`agy`),
or OpenCode (`opencode`). An OpenCode agent also needs the model's provider
configured in your own OpenCode configuration; see
[OpenCode with Ollama-hosted models](docs/reference.md#opencode-with-ollama-hosted-models).
Interactive sessions also require cmux. Headless tasks need no cmux connection.
Projects that configure knowledge bundles also require the `okf` binary for
[knowledge checks](docs/reference.md#knowledge-checks). Repository agents use
the GitHub CLI (`gh`) for issue tracking; ahu itself never invokes it.
ahu installs none of these and holds no provider credentials. Read-only help and
launch previews do not require a cmux connection. Git and default cmux lookup
require executables outside Git working trees; see the
[lookup rules](docs/reference.md#utility-lookup).

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
Each task receives a short handle derived from its displayed title, such as
`@configuration-review`. Use `--name review` to choose one. Task commands accept
the handle: `ahu task @review`, `ahu focus @review`, or `ahu cancel @review`.
Names belong to the repository and remain reserved after removal; task IDs still work.
The development, security, and documentation agents declare `permissions = "auto"`,
so their launches require the explicit approval-widening flag, even for previews. The flag controls
the child's requested settings; it grants no extra access to the caller.

For unattended execution, use an initialized project and the same registered agent:

```sh
ahu launch @dev-astra --headless --prompt-file assignment.txt \
  --allow-widened-approvals --output json
ahu launch @dev-astra --headless --background --prompt-file assignment.txt \
  --allow-widened-approvals --output json
```

The first command waits in the foreground; the second returns after supervisor
startup. Each launches a separate task. Headless coordination lives in the primary checkout’s private state; native
histories stay in the harness’s own stores. See [headless execution](docs/reference.md#headless-execution) for
supported CLI versions, result collection, resume, cancellation, and limits.
Headless mode covers Claude Code, Codex, Antigravity, and OpenCode; an agent on
any other harness is refused rather than run on one of them.
Headless child launches need host grants (`--allow-child` or
`--allow-child-widened`). Claude Code 2.1.270 also supports bounded native helpers
for entirely read-only assignments; use a separate registered reviewer for that
mode when the coordinator needs to edit or run commands.

Inspect work from the primary checkout or a sibling task worktree. Outside it,
prefix the command with `ahu --repo /path/to/project`. Task commands accept
exact `@name` handles, `ahu:task:<id>`, bare IDs, and unique ID prefixes:

```sh
ahu tasks
task_id=abc123  # replace with a full task ID from ahu tasks
ahu task "$task_id"
ahu result "$task_id"  # headless tasks
ahu diff "$task_id"
ahu focus "$task_id"  # interactive cmux tasks
```

Headless inspection shows the attempt outcome, observed ownership, blockers,
known native session reference, and artifact paths. A process exit does not
prove completion. Review the changes and findings. Tasks keep their branches,
worktrees, and records after the harness exits.

## This repository's agents

| Agent | Harness / model | Role |
| --- | --- | --- |
| `@arch-astra` | Codex / `gpt-6-astra` | Plans implementation with code-grounded designs, dependencies, and verifiable work increments. |
| `@offsec-astra` | Codex / `gpt-6-astra` | Identifies and validates security issues with concrete evidence. |
| `@defsec-astra` | Codex / `gpt-6-astra` | Reviews and implements defensive programming and focused refactoring. |
| `@dev-astra` | Codex / `gpt-6-astra` | Validates and remediates reported issues with regression coverage. |
| `@docs-astra` | Codex / `gpt-6-astra` | Maintains all tracked Markdown, specifications, knowledge formats, and current project context. |
| `@arch-opus` | Claude Code / `claude-opus-5` | Plans implementation with code-grounded designs, dependencies, and verifiable work increments. |
| `@defsec-opus` | Claude Code / `claude-opus-5` | Reviews and implements defensive programming and focused refactoring. |
| `@dev-opus` | Claude Code / `claude-opus-5` | General-purpose ahu development and code review; product documentation belongs to docs-opus. |
| `@docs-opus` | Claude Code / `claude-opus-5` | Maintains all tracked Markdown, specifications, knowledge formats, and current project context. |
| `@arch-glm` | OpenCode / `ollama/glm-5.3:cloud` | Plans implementation with code-grounded designs, dependencies, and verifiable work increments. |
| `@defsec-glm` | OpenCode / `ollama/glm-5.3:cloud` | Reviews and implements defensive programming and focused refactoring. |
| `@dev-glm` | OpenCode / `ollama/glm-5.3:cloud` | Validates and remediates reported issues with regression coverage. |
| `@docs-glm` | OpenCode / `ollama/glm-5.3:cloud` | Maintains all tracked Markdown, specifications, knowledge formats, and current project context. |

[Manifests](.agents/ahu/agents/) are OKF Markdown documents that pair each
agent's identity with its instructions. Named launches use their manifest's
pair independently of automatic project selection. This repository's automatic
choice is Codex / `gpt-6-astra`; it does not select a named agent.

Automatic selection walks the configured harness order, takes the first harness
with an available adapter and a nonempty project model ranking, and selects that
ranking's first model. Local prerequisites are checked afterward. A missing
harness produces an error, never a different selection or catalog fallback.

## Requirements discovery

Use [discover-requirements](.agents/skills/discover-requirements/SKILL.md) when a
request needs clearer outcomes, scope, or acceptance criteria. Ask to use
`discover-requirements` with your idea; it gathers facts and proposes choices in
your preferred channel, keeping discovery records private. Specified tasks proceed
without a mandatory interview. Repository copies are provided for Codex and Claude.

## What travels with a task

Each task starts at the invoking checkout's HEAD on `ahu/<agent>/<task-id>`, in
`.worktrees/` under the primary checkout. Recognized agent configuration is copied
from the invoking checkout, including uncommitted and ignored files and local
deletions. Unrelated dirty source files stay behind. Interactive task records and
prompts live in the worktree’s ignored `.ahu/state/`; removing the worktree removes
that state. Headless coordination and the repository’s task index belong to the
primary checkout’s `.ahu/state/` and survive task worktree deletion. ahu does not
copy native event streams, stderr, final answers, or helper summaries there. See
[state and compatibility](docs/reference.md#state-and-compatibility) for discovery,
legacy records, overrides, and integrity limits.

ahu delivers its contract, available execution facts, agent instructions, and
assignment in nonce-bearing sections with raw bodies. This is prompt text,
not an enforced system prompt. The harness controls the running session,
including model changes, approvals, and context loading. ahu reports visible
settings and gaps and never claims its inventory is complete. Ordinary launches
do not alter native hooks or settings. Explicit `ahu cmux install` delegates a
reviewable native installation to cmux.
Private tracker material stays out of public files and reports; remote pushes
require explicit user permission.

## More commands and reference

| Command | Purpose |
| --- | --- |
| `ahu cmux status` | Inspect native integration evidence and headless isolation |
| `ahu cmux install --harness codex --dry-run` | Preview an explicit native installation |
| `ahu help` | Options, prompt sources, and exit codes |
| `ahu onboard` | Read-only preview of native definitions available for registration |
| `ahu inventory [@agent]` | Inspect context sources, settings, and coverage gaps |
| `ahu hygiene [@agent]` | Propose context cleanup without changing files |
| `ahu knowledge lint` | Validate and lint configured OKF bundles with installed `okf` |
| `ahu explain` | Built-in architecture overview |
| `ahu explain --open` | Open the overview in cmux's Markdown viewer |
| `ahu claude` | Open a coordinating Claude session in the current terminal |
| `ahu codex` | Open a coordinating Codex session in the current terminal |
| `ahu opencode` | Open a coordinating OpenCode session in the current terminal |

`ahu codex` passes `--dangerously-bypass-approvals-and-sandbox`; `ahu claude`
and `ahu agy` pass `--dangerously-skip-permissions`. All three disclose the
flag and retain the
harness's configured model. Registered child agents keep their own manifest
permissions and launch requirements. Inside cmux, coordinator shortcuts place
their workspace in the repository's group before starting. See
[coordinator sessions](docs/reference.md#coordinator-sessions) for details.

See the [CLI and context reference](docs/reference.md) for registration, JSON
contracts, prompt transport, hooks, state, and delegation boundaries.

## Development

The [CI workflow](.github/workflows/ci.yml) defines checks on Ubuntu 24.04 and
macOS 14 using Rust 1.96.0, rustfmt, Clippy, Git, Bash, and Python 3.9 or newer.
From a checkout with rustup installed:

```sh
rustup toolchain install --no-self-update
python3 scripts/check-skills.py
cargo fmt --check
cargo build --locked
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Fresh environments may fetch tools and dependencies. Add `--offline` to Cargo
checks only after dependencies are cached. The separate
[release dependency policy](docs/dependencies.md) defines scanner, review, and
exception requirements.

With OKF 0.5.0 installed outside the checkout, also run:

```sh
cargo run --locked -- knowledge lint
```

Configured bundle validation is a separate maintainer check. Ordinary CI tests
the validator integration with synthetic fixtures; it does not install OKF or
validate the authored bundle. Workflow files alone do not establish hosted
success or required-check enforcement.

The [current-code knowledge bundle](docs/knowledge/index.md) uses OKF v0.2.
Skill maintenance checks compare repository copies byte-for-byte; they do not
install or sync skills globally. See [knowledge checks](docs/reference.md#knowledge-checks)
for project configuration and machine-readable results.

Tests use temporary repositories and stand-in harness executables. Live cmux
tests are opt-in through `AHU_TEST_CMUX=1`; ordinary test runs do not open cmux
workspaces. Once opted in, unreachable cmux is an error. Run live checks only
with authorization.

```sh
cargo test --locked --doc
cargo test --locked --test explain
cargo run --locked -- explain --mermaid > /tmp/ahu-diagrams.md
```

The explanation tests check diagram structure, not rendering. Full validation
requires parsing the emitted blocks with Mermaid; `ahu explain --open` displays
them in cmux.

### Prose linting

Documentation prose is checked with [Vale](https://vale.sh/), configured in
[`.vale.ini`](.vale.ini). Install Vale (`brew install vale`, or see the
[install guide](https://vale.sh/docs/install)), then:

```sh
scripts/lint-prose.sh
```

The first run downloads the Google, Microsoft, write-good, and proselint style
packages into `.vale/styles`, which Git ignores; pass `--no-sync` to reuse an
existing download, and pass paths to check files outside the default set
(`README.md`, `AGENTS.md`, and `docs/`). Terms this project uses deliberately,
such as `ahu`, `cmux`, and `worktree`, are accepted through the tracked
vocabulary at
[`.vale/styles/config/vocabularies/ahu/accept.txt`](.vale/styles/config/vocabularies/ahu/accept.txt).

CI runs the same script in the `prose` job and reports configured alerts. The
configuration turns off the rules that encode the Google and Microsoft editorial
voice, such as required contractions and passive-voice warnings, along with the
Oxford comma rules, which flag two-item conjunctions. The reasons are recorded
beside each entry in `.vale.ini`.

See the [0.2.0 preparation notes](docs/releases/0.2.0.md) for implemented behavior
and validation limits. This version is not published as a release.

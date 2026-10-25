# Codex launch planner

Independent Rust package `ahu-plugin-codex` version 0.1.0. This directory is
its own Cargo workspace, with its own lockfile and only serde/serde_json direct
dependencies. It can be copied into a standalone source repository and built
without ahu or any local path dependency. No published repository or downloadable
binary is asserted. No Python customer runtime or install-time build is required.

The accepted native version is **0.163.0**, invoked as `codex`. The manifest
`tested_versions` and code allowlist agree in unit tests. This is implementation
scope, not all-mode live certification. Unknown, prefixed, prerelease, build-tagged,
and other native versions fail closed. To add native support, release a new
package version with reviewed mappings and version evidence, then update the
package version/content pins; ahu need not be rebuilt once v2 routing is integrated.

Interactive plans use `-m`, `--no-daemon`, and a final `--` before the prompt.
Prompt permissions add nothing; accept-edits adds `--approve-for-me`; auto adds
`--ask-for-approval never --sandbox workspace-write`. `--no-daemon` keeps the
native server embedded in the supervised invocation; it does not certify child
cleanup. Headless uses `exec` (or `exec resume`), `--model`, `--json`,
`-c approval_policy="never"`, and `-c sandbox_mode="danger-full-access"`.
Fresh runs add `--color never`; resume places the session after `--` before the
prompt. Headless does not add `--no-daemon` or interactive approval flags.

The executable only translates settings into an argument plan. It never executes
the native harness, reads native configuration, probes accounts, or receives the
real prompt. Core retains supervision, broker, native stream parsing, MCP/OTel,
configuration and trust preparation, and usage extraction in this preparatory
stage. Config/trust/events/usage extraction has not moved into this package.
All-mode live behavior, provider availability, native child cleanup, and effective
native permissions remain unverified here. Fixture tests are not live certification.

Invoke with exactly `--ahu-harness-launch-protocol=2`. Send one JSON object on
stdin and close stdin. Input is at most 65,536 bytes, including whitespace. All
fields below are required; unknown and duplicate fields fail. No defaults:

```json
{"protocol_version":2,"harness":"codex","mode":"headless","model":"provider/model","permissions":"auto","work_mode":true,"session":null,"native_version":"0.163.0","timeout_seconds":60}
```

Mode is `interactive` or `headless`; permissions are `prompt`, `accept-edits`, or
`auto`. Headless requires work_mode=true and permissions=auto. Guarded headless
requests fail. Interactive requires work_mode=false and session=null; interactive
resume fails. Session is explicit null or 1–128 ASCII bytes starting with an
alphanumeric, then only letters and digits, hyphens and underscores. Model is 1–256
ASCII bytes starting with an alphanumeric, then only letters and digits and `-_.:/`.
Whitespace, controls, NUL, option prefixes, and the reserved placeholder are
rejected. Timeout is an integer in 1–604800 seconds and describes the native task,
not planner runtime. Neither `cwd`, environment, accounts nor prompt is an input.
Core sets `cwd` and `PWD` uniformly for native task processes.

Success is exit 0 and one newline-terminated JSON object, at most 65,536 bytes
including the newline: protocol_version=2, harness, program, `args` (string array),
and `prompt_arg` (zero-based integer). Exactly one final argument is the literal
`<prompt: see prompt.txt>`; `prompt_arg` identifies it for core substitution.
The caller must validate the response before substitution/execution. The planner
has a five-second watchdog, including stalled I/O; the host separately enforces
manifest timeout_ms=5000. Failure is exit 2 with no successful response and only
fixed diagnostics, if any. A broken output stream can leave a partial response;
nonzero exit or incomplete output must be discarded. Timeouts may be silent.

Build and stage from this package directory using Rust 1.96.0 (the validation
toolchain). Build on the destination OS/architecture. Keep the final stage stable
while pinned; dependency license notices must accompany distributed binaries.
No command below launches a native harness or makes a model request.

```sh
set -eu
planner_source=$(pwd -P)
planner_target=$(mktemp -d /tmp/ahu-codex-build.XXXXXX)
export CARGO_TARGET_DIR="$planner_target"
cargo fmt --all --check
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
planner_package=$(mktemp -d /tmp/ahu-codex-package.XXXXXX)
mkdir "$planner_package/bin"
cp "$planner_source/Cargo.toml" "$planner_source/Cargo.lock" \
  "$planner_source/ahu-plugin.md" "$planner_source/README.md" \
  "$planner_source/LICENSE" "$planner_source/SECURITY.md" "$planner_package/"
install -m 755 "$planner_target/release/ahu-plugin-codex" \
  "$planner_package/bin/ahu-plugin-codex"
test -x "$planner_package/bin/ahu-plugin-codex"
printf 'Staged package: %s\n' "$planner_package"
```

Install only with an ahu build that implements the v2 manifest/request contract
and routing for this harness. The prior v1 Codex-only pilot cannot consume this
package. Core integration and end-to-end installation are separate pending work;
the commands below describe that integration's existing binding lifecycle and
must not be represented as validated installation against a v1 host.

After staging, these commands create a disposable ordinary Git project and pin
the finished package. They commit only the synthetic project policy. The stage
must stay available for later updates. Inspect the binding before each commit.
Installation copies verified package contents; it does not compile or run them.

```sh
planner_digest=$(ahu plugins digest "$planner_package")
planner_ahu_version=$(ahu --version)
planner_ahu_version=${planner_ahu_version#ahu }
planner_project=$(mktemp -d /tmp/ahu-codex-project.XXXXXX)
cd "$planner_project"
git init -q
git config user.name 'Plugin Example'
git config user.email 'plugin@example.invalid'
git config core.hooksPath /dev/null
git config commit.gpgsign false
mkdir -p .agents/ahu/plugins
cat > .agents/ahu/plugins/harness-codex.md <<EOF
---
okf_version: "0.2"
type: ahu:plugin-binding
ahu:
  schema_version: 1
  id: harness-codex
  version: "0.1.0"
  source:
    kind: local
    repository: "$planner_package"
    revision: "$planner_digest"
    sha256: "$planner_digest"
  compatibility:
    ahu: "$planner_ahu_version"
    host_api: 1
  enabled: false
  grants: [harness-launch]
---
Synthetic local launch-planner binding.
EOF
ahu lock --update
cat .agents/ahu/plugins/harness-codex.md
git add .agents/ahu/plugins/harness-codex.md .agents/ahu/lock.md
git commit -m 'Pin codex launch planner'
ahu plugins install harness-codex
ahu plugins enable harness-codex
ahu lock --update
git diff -- .agents/ahu/plugins/harness-codex.md .agents/ahu/lock.md
git add .agents/ahu/plugins/harness-codex.md .agents/ahu/lock.md
git commit -m 'Enable codex launch planner'
ahu plugins installed --output json
```

Do not use `plugins invoke`; this is a launch planner, not a generic action.
A compatible core selects the binding for registered Codex tasks. Native
installation, authentication and reviewed native configuration remain operator concerns.
No package build or installation establishes an OS sandbox. Turn off and commit
the binding before `ahu plugins remove harness-codex`; first resolve any tasks
that retain the installation. Retain old pinned packages for recorded attempts.

Contract review: the consumer is ahu's launch planner boundary. A skill cannot
supply a deterministic, bounded `argv` response to core; the old v1 contract lacks
native-version and timeout inputs. This v2 executable receives only settings and
returns data, with no native side effects or delegated authority. Refusals leave
native execution to the caller; native execution outcomes are outside this
protocol. Unit tests cover the mode/permission matrix, exact fresh/resume `argv`,
version evidence, validation, duplicate/missing/unknown fields, and input/output
limits. Executable tests check exit status, fixed errors, and JSON framing using
synthetic inputs, without native binaries or credentials.

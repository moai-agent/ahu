# Contributing to ahu

Read the [Rust style guide](docs/rust-style-guide.md) before changing Rust code.
It covers module boundaries, ownership, errors, lifecycle effects, and tests,
alongside the formatting and lint checks below.

The [CI workflow](.github/workflows/ci.yml) defines checks on Ubuntu 24.04 and
macOS 14 using Rust 1.96.0, rustfmt, Clippy, Git, Bash, and Python 3.9 or newer.
From a checkout with rustup installed:

```sh
rustup toolchain install --no-self-update
umask 077
test ! -L .ahu && test ! -L .ahu/build
mkdir -p .ahu/build
export CARGO_TARGET_DIR="$PWD/.ahu/build/development"
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
Skill maintenance checks validate the skill tree contract and its compiled
bundle; they do not install or sync skills globally. See [knowledge checks](docs/reference.md#knowledge-checks)
for project configuration and machine-readable results.

Tests use temporary repositories and stand-in harness executables. Core CI tests
the plugin contracts without GitHub services. Passing core fixtures does not
establish live harness compatibility.

```sh
cargo test --locked --doc
cargo test --locked --test explain
cargo run --locked -- explain --mermaid > .ahu/ahu-diagrams.md
```

The explanation tests check diagram structure, not rendering. Full validation
requires parsing the emitted blocks with Mermaid.

## Prose linting

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


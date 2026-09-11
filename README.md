# ahu

The Rust command-line interface for moai-agent. The binary is named `ahu`, and its only command is `help`.

## Install

With Rust and Cargo installed:

```sh
cargo install --git https://github.com/moai-agent/ahu --locked
```

## Usage

```sh
ahu help
```

Running `ahu`, `ahu -h`, or `ahu --help` also prints help. Unsupported commands or extra arguments exit with status 2.

## Development

```sh
cargo run -- help
cargo fmt --check
cargo clippy -- -D warnings
```

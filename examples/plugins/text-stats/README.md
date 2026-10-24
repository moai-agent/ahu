# Offline text statistics plugin

This independent authoring example uses the public [executable plugin V1
protocol](../../../docs/executable-plugins.md). It needs `/usr/bin/python3`
(Python 3.5 or later), Git, and an already built `ahu` on PATH. There are no Python
dependencies, build steps, credentials, network calls, `gh`, or `cmux` commands.
`Cargo.toml` supplies the required package identity only.

Ahu sends one JSON request on stdin, closes stdin, and expects one JSON response
and a zero exit status. Only the entrypoint is copied into the execution snapshot;
the script therefore uses only Python's standard library, with no package-relative
assets. Its result depends only on the supplied text, not the invocation ID.
Input must contain exactly one string field, `text`. The result counts Unicode
code points and whitespace-separated words; empty text returns two zeroes.
Invalid payloads produce the protocol's `invalid_input` response (the foreground
CLI reports a plugin error as `failed`).

Run these commands from the Ahu checkout root. They create a disposable ordinary
Git repository, commit only its synthetic binding, and use a local package pin.
They do not change the Ahu checkout. Keep the script executable before hashing it.

```sh
example_package=$(cd examples/plugins/text-stats && pwd -P)
test -x "$example_package/text-stats.py"
example_digest=$(ahu plugins digest "$example_package")
example_ahu_version=$(ahu --version)
example_ahu_version=${example_ahu_version#ahu }
example_repo=$(mktemp -d)
cd "$example_repo"
git init -q
git config user.name 'Plugin Example'
git config user.email 'plugin@example.invalid'
git config core.hooksPath /dev/null
git config commit.gpgsign false
mkdir -p .agents/ahu/plugins
cat > .agents/ahu/plugins/text-stats.md <<EOF
---
okf_version: "0.2"
type: ahu:plugin-binding
ahu:
  schema_version: 1
  id: text-stats
  version: "1.0.0"
  source:
    kind: local
    repository: "$example_package"
    revision: "$example_digest"
    sha256: "$example_digest"
  compatibility:
    ahu: "$example_ahu_version"
    host_api: 1
  enabled: false
  grants: [execute]
---
Synthetic offline authoring example.
EOF
git add .agents/ahu/plugins/text-stats.md
git commit -qm 'Pin offline text statistics example'
ahu plugins install text-stats
ahu plugins enable text-stats
git diff -- .agents/ahu/plugins/text-stats.md
git add .agents/ahu/plugins/text-stats.md
git commit -qm 'Enable offline text statistics example'
printf '%s' '{"text":"hello offline world"}' |
  ahu plugins invoke text-stats --input-file -
ahu plugins disable text-stats
git diff -- .agents/ahu/plugins/text-stats.md
git add .agents/ahu/plugins/text-stats.md
git commit -qm 'Disable offline text statistics example'
printf '%s' '{"text":"hello offline world"}' |
  ahu plugins invoke text-stats --input-file -
# Expected: denied, exit 4, without starting the entrypoint.
ahu plugins remove text-stats
ahu plugins installed --output json
```

The successful invocation returns
`{"schema_version":1,"status":"ok","result":{"characters":19,"words":3}}`
(JSON key order is immaterial). The final inventory has no installations; removal
retains the disabled binding. Enable/disable edit the working binding, so commit
each policy change before relying on it. In projects that also launch agents,
refresh and commit the context lock with those changes as described in the
[installation lifecycle](../../../docs/reference.md#project-local-installation-lifecycle).

The offline integration test exercises this package through the public CLI:

```sh
cargo test --offline --test plugin_authoring
```

It copies the package into temporary storage and adds a test-only startup marker
before computing the pin. The marker proves that installation, inventories,
enable/disable, denied invocation, and removal never start the entrypoint. The
temporary repositories are ordinary Git repositories, not worktrees.

#!/bin/sh
# Development helper: build, stage, pin, install and enable harness plugin
# packages for the project in the current directory. It never pushes, and it
# leaves the resulting bindings and lock uncommitted for review.
#
#   scripts/install-harness-plugins.sh [harness ...]   (default: all five)
#   AHU=/path/to/ahu  PACKAGES=/stage/dir  TARGET=/build/dir  override defaults
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
ahu=${AHU:-ahu}
packages=${PACKAGES:-$(pwd)/.ahu/plugin-packages}
target=${TARGET:-$(pwd)/.ahu/build/harness-plugins}
# Use the same ignored, private local store as the CLI.
if [ ! -e .ahu ]; then mkdir -m 700 .ahu; fi
if [ -L .ahu ] || [ ! -d .ahu ]; then
  echo "Refusing unsafe .ahu directory" >&2; exit 1
fi
if [ -L .ahu/.gitignore ]; then
  echo "Refusing symlinked .ahu/.gitignore" >&2; exit 1
fi
if [ ! -e .ahu/.gitignore ]; then printf '*\n' > .ahu/.gitignore; fi
git check-ignore --quiet .ahu/plugin-packages/probe &&
git check-ignore --quiet .ahu/build/probe || {
  echo "Ignore .ahu/plugin-packages and .ahu/build before staging plugins" >&2; exit 1;
}
[ $# -gt 0 ] || set -- codex claude-code antigravity opencode kiro
# Validate before building or replacing any package. Explicit overrides may be
# external, but no staging or build path may traverse a symlink.
for h in "$@"; do
  case "$h" in
    codex|claude-code|antigravity|opencode|kiro) ;;
    *) echo "Unsupported harness: $h" >&2; exit 1 ;;
  esac
done
for destination in "$packages" "$target"; do
  case "$destination" in
    /*) ;;
    *) echo "Use an absolute staging/build path: $destination" >&2; exit 1 ;;
  esac
  cursor=$destination
  while [ "$cursor" != / ]; do
    if [ -L "$cursor" ]; then
      echo "Refusing symlinked staging/build path: $cursor" >&2; exit 1
    fi
    cursor=$(dirname "$cursor")
  done
done
path=""
for h in "$@"; do path="$path$packages/harness-$h:"; done
for h in "$@"; do
  src="$root/plugins/harness-$h"
  dest="$packages/harness-$h"
  CARGO_TARGET_DIR=$target cargo build --locked --release --manifest-path "$src/Cargo.toml"
  rm -rf "$dest"
  mkdir -p "$dest/bin"
  for f in ahu-plugin.md Cargo.toml Cargo.lock README.md LICENSE SECURITY.md; do
    cp "$src/$f" "$dest/$f"
  done
  cp "$target/release/ahu-plugin-$h" "$dest/bin/"
  binding=".agents/ahu/plugins/harness-$h.md"
  if [ -f "$binding" ]; then
    # Re-pin an existing binding to the freshly staged content.
    digest=$("$ahu" plugins digest "$dest" | grep -oE '[0-9a-f]{64}' | tail -1)
    version=$(sed -n '3s/^version = "\(.*\)"$/\1/p' "$src/Cargo.toml")
    # Quote the YAML path, then escape sed replacement metacharacters.
    source_path=$(printf '%s' "$dest" | sed 's/\\/\\\\/g; s/"/\\"/g; s/[&|]/\\&/g')
    sed -i.bak -E -e "s|^(    repository: ).*|\1\"$source_path\"|" -e "s/^(    (revision|sha256): ).*/\1$digest/" \
      -e "s/^  version: .*/  version: $version/" "$binding"
    rm -f "$binding.bak"
  else
    AHU_PLUGIN_PATH=$path "$ahu" setup --harness-plugin "$dest"
    sed -i.bak 's/^  grants: \[\]$/  grants:\n  - harness-runtime/' "$binding"
    rm -f "$binding.bak"
  fi
done
echo "Pins updated. Review and commit .agents/ahu/plugins, then rerun with INSTALL=1." >&2
if [ "${INSTALL:-0}" = 1 ]; then
  for h in "$@"; do
    AHU_PLUGIN_PATH=$path "$ahu" plugins update "harness-$h" ||
      AHU_PLUGIN_PATH=$path "$ahu" plugins install "harness-$h"
    "$ahu" plugins enable "harness-$h"
  done
  "$ahu" lock --update
fi

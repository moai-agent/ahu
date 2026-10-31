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
packages=${PACKAGES:-$HOME/.local/share/ahu/plugin-packages}
target=${TARGET:-${TMPDIR:-/tmp}/ahu-plugins-target}
[ $# -gt 0 ] || set -- codex claude-code antigravity opencode kiro
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
    sed -i.bak -E "s/^(    (revision|sha256): ).*/\1$digest/" "$binding"
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

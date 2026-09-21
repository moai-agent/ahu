#!/usr/bin/env bash
# Lint repository prose with Vale (https://vale.sh/).
#
# Usage: scripts/lint-prose.sh [path ...]
#
# With no arguments, checks the human-facing documentation. Style packages
# named in .vale.ini are downloaded on demand into .vale/styles, which Git
# ignores; pass --no-sync to skip that download and use what is already there.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if ! command -v vale >/dev/null 2>&1; then
  echo "vale not found. Install it from https://vale.sh/docs/install" >&2
  exit 127
fi

sync=1
paths=()
for arg in "$@"; do
  case "$arg" in
    --no-sync) sync=0 ;;
    *) paths+=("$arg") ;;
  esac
done

if [ "${#paths[@]}" -eq 0 ]; then
  paths=(README.md AGENTS.md docs)
fi

if [ "$sync" -eq 1 ]; then
  vale sync
fi

exec vale "${paths[@]}"

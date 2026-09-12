#!/usr/bin/env python3
"""Check that repository skill copies match their canonical source."""

from pathlib import Path
import sys


def files(root: Path) -> dict[str, bytes]:
    if root.is_symlink() or not root.is_dir():
        raise ValueError(f"missing directory or symlink: {root}")
    result = {}
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"skill symlinks are not supported: {path}")
        if path.is_file():
            result[path.relative_to(root).as_posix()] = path.read_bytes()
        elif not path.is_dir():
            raise ValueError(f"unsupported skill entry: {path}")
    if not any(name.endswith("/SKILL.md") for name in result):
        raise ValueError(f"no skills found: {root}")
    return result


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    try:
        canonical = files(root / ".agents/skills")
        deployed = files(root / ".claude/skills")
        differences = [
            name for name in sorted(canonical.keys() | deployed.keys())
            if canonical.get(name) != deployed.get(name)
        ]
        if differences:
            for name in differences:
                print(f"skill copy mismatch: {name!r}", file=sys.stderr)
            return 1
    except (OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        return 1
    print("Repository skill copies match .agents/skills.")
    return 0


if __name__ == "__main__":
    sys.exit(main())

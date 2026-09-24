#!/usr/bin/env python3
"""Provider-free skill fixture and metadata recorder; never runs a harness."""

import argparse
import json
from pathlib import Path
import re
import subprocess

LOCATIONS = (".agents/skills", ".claude/skills", ".gemini/antigravity-cli/skills")
HARNESS = ("codex", "opencode", "claude-code", "antigravity")
SKILL = """---
name: probe-skill
description: Test-only skill for harness discovery probes.
---

If invoked, respond with exactly: PROBE_SKILL_OK
"""


def version(value: str) -> str:
    # Record a release number, never arbitrary CLI output or local paths.
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.-]+)?", value):
        raise argparse.ArgumentTypeError("expected a release number, e.g. 0.155.1")
    return value


def fixture(parent: Path, location: str) -> None:
    # Require a new directory: no overwrites, inherited files, or ambiguous roots.
    parent.mkdir(parents=False, exist_ok=False)
    subprocess.run(["git", "init", "-q", "--template=", str(parent)], check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    skill = parent / location / "probe-skill" / "SKILL.md"
    skill.parent.mkdir(parents=True)
    skill.write_text(SKILL, encoding="utf-8")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    create = commands.add_parser("fixture", help="create a new single-location git fixture")
    create.add_argument("directory", type=Path)
    create.add_argument("--location", choices=LOCATIONS, default=LOCATIONS[0])
    record = commands.add_parser("record", help="emit operator-observed metadata as JSON")
    record.add_argument("--harness", choices=HARNESS, required=True)
    record.add_argument("--version", type=version, required=True)
    record.add_argument("--location", choices=LOCATIONS, default=LOCATIONS[0])
    record.add_argument("--discovery", choices=("discovered", "not-observed", "not-tested"),
                        required=True)
    record.add_argument("--trust", choices=("trusted", "untrusted", "unknown"), required=True)
    record.add_argument("--invocation", choices=("sentinel-observed", "failed", "not-tested"),
                        required=True)
    args = parser.parse_args()
    if args.command == "fixture":
        try:
            fixture(args.directory, args.location)
        except (OSError, subprocess.CalledProcessError):
            parser.exit(1, "could not create fixture; use a new directory outside the repository\n")
        return
    if args.invocation == "sentinel-observed" and args.discovery != "discovered":
        parser.error("sentinel-observed requires discovered; record unresolved evidence separately")
    prerequisites = {
        "codex": "not-established",
        "opencode": "not-established",
        "claude-code": "git-repository-and-session-trust",
        "antigravity": "folder-trust-untested",
    }
    print(json.dumps({
        "schema_version": 1,
        "evidence": "operator-observed",
        "harness": args.harness,
        "harness_version": args.version,
        "location": args.location,
        "git_repository": True,
        "trust_prerequisite": prerequisites[args.harness],
        "trust": args.trust,
        "discovery": args.discovery,
        "invocation": args.invocation,
    }, sort_keys=True))


if __name__ == "__main__":
    main()

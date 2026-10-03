#!/usr/bin/env python3
"""Enforce line coverage for ahu's evaluation and MCP service code."""

import json
import sys
from collections.abc import Callable


def is_eval(path: str) -> bool:
    return (
        path.endswith("/src/eval.rs")
        or path.endswith("/src/eval_otel.rs")
        or path.endswith("/src/telemetry.rs")
        or "/src/eval/" in path
    )


def is_mcp(path: str) -> bool:
    return path.endswith(
        (
            "/src/approval.rs",
            "/src/mcp.rs",
            "/src/mcp_decisions.rs",
            "/src/mcp_tasks.rs",
            "/src/telemetry.rs",
        )
    )


def coverage(files: list[dict], label: str, select: Callable[[str], bool]) -> tuple[int, int]:
    selected = [file["summary"]["lines"] for file in files if select(file["filename"])]
    covered = sum(item["covered"] for item in selected)
    total = sum(item["count"] for item in selected)
    if not total:
        raise ValueError(f"coverage report contains no {label} source files")
    return covered, total


def main() -> int:
    report = json.load(sys.stdin)
    files = report["data"][0]["files"]
    failed = False
    for label, select in (("eval + OTel", is_eval), ("MCP tools + OTel", is_mcp)):
        covered, total = coverage(files, label, select)
        percent = covered / total * 100
        print(f"{label}: {covered}/{total} lines ({percent:.2f}%), required 95.00%")
        failed |= percent < 95.0
    return int(failed)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (KeyError, IndexError, TypeError, ValueError, json.JSONDecodeError) as error:
        print(f"coverage report check failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error

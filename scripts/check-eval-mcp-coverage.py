#!/usr/bin/env python3
"""Enforce workspace, evaluation/OTel, and MCP/OTel release coverage."""

import json
import sys
from collections.abc import Callable


def is_eval(path: str) -> bool:
    return (
        path.endswith("/src/eval.rs")
        or path.endswith("/src/eval_otel.rs")
        or path.endswith("/src/otel_report.rs")
        or path.endswith("/src/telemetry.rs")
        or "/src/eval/" in path
    )


def is_mcp(path: str) -> bool:
    return path.endswith(
        (
            "/src/approval.rs",
            "/src/mcp.rs",
            "/src/mcp_decisions.rs",
            "/src/mcp_skills.rs",
            "/src/mcp_tasks.rs",
            "/src/telemetry.rs",
        )
    )


def counts(lines: dict) -> tuple[int, int]:
    covered, total = lines["covered"], lines["count"]
    if type(covered) is not int or type(total) is not int or not 0 <= covered <= total:
        raise ValueError("invalid line coverage counts")
    return covered, total


def coverage(files: list[dict], label: str, select: Callable[[str], bool]) -> tuple[int, int]:
    selected = [counts(file["summary"]["lines"]) for file in files if select(file["filename"])]
    covered = sum(item[0] for item in selected)
    total = sum(item[1] for item in selected)
    if not total:
        raise ValueError(f"coverage report contains no {label} source files")
    return covered, total


def main() -> int:
    report = json.load(sys.stdin)
    files = report["data"][0]["files"]
    covered, total = counts(report["data"][0]["totals"]["lines"])
    if not total:
        raise ValueError("coverage report contains no workspace lines")
    print(f"workspace: {covered}/{total} lines ({covered / total * 100:.2f}%), required >90.00%")
    failed = covered * 100 <= total * 90
    for label, select in (("eval + OTel", is_eval), ("MCP tools + OTel", is_mcp)):
        covered, total = coverage(files, label, select)
        percent = covered / total * 100
        print(f"{label}: {covered}/{total} lines ({percent:.2f}%), required 95.00%")
        failed |= covered * 100 < total * 95
    return int(failed)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (KeyError, IndexError, TypeError, ValueError, json.JSONDecodeError) as error:
        print(f"coverage report check failed: {error}", file=sys.stderr)
        raise SystemExit(2) from error

#!/usr/bin/env python3
"""Score synthetic local-agent runs without retaining task or decision payloads."""

from __future__ import annotations

import argparse
import json
import statistics
import sys
from datetime import datetime, timezone
from pathlib import Path


REPO = Path(__file__).resolve().parents[1]


def read_json(path: Path):
    with path.open(encoding="utf-8") as stream:
        return json.load(stream)


def score(case: dict, answer: dict, result: dict, decision_calls: int | None, elapsed_ms: int | None) -> dict:
    expected = case["expected"]
    weights = case["scoring"]
    parts = {}
    for name, wanted in expected.items():
        observed = answer.get(name)
        correct = observed == wanted
        if isinstance(wanted, dict) and "minimum" in wanted:
            correct = isinstance(observed, (int, float)) and not isinstance(observed, bool) and observed >= wanted["minimum"]
        parts[name] = (1.0 if correct else 0.0) * weights[name]
    value = sum(parts.values())
    metrics = result.get("metrics", {}).get("values", {})
    usage = {
        name: entry["value"]
        for name, entry in metrics.items()
        if isinstance(entry, dict) and entry.get("kind") == "observed"
    }
    return {
        "schema_version": 1,
        "recorded_at": datetime.now(timezone.utc).isoformat(),
        "case_id": case["id"],
        "corpus_version": case["corpus_version"],
        "task_id": result.get("task_id"),
        "attempt": result.get("attempt"),
        "outcome": result.get("outcome", "unknown"),
        "score": round(value, 4),
        "passed": value >= weights["exact_match_pass_threshold"],
        "score_parts": parts,
        "reported_tokens": usage,
        "decision_call_count": decision_calls,
        "elapsed_ms": elapsed_ms,
        "skill_observation": result.get("harness", {}).get("skill_observation"),
    }


def record(args) -> int:
    output = args.output.expanduser().resolve()
    if output == REPO or REPO in output.parents:
        raise ValueError("run records must be stored outside the repository")
    case = read_json(args.case)
    answer = read_json(args.answer)
    result = read_json(args.result)
    if not isinstance(answer, dict):
        raise ValueError("answer artifact must be a JSON object")
    row = score(case, answer, result, args.decision_calls, args.elapsed_ms)
    row.update({
        "run_id": args.run_id,
        "stage": args.stage,
        "agent": args.agent,
        "agent_version": args.agent_version,
        "evaluator": args.evaluator,
        "evaluator_version": args.evaluator_version,
        "model": args.model, "harness": args.harness,
        "harness_version": args.harness_version,
        "ahu_revision": args.ahu_revision,
        "skill_digest": args.skill_digest,
    })
    if args.trace_id:
        row["trace_id"] = args.trace_id
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("a", encoding="utf-8") as stream:
        stream.write(json.dumps(row, sort_keys=True) + "\n")
    print(json.dumps({key: row[key] for key in ("case_id", "task_id", "score", "passed", "outcome")}))
    return 0


def trend(args) -> int:
    rows = [json.loads(line) for line in args.records.expanduser().read_text(encoding="utf-8").splitlines() if line.strip()]
    groups: dict[tuple, list[dict]] = {}
    for row in rows:
        groups.setdefault((
            row["case_id"], row.get("stage") or "candidate",
            row.get("agent") or "unspecified", row.get("agent_version") or "unspecified",
            row.get("evaluator") or "unspecified", row.get("evaluator_version") or "unspecified",
            row["model"], row["harness"], row.get("harness_version") or "unspecified",
            row.get("ahu_revision") or "unspecified", row.get("skill_digest") or "unspecified",
        ), []).append(row)
    summaries = []
    for (case_id, stage, agent, agent_version, evaluator, evaluator_version, model, harness,
         harness_version, ahu_revision, skill_digest), items in sorted(groups.items()):
        scores = [item["score"] for item in items]
        summaries.append({
            "case_id": case_id, "stage": stage, "model": model, "harness": harness,
            "agent": agent, "agent_version": agent_version,
            "evaluator": evaluator, "evaluator_version": evaluator_version,
            "harness_version": harness_version,
            "ahu_revision": ahu_revision, "skill_digest": skill_digest,
            "runs": len(items), "mean_score": round(statistics.mean(scores), 4),
            "pass_rate": round(sum(item["passed"] for item in items) / len(items), 4),
            "token_observations": sum(bool(item["reported_tokens"]) for item in items),
            "timing_observations": sum(item.get("elapsed_ms") is not None for item in items),
            "decision_call_observations": sum(item.get("decision_call_count") is not None for item in items),
        })
    print(json.dumps(summaries, indent=2))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    add = commands.add_parser("record", help="score one ahu result and append a compact external record")
    add.add_argument("--case", type=Path, default=REPO / "evals/cases/decision-routing.json")
    add.add_argument("--answer", type=Path, required=True, help="synthetic answer artifact produced by the agent")
    add.add_argument("--result", type=Path, required=True, help="JSON from ahu result --output json")
    add.add_argument("--model", required=True)
    add.add_argument("--agent", default="unspecified", help="registered ahu candidate, such as @triage")
    add.add_argument("--agent-version", default="unspecified")
    add.add_argument("--evaluator", default="unspecified", help="registered ahu evaluator when agent judging is used")
    add.add_argument("--evaluator-version", default="unspecified")
    add.add_argument("--run-id")
    add.add_argument("--stage", choices=("candidate", "evaluator"), default="candidate")
    add.add_argument("--harness", required=True)
    add.add_argument("--harness-version", default="unspecified")
    add.add_argument("--ahu-revision", default="unspecified")
    add.add_argument("--skill-digest", default="unspecified")
    add.add_argument("--trace-id", help="OTel trace ID for this task/attempt")
    add.add_argument("--decision-calls", type=int, help="aggregate count from a verified tool-call observation")
    add.add_argument("--elapsed-ms", type=int, help="elapsed time read from the matching OTel span")
    add.add_argument("--output", type=Path, required=True, help="JSONL path outside this repository")
    add.set_defaults(run=record)
    show = commands.add_parser("trend", help="summarize score and coverage by case, model, and harness")
    show.add_argument("--records", type=Path, required=True)
    show.set_defaults(run=trend)
    args = parser.parse_args()
    try:
        return args.run(args)
    except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        print(f"local_eval: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())

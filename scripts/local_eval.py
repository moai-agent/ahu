#!/usr/bin/env python3
"""Score synthetic local-agent runs without retaining task or decision payloads."""

from __future__ import annotations

import argparse
import hashlib
import json
import statistics
import sys
from datetime import datetime, timezone
from pathlib import Path

try:
    import yaml
except ImportError:  # The Rust runner has no Python dependency.
    yaml = None

REPO = Path(__file__).resolve().parents[1]


def read_json(path: Path):
    with path.open(encoding="utf-8") as stream:
        return json.load(stream)


def read_case(path: Path):
    if yaml is None:
        raise ValueError(
            "reading OKF Markdown cases with this helper requires PyYAML; "
            "install requirements-evals.txt"
        )
    text = path.read_text(encoding="utf-8")
    if not text.startswith("---\n"):
        raise ValueError("evaluation case must be OKF Markdown with YAML front matter")
    frontmatter_text, separator, body = text[4:].partition("\n---\n")
    if not separator:
        raise ValueError("evaluation case is missing its closing front matter delimiter")
    frontmatter = yaml.safe_load(frontmatter_text)
    if (
        not isinstance(frontmatter, dict)
        or frontmatter.get("okf_version") != "0.2"
        or frontmatter.get("type") != "ahu:eval-case"
        or frontmatter.get("schema_version") != 2
    ):
        raise ValueError("evaluation case must use schema_version 2 and declare okf_version 0.2 and type ahu:eval-case")
    if not body.strip():
        raise ValueError("evaluation case Markdown body must describe the case")
    return frontmatter


def score(case: dict, answer: dict, result: dict, decision_calls: int | None, elapsed_ms: int | None, case_digest: str) -> dict:
    expected = case["expected"]
    weights = case["scoring"]
    if not isinstance(expected, dict) or not isinstance(answer, dict) or answer.keys() != expected.keys():
        raise ValueError("answer must contain exactly the case's expected question keys")
    question_weights = {key: value for key, value in weights.items() if key != "exact_match_pass_threshold"}
    threshold = weights.get("exact_match_pass_threshold")
    if question_weights.keys() != expected.keys() or any(
        not isinstance(value, (int, float)) or isinstance(value, bool) or value < 0
        for value in question_weights.values()
    ) or not isinstance(threshold, (int, float)) or isinstance(threshold, bool) or not 0 <= threshold <= 1:
        raise ValueError("case scoring weights or pass threshold are invalid")
    total_weight = sum(question_weights.values())
    if total_weight <= 0:
        raise ValueError("case scoring weights must sum to more than zero")
    questions = case.get("questions", {})
    if not isinstance(questions, dict) or questions.keys() != expected.keys():
        raise ValueError("case questions must match expected answer keys")
    for name, question in questions.items():
        actual = answer[name]
        kind = question.get("type") if isinstance(question, dict) else None
        if kind == "choice":
            options = question.get("options")
            valid = isinstance(options, dict) and actual in options
        elif kind == "probability":
            valid = isinstance(actual, (int, float)) and not isinstance(actual, bool) and 0 <= actual <= 1
        elif kind == "score":
            valid = (isinstance(actual, (int, float)) and not isinstance(actual, bool)
                     and question.get("min") <= actual <= question.get("max"))
        else:
            valid = False
        if not valid:
            raise ValueError(f"answer for {name!r} does not match its declared question type")
    parts = {}
    for name, wanted in expected.items():
        observed = answer.get(name)
        correct = observed == wanted
        if isinstance(wanted, dict) and "minimum" in wanted:
            correct = isinstance(observed, (int, float)) and not isinstance(observed, bool) and observed >= wanted["minimum"]
        parts[name] = (1.0 if correct else 0.0) * weights[name]
    value = sum(parts.values()) / total_weight
    metrics = result.get("metrics", {}).get("values", {})
    usage = {
        name: entry["value"]
        for name, entry in metrics.items()
        if (isinstance(entry, dict) and entry.get("kind") == "observed"
            and isinstance(entry.get("value"), (int, float)) and not isinstance(entry.get("value"), bool))
        or (isinstance(entry, (int, float)) and not isinstance(entry, bool))
    }
    expectations = case.get("tool_expectations")
    has_tool_expectations = bool(expectations and (expectations.get("required") or expectations.get("forbidden")))
    return {
        "schema_version": 2,
        "recorded_at": datetime.now(timezone.utc).isoformat(),
        "case_id": case["id"],
        "corpus_version": case["corpus_version"],
        "case_schema_version": 2,
        "case_digest": case_digest,
        "prompt_profile": "manual_unverified",
        "prompt_version": None,
        "scoring_version": 2,
        "input_fingerprint": None,
        "fingerprint_completeness": "partial",
        "task_id": result.get("task_id"),
        "attempt": result.get("attempt"),
        "outcome": result.get("outcome", "unknown"),
        "score": round(value, 4),
        "passed": value >= weights["exact_match_pass_threshold"],
        "answer_score": round(value, 4),
        "answer_passed": value >= weights["exact_match_pass_threshold"],
        "score_source": "manual_deterministic",
        "judge_status": "not_run",
        "tool_expectation_status": "unknown" if has_tool_expectations else "not_applicable",
        "tool_expectation_required": (expectations or {}).get("required", []),
        "tool_expectation_forbidden": (expectations or {}).get("forbidden", []),
        # This helper records optional manual counts; it does not collect OTLP.
        "telemetry_coverage": "none",
        "score_parts": parts,
        "reported_tokens": usage,
        "mcp_observed": False,
        "decision_call_count": decision_calls,
        "elapsed_ms": elapsed_ms,
        "skill_observation": result.get("harness", {}).get("skill_observation"),
    }


def record(args) -> int:
    output = args.output.expanduser().resolve()
    if output == REPO or REPO in output.parents:
        raise ValueError("run records must be stored outside the repository")
    case = read_case(args.case)
    case_digest = hashlib.sha256(args.case.read_bytes()).hexdigest()
    answer = read_json(args.answer)
    result = read_json(args.result)
    if not isinstance(answer, dict):
        raise ValueError("answer artifact must be a JSON object")
    row = score(case, answer, result, args.decision_calls, args.elapsed_ms, case_digest)
    row.update({
        "run_id": args.run_id,
        "stage": args.stage,
        "agent": args.agent,
        "agent_version": args.agent_version,
        "evaluator": args.evaluator,
        "evaluator_version": args.evaluator_version,
        "evaluator_model": args.evaluator_model,
        "evaluator_harness": args.evaluator_harness,
        "evaluator_harness_version": args.evaluator_harness_version,
        "model": args.model, "harness": args.harness,
        "harness_version": args.harness_version,
        "skill_digest": args.skill_digest,
    })
    fingerprint_inputs = {
        key: row.get(key)
        for key in (
            "case_digest", "case_id", "corpus_version", "prompt_profile",
            "agent", "agent_version", "model", "harness", "harness_version",
            "skill_digest", "stage",
        )
    }
    row["input_fingerprint"] = hashlib.sha256(
        json.dumps(fingerprint_inputs, sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    if args.trace_id:
        row["trace_id"] = args.trace_id
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("a", encoding="utf-8") as stream:
        stream.write(json.dumps(row, sort_keys=True) + "\n")
    print(json.dumps({key: row[key] for key in ("case_id", "task_id", "score", "passed", "outcome")}))
    return 0


# Manual records use the current schema, but remain visibly partial because this
# path cannot establish the prompt, agent definition, ahu build, or full tool
# telemetry used by `ahu eval run`.
MANUAL_RECORD_SCHEMA_VERSION = 2


def reject_unsupported_records(rows: list[dict]) -> None:
    for number, row in enumerate(rows, start=1):
        version = row.get("schema_version")
        if not isinstance(version, int) or version != MANUAL_RECORD_SCHEMA_VERSION:
            raise ValueError(
                f"line {number}: record schema_version {version!r} is not the current schema "
                f"({MANUAL_RECORD_SCHEMA_VERSION}). Use `ahu eval report --records ...` for current records."
            )


def trend(args) -> int:
    rows = [json.loads(line) for line in args.records.expanduser().read_text(encoding="utf-8").splitlines() if line.strip()]
    reject_unsupported_records(rows)
    groups: dict[tuple, list[dict]] = {}
    for row in rows:
        groups.setdefault((
            row["case_id"], row.get("corpus_version") or "unspecified",
            row.get("stage") or "candidate",
            row.get("agent") or "unspecified", row.get("agent_version") or "unspecified",
            row.get("evaluator") or "unspecified", row.get("evaluator_version") or "unspecified",
            row.get("evaluator_model") or "unspecified",
            row.get("evaluator_harness") or "unspecified",
            row.get("evaluator_harness_version") or "unspecified",
            row["model"], row["harness"], row.get("harness_version") or "unspecified",
            row.get("case_digest") or "unspecified", row.get("prompt_profile") or "unspecified",
            row.get("input_fingerprint") or "unspecified",
            row.get("fingerprint_completeness") or "unspecified",
            row.get("skill_digest") or "unspecified",
            row.get("evaluator_skill_digest") or "unspecified",
            row.get("evaluator_repo_head") or "unspecified",
        ), []).append(row)
    summaries = []
    for (case_id, corpus_version, stage, agent, agent_version, evaluator, evaluator_version,
         evaluator_model, evaluator_harness, evaluator_harness_version, model, harness,
         harness_version, case_digest, prompt_profile, input_fingerprint,
         fingerprint_completeness, skill_digest, evaluator_skill_digest, evaluator_repo_head), items in sorted(groups.items()):
        scores = [item["score"] for item in items if isinstance(item.get("score"), (int, float))]
        token_fields = set()
        token_observations = 0
        for item in items:
            metrics = item.get("reported_tokens")
            if isinstance(metrics, dict):
                observed = {name for name, entry in metrics.items()
                            if (isinstance(entry, dict) and entry.get("kind") == "observed"
                                and isinstance(entry.get("value"), (int, float)) and not isinstance(entry.get("value"), bool))
                            or (isinstance(entry, (int, float)) and not isinstance(entry, bool))}
                if observed:
                    token_observations += 1
                    token_fields.update(observed)
        measured = [item for item in items if item.get("mcp_observed") is True]

        def mean_field(field, subset=items):
            values = [item[field] for item in subset if item.get(field) is not None]
            return round(statistics.mean(values), 4) if values else None

        known_tools = ("ahu_agents_list", "ahu_tasks_list", "ahu_task_get", "ahu_typed_decide")
        tool_values = {name: [] for name in known_tools}
        for item in measured:
            named = item.get("mcp_tools")
            if not isinstance(named, dict):
                continue
            fully_named = (isinstance(item.get("mcp_tool_call_count"), (int, float))
                           and sum(value for value in named.values()
                                   if isinstance(value, (int, float)) and not isinstance(value, bool)
                                   ) == item["mcp_tool_call_count"])
            for name in known_tools:
                if name in named and isinstance(named[name], (int, float)) and not isinstance(named[name], bool):
                    tool_values[name].append(named[name])
                elif fully_named:
                    tool_values[name].append(0)
        summaries.append({
            "case_id": case_id, "corpus_version": corpus_version,
            "stage": stage, "model": model, "harness": harness,
            "agent": agent, "agent_version": agent_version,
            "evaluator": evaluator, "evaluator_version": evaluator_version,
            "evaluator_model": evaluator_model,
            "evaluator_harness": evaluator_harness,
            "evaluator_harness_version": evaluator_harness_version,
            "harness_version": harness_version,
            "case_digest": case_digest, "prompt_profile": prompt_profile,
            "input_fingerprint": input_fingerprint,
            "fingerprint_completeness": fingerprint_completeness,
            "skill_digest": skill_digest,
            "evaluator_skill_digest": evaluator_skill_digest,
            "evaluator_repo_head": evaluator_repo_head,
            "runs": len(items), "score_observations": len(scores),
            "mean_score": round(statistics.mean(scores), 4) if scores else None,
            "pass_rate": round(sum(item["passed"] for item in items) / len(items), 4),
            "token_observations": token_observations,
            "token_fields": sorted(token_fields),
            "timing_observations": sum(item.get("elapsed_ms") is not None for item in items),
            "decision_call_observations": sum(item.get("decision_call_count") is not None for item in items),
            "mcp_observations": len(measured),
            "mean_mcp_requests": mean_field("mcp_request_count", measured),
            "mean_mcp_tool_lists": mean_field("mcp_tool_list_count", measured),
            "mean_mcp_tool_calls": mean_field("mcp_tool_call_count", measured),
            "mean_mcp_tool_errors": mean_field("mcp_tool_error_count", measured),
            "mean_typed_decision_errors": mean_field("typed_decision_error_count", measured),
            "mcp_tools": {
                name: round(statistics.mean(values), 4)
                for name in known_tools
                if (values := tool_values[name])
            } if measured else {},
        })
    print(json.dumps(summaries, indent=2))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    add = commands.add_parser("record", help="score one ahu result and append a compact external record")
    add.add_argument("--case", type=Path, default=REPO / "evals/cases/decision-routing.md")
    add.add_argument("--answer", type=Path, required=True, help="synthetic answer artifact produced by the agent")
    add.add_argument("--result", type=Path, required=True, help="JSON from ahu result --output json")
    add.add_argument("--model", required=True)
    add.add_argument("--agent", default="unspecified", help="registered ahu candidate, such as @triage")
    add.add_argument("--agent-version", default="unspecified")
    add.add_argument("--evaluator", default="unspecified", help="registered ahu evaluator when agent judging is used")
    add.add_argument("--evaluator-version", default="unspecified")
    add.add_argument("--evaluator-model", default="unspecified")
    add.add_argument("--evaluator-harness", default="unspecified")
    add.add_argument("--evaluator-harness-version", default="unspecified")
    add.add_argument("--run-id")
    add.add_argument("--stage", choices=("candidate", "evaluator"), default="candidate")
    add.add_argument("--harness", required=True)
    add.add_argument("--harness-version", default="unspecified")
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

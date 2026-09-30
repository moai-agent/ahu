#!/usr/bin/env python3
"""Offline structural checks and actual ahu parser refusal, without a launch."""
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent
AHU = sys.argv[1] if len(sys.argv) > 1 else "ahu"
GUARD = 'evaluation candidate "@corpus-parse-only" is not a registered ahu agent'

def document(path):
    text = path.read_text()
    assert text.startswith("---\n"), path
    front, body = text[4:].split("\n---\n", 1)
    assert body.strip(), path
    return json.loads(front), body.strip()

def refused(source, stdin=None, malformed=False):
    result = subprocess.run(
        [AHU, "eval", "run", *source, "--agent", "@corpus-parse-only",
         "--records", "/tmp/ahu-selection-never-written.jsonl"],
        cwd=ROOT.parents[1], input=stdin, text=True, capture_output=True, timeout=15,
    )
    if malformed:
        assert result.returncode == 2 and "evaluation case schema" in result.stderr
        assert GUARD not in result.stderr, result.stderr
    else:
        assert result.returncode == 2 and GUARD in result.stderr, result.stderr

def main():
    listing = subprocess.run([AHU, "agents"], cwd=ROOT.parents[1],
                             text=True, capture_output=True, timeout=15)
    assert listing.returncode == 0, listing.stderr
    assert "@corpus-parse-only" not in listing.stdout, "Refusing: validation guard is registered"
    paths = sorted((ROOT / "cases").glob("*.md"))
    assert len(paths) == 12
    skills = sorted((ROOT / "fixture/.agents/skills").glob("*/SKILL.md"))
    assert len(skills) == 12
    skill_paths = {p.relative_to(ROOT / "fixture").as_posix() for p in skills}
    allowed = skill_paths | {"AGENTS.md"}
    fixture_files = {p.relative_to(ROOT / "fixture").as_posix()
                     for p in (ROOT / "fixture").rglob("*") if p.is_file()}
    assert fixture_files == allowed, fixture_files
    assert not any(p.is_symlink() for p in ROOT.rglob("*"))
    for skill in skills:
        text = skill.read_text()
        front, body = text[4:].split("\n---\n", 1)
        name_line, description_line = front.splitlines()
        assert name_line == "name: " + skill.parent.name
        assert len(json.loads(description_line.removeprefix("description: "))) > 80
        assert len(body.strip()) > 100
    reference = json.loads((ROOT / "reference.json").read_text())
    labels = reference["cases"]
    assert reference["corpus_version"] == "1.0.0"
    seen, required_union, counts = set(), set(), []
    question_total = 0
    for path in paths:
        case, _ = document(path)
        identity = case["id"]
        assert identity not in seen
        seen.add(identity)
        assert case["schema_version"] == 2 and case["corpus_version"] == "1.0.0"
        assert case["type"] == "ahu:eval-case" and case["okf_version"] == "0.2"
        assert set(case["questions"]) == set(case["expected"])
        assert set(case["scoring"]) == set(case["questions"]) | {"exact_match_pass_threshold"}
        assert all(value == 1.0 for value in case["scoring"].values())
        assert "tool_expectations" not in case and "rubric" not in case
        for key, question in case["questions"].items():
            assert question["type"] == "choice" and question["instructions"]
            assert len(question["options"]) >= 2
            assert case["expected"][key] in question["options"]
        question_total += len(case["questions"])
        label = labels[identity]
        required, useful = label["required"], label["useful"]
        assert len(required) == len(set(required)) <= 3
        assert len(useful) == len(set(useful))
        assert set(required).isdisjoint(useful)
        assert (set(required) | set(useful)) <= skill_paths
        assert label["rationale"]
        required_union.update(required)
        counts.append(len(required))
        refused(["--case", str(path)])
    assert seen == set(labels)
    assert counts.count(1) == 6 and counts.count(0) == 3
    assert sum(n > 1 for n in counts) == 3 and required_union == skill_paths
    assert question_total == 26
    suite_path = ROOT / "suites/skill-selection.md"
    suite, _ = document(suite_path)
    assert suite["schema_version"] == 1 and suite["type"] == "ahu:eval-suite"
    assert suite["version"] == "1.0.0"
    assert len(suite["cases"]) == 12
    assert {entry["path"] for entry in suite["cases"]} == {
        "../cases/" + path.name for path in paths}
    assert all(entry["weight"] == 1.0 for entry in suite["cases"])
    refused(["--suite", str(suite_path)])
    bad, _ = document(paths[0])
    bad["expected"] = {}
    refused(["--case", "/dev/stdin"],
            "---\n" + json.dumps(bad) + "\n---\n\nNegative control.\n", malformed=True)
    print("PASS: 12 skills, 12 schema-2 cases, schema-1 suite, 26 decisions; "
          "labels and fixture boundary valid; actual parser preflights and negative control passed. "
          "No provider launched.")

if __name__ == "__main__":
    main()

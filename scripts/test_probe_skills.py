"""Synthetic checks only: no harness, credentials, prompts, or transcripts."""

import json
from pathlib import Path
import subprocess
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("probe-skills.py")


def run(*args):
    return subprocess.run(["python3", "-B", str(SCRIPT), *args],
                          capture_output=True, text=True)


class ProbeSkillsTests(unittest.TestCase):
    def test_single_location_fixture_and_no_overwrite(self):
        for location in (".agents/skills", ".claude/skills", ".gemini/antigravity-cli/skills"):
            with self.subTest(location=location), tempfile.TemporaryDirectory() as directory:
                target = Path(directory) / "fixture"
                result = run("fixture", str(target), "--location", location)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue((target / ".git").is_dir())
                skills = list(target.rglob("SKILL.md"))
                self.assertEqual(skills, [target / location / "probe-skill/SKILL.md"])
                self.assertIn("PROBE_SKILL_OK", skills[0].read_text())
                self.assertNotEqual(run("fixture", str(target)).returncode, 0)

    def test_four_harness_records_are_metadata_only(self):
        for harness, prerequisite in (
            ("codex", "not-established"), ("opencode", "not-established"),
            ("claude-code", "git-repository-and-session-trust"),
            ("antigravity", "folder-trust-untested"),
        ):
            with self.subTest(harness=harness):
                result = run("record", "--harness", harness, "--version", "1.2.3",
                             "--discovery", "not-observed", "--trust", "unknown",
                             "--invocation", "not-tested")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout), {
                    "schema_version": 1, "evidence": "operator-observed",
                    "harness": harness, "harness_version": "1.2.3",
                    "location": ".agents/skills", "git_repository": True,
                    "trust_prerequisite": prerequisite, "trust": "unknown",
                    "discovery": "not-observed", "invocation": "not-tested",
                })

    def test_records_reject_free_text_and_inconsistent_success(self):
        base = ["record", "--harness", "codex", "--version", "0.155.1",
                "--discovery", "discovered", "--trust", "trusted",
                "--invocation", "sentinel-observed"]
        self.assertEqual(run(*base).returncode, 0)
        for option, value in (("--version", "/private/local/path"),
                              ("--trust", "transcript text"),
                              ("--discovery", "not-observed"),
                              ("--location", "../../other")):
            args = base.copy()
            if option in args:
                args[args.index(option) + 1] = value
            else:
                args.extend([option, value])
            self.assertNotEqual(run(*args).returncode, 0)


if __name__ == "__main__":
    unittest.main()

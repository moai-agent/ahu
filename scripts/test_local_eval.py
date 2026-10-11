import importlib.util
import contextlib
import io
import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock


SPEC = importlib.util.spec_from_file_location(
    "local_eval", Path(__file__).with_name("local_eval.py")
)
local_eval = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(local_eval)


class LocalEvalRecordTests(unittest.TestCase):
    def test_manual_counts_do_not_claim_otel_observation(self):
        case = {
            "id": "synthetic-case",
            "corpus_version": "1.0.0",
            "expected": {"route": "billing"},
            "questions": {"route": {"type": "choice", "options": {"billing": "Payments", "technical": "Product"}}},
            "scoring": {"route": 1.0, "exact_match_pass_threshold": 1.0},
        }
        row = local_eval.score(
            case,
            {"route": "billing"},
            {},
            decision_calls=2,
            elapsed_ms=None,
            case_digest="synthetic-digest",
        )
        self.assertEqual(row["decision_call_count"], 2)
        self.assertEqual(row["telemetry_coverage"], "none")
        self.assertFalse(row["mcp_observed"])
        self.assertEqual(row["score"], 1.0)

    def test_weighted_score_is_normalized_and_invalid_probability_is_rejected(self):
        case = {
            "id": "weighted", "corpus_version": "1.0.0",
            "expected": {"route": "billing", "confidence": {"minimum": 0.8}},
            "questions": {
                "route": {"type": "choice", "options": {"billing": "Payments", "technical": "Product"}},
                "confidence": {"type": "probability"},
            },
            "scoring": {"route": 1.0, "confidence": 1.0, "exact_match_pass_threshold": 1.0},
        }
        row = local_eval.score(case, {"route": "billing", "confidence": 0.4}, {}, None, None, "d")
        self.assertEqual(row["score"], 0.5)
        self.assertFalse(row["passed"])
        with self.assertRaises(ValueError):
            local_eval.score(case, {"route": "billing", "confidence": 100}, {}, None, None, "d")


class LocalEvalToolTrendTests(unittest.TestCase):
    def test_skill_retrieval_counts_survive_summary(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "records.jsonl"
            path.write_text(json.dumps({
                "schema_version": local_eval.MANUAL_RECORD_SCHEMA_VERSION,
                "case_id": "fixture", "model": "fixture", "harness": "fixture",
                "score": 1, "passed": True, "mcp_observed": True,
                "mcp_tool_call_count": 3,
                "mcp_tools": {"ahu_skills_list": 1, "ahu_skills_get": 2},
            }) + "\n")
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(local_eval.trend(type("Args", (), {"records": path})()), 0)
            tools = json.loads(output.getvalue())[0]["mcp_tools"]
            self.assertEqual(tools["ahu_skills_list"], 1)
            self.assertEqual(tools["ahu_skills_get"], 2)
            self.assertEqual(tools["ahu_typed_decide"], 0)


class LocalEvalStorageTests(unittest.TestCase):
    def setUp(self):
        env = {key: value for key, value in os.environ.items()
               if key not in ("GIT_DIR", "GIT_COMMON_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")}
        env.update(GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_SYSTEM="/dev/null")
        environment = mock.patch.dict(os.environ, env, clear=True)
        environment.start()
        self.addCleanup(environment.stop)
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.repo = self.root / "repo"
        self.repo.mkdir()
        self.git("init", "-q")
        self.patch = mock.patch.object(local_eval, "REPO", self.repo)
        self.patch.start()
        self.addCleanup(self.patch.stop)
        self.local = self.repo / ".ahu/evals/runs.jsonl"

    def git(self, *args):
        return subprocess.run(
            ["git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgSign=false",
             "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
             "-C", str(self.repo), *args],
            check=True, capture_output=True,
            env={**os.environ, "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_SYSTEM": "/dev/null"},
        )

    def ignore(self):
        (self.repo / ".gitignore").write_text("/.ahu/evals/\n")

    def test_local_output_requires_ignore_and_primary_index_remains_authoritative(self):
        with self.assertRaisesRegex(ValueError, "ignored"):
            local_eval.record_output(self.local)
        self.ignore()
        self.assertEqual(local_eval.record_output(self.local), self.local)
        self.assertFalse(self.local.parent.exists(), "validation alone creates nothing")
        local_eval.append_record(self.local, {"schema_version": 2, "score": 1})
        self.git("add", "-f", ".ahu/evals/runs.jsonl")
        with self.assertRaisesRegex(ValueError, "tracked"):
            local_eval.append_record(self.local, {"score": 0})
        self.local.unlink()
        with self.assertRaisesRegex(ValueError, "tracked"):
            local_eval.record_output(self.local)
        self.git("commit", "--allow-empty", "-qm", "synthetic fixture")
        child = self.root / "worktree"
        self.git("worktree", "add", "--detach", str(child), "HEAD")
        with mock.patch.object(local_eval, "REPO", child):
            # The primary ignore rule is uncommitted and absent from the child.
            self.assertEqual(local_eval.record_output(self.local.with_name("other.jsonl")), self.local.with_name("other.jsonl"))
            with self.assertRaisesRegex(ValueError, "tracked"):
                local_eval.record_output(self.local)
            with self.assertRaises(ValueError):
                local_eval.record_output(child / ".ahu/evals/runs.jsonl")

    def test_alternate_index_cannot_authorize_local_output(self):
        self.ignore()
        with mock.patch.dict(os.environ, {"GIT_INDEX_FILE": str(self.root / "alternate-index")}):
            with self.assertRaisesRegex(ValueError, "overrides"):
                local_eval.record_output(self.local)
        self.assertFalse(self.local.parent.exists())

    def test_append_is_private_preserves_jsonl_and_external_0644(self):
        self.ignore()
        row = {"schema_version": 2, "score": 1, "input_fingerprint": "fixture"}
        for path in (self.local, self.root / "external/nested/runs.jsonl"):
            with self.subTest(path=path):
                local_eval.append_record(path, row)
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
                self.assertEqual(stat.S_IMODE(path.parent.stat().st_mode), 0o700)
                path.chmod(0o644)
                local_eval.append_record(path, {**row, "score": 0})
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o644)
                self.assertEqual([json.loads(line) for line in path.read_text().splitlines()], [row, {**row, "score": 0}])
                before = path.read_bytes()
                with mock.patch.object(local_eval, "MAX_RECORDS_BYTES", len(before)):
                    with self.assertRaisesRegex(ValueError, "size limit"):
                        local_eval.append_record(path, row)
                self.assertEqual(path.read_bytes(), before)
        self.assertEqual(list(self.local.parent.iterdir()), [self.local])

    def test_rejects_arbitrary_repository_paths_negated_ignores_and_literal_tracked_names(self):
        self.ignore()
        for relative in ("runs.jsonl", ".ahu/evals-other/runs.jsonl", ".ahu/state/runs.jsonl", ".ahu/evals/../runs.jsonl"):
            with self.subTest(relative=relative), self.assertRaises(ValueError):
                local_eval.record_output(self.repo / relative)
        literal = self.local.with_name("[run].jsonl")
        local_eval.append_record(literal, {})
        self.git("--literal-pathspecs", "add", "-f", ".ahu/evals/[run].jsonl")
        with self.assertRaisesRegex(ValueError, "tracked"):
            local_eval.record_output(literal)
        (self.repo / ".gitignore").write_text("/.ahu/evals/*\n!/.ahu/evals/public.jsonl\n")
        with self.assertRaisesRegex(ValueError, "ignored"):
            local_eval.record_output(self.local.with_name("public.jsonl"))

    def test_parent_aliases_are_compatible_but_leaf_and_repository_redirects_are_rejected(self):
        self.ignore()
        external = self.root / "external"
        external.mkdir()
        alias = self.root / "alias"
        alias.symlink_to(external, target_is_directory=True)
        local_eval.append_record(alias / "runs.jsonl", {})
        self.assertEqual((external / "runs.jsonl").read_text(), "{}\n")
        (external / "link.jsonl").symlink_to(external / "runs.jsonl")
        with self.assertRaises(ValueError):
            local_eval.append_record(alias / "link.jsonl", {})
        (external / "dangling.jsonl").symlink_to(external / "absent")
        with self.assertRaises(ValueError):
            local_eval.record_output(external / "dangling.jsonl")
        (self.repo / ".ahu").symlink_to(external, target_is_directory=True)
        with self.assertRaises(ValueError):
            local_eval.record_output(self.local)
        (self.repo / ".ahu").unlink()
        alias.unlink()
        alias.symlink_to(self.repo, target_is_directory=True)
        with self.assertRaises(ValueError):
            local_eval.record_output(alias / ".ahu/evals/runs.jsonl")
        alias.unlink()
        alias.symlink_to(self.repo / ".ahu", target_is_directory=True)
        (self.repo / ".ahu").symlink_to(external, target_is_directory=True)
        with self.assertRaises(ValueError):
            local_eval.record_output(alias / "runs.jsonl")

    def test_unsafe_files_and_directory_modes_are_refused_without_modification(self):
        self.ignore()
        local_eval.append_record(self.local, {})
        for mode in (0o666, 0o4755):
            self.local.chmod(mode)
            with self.assertRaises(ValueError):
                local_eval.append_record(self.local, {})
            self.assertEqual(self.local.read_text(), "{}\n")
        self.local.chmod(0o644)
        hardlink = self.local.with_name("hardlink.jsonl")
        os.link(self.local, hardlink)
        with self.assertRaises(ValueError):
            local_eval.record_output(hardlink)
        hardlink.unlink()
        fifo = self.local.with_name("fifo.jsonl")
        os.mkfifo(fifo)
        with self.assertRaises(ValueError):
            local_eval.append_record(fifo, {})
        self.local.parent.chmod(0o777)
        with self.assertRaises(ValueError):
            local_eval.append_record(self.local, {})
        self.assertEqual(stat.S_IMODE(self.local.parent.stat().st_mode), 0o777)
        self.local.parent.chmod(0o700)
        (self.local.parent / ".git").mkdir()
        with self.assertRaisesRegex(ValueError, "nested checkout"):
            local_eval.record_output(self.local)

    @unittest.skipUnless(sys.platform == "darwin", "macOS system aliases")
    def test_macos_tmp_and_var_aliases(self):
        for parent in ("/tmp", "/var/tmp"):
            with tempfile.TemporaryDirectory(dir=parent) as directory:
                path = Path(directory) / "runs.jsonl"
                local_eval.append_record(path, {})
                self.assertEqual(local_eval.record_output(path), path.resolve())
                self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)

    def test_record_command_projects_bounded_output_and_retains_explicit_output(self):
        self.ignore()
        case_path = self.root / "case.md"
        case_path.write_text("synthetic case")
        case = {"id": "fixture", "corpus_version": "1", "expected": {"route": "billing"},
                "questions": {"route": {"type": "choice", "options": {"billing": "Payments"}}},
                "scoring": {"route": 1, "exact_match_pass_threshold": 1}}
        answer = self.root / "answer.json"
        answer.write_text('{"route":"billing"}')
        result = self.root / "result.json"
        result.write_text('{"outcome":"succeeded","raw_prompt":"do not copy"}')
        argv = ["local_eval.py", "record", "--case", str(case_path), "--answer", str(answer),
                "--result", str(result), "--model", "fixture", "--harness", "fixture"]
        with mock.patch.object(sys, "argv", argv), contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                local_eval.main()
        with mock.patch.object(local_eval, "read_case", return_value=case), contextlib.redirect_stdout(io.StringIO()):
            for _ in range(2):
                with mock.patch.object(sys, "argv", argv + ["--output", str(self.local)]):
                    self.assertEqual(local_eval.main(), 0)
        rows = [json.loads(line) for line in self.local.read_text().splitlines()]
        self.assertEqual(len(rows), 2)
        self.assertEqual(rows[0]["input_fingerprint"], rows[1]["input_fingerprint"])
        self.assertEqual(rows[0]["schema_version"], 2)
        self.assertTrue(rows[0]["passed"])
        self.assertNotIn("raw_prompt", rows[0])
        self.assertNotIn("answer", rows[0])


if __name__ == "__main__":
    unittest.main()

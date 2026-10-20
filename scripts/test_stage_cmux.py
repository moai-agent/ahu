"""Offline staging failure/containment checks; no cmux or Cargo execution."""

from contextlib import redirect_stderr
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location("stage_cmux", Path(__file__).with_name("stage-cmux.py"))
stage_cmux = importlib.util.module_from_spec(spec)
spec.loader.exec_module(stage_cmux)


class StagingTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="cmux-staging-test-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.output = self.root / "package with spaces"
        self.target = self.root / "build with spaces"

    def build(self, command, **kwargs):
        self.assertIn("--locked", command)
        self.assertIn("--release", command)
        self.assertEqual(command[command.index("--target-dir") + 1], str(self.target))
        self.assertEqual(kwargs["env"]["CARGO_BUILD_BUILD_DIR"], str(self.target / "build"))
        executable = self.target / "configured-triple/release/ahu-cmux-host"
        executable.parent.mkdir(parents=True, exist_ok=True)
        executable.write_bytes(b"synthetic binary")
        executable.chmod(0o755)
        return subprocess.CompletedProcess(command, 0, json.dumps({
            "reason": "compiler-artifact", "target": {"name": "ahu-cmux-host"},
            "executable": str(executable),
        }) + "\n")

    def test_stages_reported_artifact_and_preserves_source_manifest(self):
        manifest = stage_cmux.ROOT / "plugins/cmux/ahu-plugin.md"
        original = manifest.read_bytes()
        with patch.object(stage_cmux.subprocess, "run", side_effect=self.build):
            stage_cmux.stage(str(self.output), str(self.target))
            stage_cmux.stage(str(self.root / "second-package"), str(self.target))
        self.assertEqual(manifest.read_bytes(), original)
        self.assertEqual((self.output / "bin/ahu-cmux-host").read_bytes(), b"synthetic binary")
        self.assertTrue(os.access(self.output / "bin/ahu-cmux-host", os.X_OK))
        self.assertEqual(sorted(p.name for p in self.output.iterdir()), ["Cargo.toml", "ahu-plugin.md", "bin"])
        source = tomllib.loads((stage_cmux.ROOT / "plugins/cmux/Cargo.toml").read_text())
        staged = tomllib.loads((self.output / "Cargo.toml").read_text())
        self.assertEqual(staged["package"]["version"], source["package"]["version"])
        self.assertIn("  path: bin/ahu-cmux-host\n  protocol_version: 1", (self.output / "ahu-plugin.md").read_text())

    def test_refuses_bad_paths_before_build(self):
        checkout = self.root / "checkout"
        checkout.mkdir()
        (checkout / ".git").write_text("gitdir: /synthetic/common.git")
        alias = self.root / "alias"
        alias.symlink_to(checkout, target_is_directory=True)
        self.output.mkdir()
        cases = [
            ("relative", str(self.target)),
            (str(self.root / "new"), "relative"),
            (str(stage_cmux.ROOT / "output"), str(self.target)),
            (str(alias / "package"), str(self.target)),
            (str(self.root / "new"), str(alias / "build")),
            (str(self.output), str(self.target)),
            (str(self.target), str(self.target)),
            (str(self.target / "package"), str(self.target)),
            (str(self.root / "outer"), str(self.root / "outer/build")),
        ]
        with patch.object(stage_cmux.subprocess, "run") as run:
            for output, target in cases:
                with self.subTest(output=output, target=target), self.assertRaises(ValueError):
                    stage_cmux.stage(output, target)
            run.assert_not_called()

    def test_build_failure_leaves_no_package(self):
        diagnostics = io.StringIO()
        with patch.object(stage_cmux.subprocess, "run", side_effect=subprocess.CalledProcessError(42, "cargo")):
            with patch.object(sys, "argv", ["stage-cmux", "--output", str(self.output), "--target-dir", str(self.target)]):
                with redirect_stderr(diagnostics), self.assertRaises(SystemExit) as raised:
                    stage_cmux.main()
        self.assertEqual(raised.exception.code, 1)
        self.assertIn("Cargo build failed (exit 42); package not staged", diagnostics.getvalue())
        self.assertFalse(self.output.exists())

    def test_missing_artifact_leaves_no_package(self):
        with patch.object(stage_cmux.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "")):
            with self.assertRaisesRegex(ValueError, "did not report"):
                stage_cmux.stage(str(self.output), str(self.target))
        self.assertFalse(self.output.exists())

    def test_copy_failure_cleans_partial_stage(self):
        with patch.object(stage_cmux.subprocess, "run", side_effect=self.build):
            with patch.object(stage_cmux.shutil, "copyfile", side_effect=OSError("copy failed")):
                with self.assertRaises(OSError):
                    stage_cmux.stage(str(self.output), str(self.target))
        self.assertFalse(self.output.exists())
        self.assertEqual(list(self.root.glob(".cmux-stage-*")), [])

    def test_bad_cli_arguments(self):
        for args in [[], ["--unknown"], ["--output"]]:
            result = subprocess.run([sys.executable, "-B", str(Path(stage_cmux.__file__)), *args], capture_output=True, text=True)
            self.assertEqual(result.returncode, 2)
            self.assertIn("usage:", result.stderr)


if __name__ == "__main__":
    unittest.main()

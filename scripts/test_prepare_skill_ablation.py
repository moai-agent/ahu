#!/usr/bin/env python3
"""Local integration tests for prepare_skill_ablation.py."""

from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("prepare_skill_ablation.py").resolve()
SKILL_ROOTS = (".agents/skills", ".claude/skills", ".opencode/skills")


def git(root: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=root, check=True, text=True, stdout=subprocess.PIPE
    ).stdout.strip()


class SkillAblationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="ahu-skill-ablation-test-")
        self.root = Path(self.temporary.name)
        self.repo = self.root / "source"
        self.repo.mkdir()
        git(self.repo, "init", "-b", "main")
        git(self.repo, "config", "user.name", "Test User")
        git(self.repo, "config", "user.email", "test@example.invalid")
        for skill_root in SKILL_ROOTS:
            for name in ("alpha", "beta"):
                path = self.repo / skill_root / name / "SKILL.md"
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(f"---\nname: {name}\ndescription: fixture\n---\n{name}\n")
        (self.repo / "ahu.lock").write_text("before\n")
        git(self.repo, "add", ".")
        git(self.repo, "add", "-f", *SKILL_ROOTS)
        git(self.repo, "commit", "-m", "fixture")
        self.ahu = self.root / "ahu-fixture"
        self.ahu.write_text(
            "#!/usr/bin/env python3\n"
            "import pathlib, sys\n"
            "assert sys.argv[1] == '--repo'\n"
            "pathlib.Path(sys.argv[2], 'ahu.lock').write_text('refreshed\\n')\n"
        )
        self.ahu.chmod(0o755)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def prepare(self, destination: Path, *options: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [
                "python3", str(SCRIPT), "--repo", str(self.repo),
                "--worktree", str(destination), "--branch", "eval/test-control",
                "--ahu", str(self.ahu), *options,
            ],
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )

    def test_all_project_skills_are_removed_and_locked_in_control_commit(self) -> None:
        destination = self.root / "control"
        result = self.prepare(destination, "--all-project-skills")
        self.assertEqual(result.returncode, 0, result.stderr)
        for skill_root in SKILL_ROOTS:
            self.assertFalse((destination / skill_root).exists())
            self.assertTrue((self.repo / skill_root / "alpha" / "SKILL.md").is_file())
        self.assertEqual((destination / "ahu.lock").read_text(), "refreshed\n")
        self.assertEqual(git(destination, "status", "--porcelain"), "")
        self.assertIn("ahu.lock", git(destination, "show", "--format=", "--name-only", "HEAD"))

    def test_single_skill_removal_preserves_other_skill_files(self) -> None:
        destination = self.root / "control"
        result = self.prepare(destination, "--skill", "alpha")
        self.assertEqual(result.returncode, 0, result.stderr)
        for skill_root in SKILL_ROOTS:
            self.assertFalse((destination / skill_root / "alpha").exists())
            self.assertTrue((destination / skill_root / "beta" / "SKILL.md").is_file())
        self.assertEqual(git(destination, "status", "--porcelain"), "")

    def test_dirty_source_is_refused_before_worktree_creation(self) -> None:
        (self.repo / "uncommitted.txt").write_text("change\n")
        destination = self.root / "control"
        result = self.prepare(destination, "--all-project-skills")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("source checkout must be clean", result.stderr)
        self.assertFalse(destination.exists())

    def test_ignored_harness_context_is_refused(self) -> None:
        (self.repo / ".claude").mkdir(exist_ok=True)
        (self.repo / ".claude/settings.local.json").write_text("{}\n")
        with (self.repo / ".gitignore").open("a") as stream:
            stream.write(".claude/settings.local.json\n")
        git(self.repo, "add", ".gitignore")
        git(self.repo, "commit", "-m", "ignore local settings")
        destination = self.root / "control"
        result = self.prepare(destination, "--all-project-skills")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("ignored harness context", result.stderr)
        self.assertFalse(destination.exists())

    def test_skill_path_symlink_cannot_remove_files_outside_the_worktree(self) -> None:
        external = self.root / "external-skills"
        external.mkdir()
        marker = external / "keep.txt"
        marker.write_text("preserve\n")
        git(self.repo, "rm", "-r", ".agents")
        (self.repo / ".agents").symlink_to(external, target_is_directory=True)
        git(self.repo, "add", ".agents")
        git(self.repo, "commit", "-m", "symlink project skill root")
        destination = self.root / "control"
        result = self.prepare(destination, "--all-project-skills")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("refusing to traverse symlink", result.stderr)
        self.assertEqual(marker.read_text(), "preserve\n")


if __name__ == "__main__":
    unittest.main()

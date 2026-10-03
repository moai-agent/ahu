#!/usr/bin/env python3
"""Local integration tests for prepare_skill_ablation.py."""

from __future__ import annotations

import os
import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("prepare_skill_ablation.py").resolve()
SKILL_ROOTS = (".agents/skills", ".claude/skills", ".opencode/skills")


def fixture_env() -> dict[str, str]:
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")
    return env


def git(root: Path, *args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=root, env=fixture_env(), check=True, text=True, stdout=subprocess.PIPE
    ).stdout.strip()


class SkillAblationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="ahu-skill-ablation-test-")
        self.root = Path(self.temporary.name)
        self.repo = self.root / "source"
        self.repo.mkdir()
        self.env = fixture_env()
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
            "import os, pathlib, sys\n"
            "state = pathlib.Path(os.environ['XDG_STATE_HOME'])\n"
            "assert state.is_dir() and not state.is_relative_to(pathlib.Path(sys.argv[2]))\n"
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
            env=self.env,
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

    def test_baseline_commit_index_and_lock_are_unchanged(self) -> None:
        before = (git(self.repo, "rev-parse", "HEAD"), git(self.repo, "write-tree"))
        destination = self.root / "control"
        result = self.prepare(destination, "--skill", "alpha")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(before, (git(self.repo, "rev-parse", "HEAD"), git(self.repo, "write-tree")))
        self.assertEqual((self.repo / "ahu.lock").read_text(), "before\n")
        self.assertEqual(git(self.repo, "status", "--porcelain"), "")
        self.assertEqual(git(destination, "rev-parse", "HEAD^"), before[0])
        self.assertNotEqual(git(destination, "rev-parse", "HEAD"), before[0])

    def test_checkout_hooks_and_commit_signing_do_not_run(self) -> None:
        hooks = self.root / "hooks"
        hooks.mkdir()
        marker = self.root / "unexpected-hook"
        hook = hooks / "post-checkout"
        hook.write_text(f"#!/bin/sh\ntouch '{marker}'\n")
        hook.chmod(0o755)
        git(self.repo, "config", "core.hooksPath", str(hooks))
        signer = self.root / "unexpected-signer"
        signer.write_text(f"#!/bin/sh\ntouch '{marker}'\nexit 1\n")
        signer.chmod(0o755)
        git(self.repo, "config", "commit.gpgSign", "true")
        git(self.repo, "config", "gpg.program", str(signer))
        result = self.prepare(self.root / "control", "--all-project-skills")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(marker.exists())

    def test_dangling_destination_symlink_is_refused(self) -> None:
        destination = self.root / "control"
        target = self.root / "unexpected-destination"
        destination.symlink_to(target, target_is_directory=True)
        result = self.prepare(destination, "--all-project-skills")
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(destination.is_symlink())
        self.assertFalse(target.exists())

    def test_lock_symlink_is_refused_before_worktree_creation(self) -> None:
        marker = self.root / "external-lock"
        marker.write_text("preserve\n")
        (self.repo / "ahu.lock").unlink()
        (self.repo / "ahu.lock").symlink_to(marker)
        git(self.repo, "add", "ahu.lock")
        git(self.repo, "commit", "-m", "symlink lock fixture")
        destination = self.root / "control"
        result = self.prepare(destination, "--all-project-skills")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(marker.read_text(), "preserve\n")
        self.assertFalse(destination.exists())

    def test_no_matching_skill_is_refused_before_worktree_creation(self) -> None:
        destination = self.root / "control"
        result = self.prepare(destination, "--skill", "missing")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no project skill files matched", result.stderr)
        self.assertFalse(destination.exists())

    def test_unrelated_lock_tool_edits_are_not_committed(self) -> None:
        with self.ahu.open("a") as stream:
            stream.write("pathlib.Path(sys.argv[2], 'unrelated.txt').write_text('unexpected')\n")
        destination = self.root / "control"
        result = self.prepare(destination, "--all-project-skills")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unexpected changes", result.stderr)
        self.assertEqual(git(destination, "rev-parse", "HEAD"), git(self.repo, "rev-parse", "HEAD"))

    def test_lock_refresh_cannot_restore_removed_skills(self) -> None:
        with self.ahu.open("a") as stream:
            stream.write("pathlib.Path(sys.argv[2], '.agents/skills/alpha').mkdir(parents=True)\n")
        destination = self.root / "control"
        result = self.prepare(destination, "--skill", "alpha")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("recreated removed skills", result.stderr)
        self.assertEqual(git(destination, "rev-parse", "HEAD"), git(self.repo, "rev-parse", "HEAD"))

    def test_leaf_skill_symlink_is_removed_without_following_it(self) -> None:
        external = self.root / "external-skill"
        external.mkdir()
        marker = external / "SKILL.md"
        marker.write_text("preserve\n")
        git(self.repo, "rm", "-r", ".agents/skills/alpha")
        (self.repo / ".agents/skills/alpha").symlink_to(external, target_is_directory=True)
        git(self.repo, "add", ".agents/skills/alpha")
        git(self.repo, "commit", "-m", "leaf symlink fixture")
        destination = self.root / "control"
        result = self.prepare(destination, "--skill", "alpha")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(marker.read_text(), "preserve\n")
        self.assertFalse((destination / ".agents/skills/alpha").is_symlink())
        self.assertTrue((self.repo / ".agents/skills/alpha").is_symlink())

    def test_inherited_index_override_is_refused(self) -> None:
        external_index = self.root / "external-index"
        external_index.write_bytes((self.repo / ".git/index").read_bytes())
        before = external_index.read_bytes()
        self.env["GIT_INDEX_FILE"] = str(external_index)
        destination = self.root / "control"
        result = self.prepare(destination, "--all-project-skills")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unset Git repository/index overrides", result.stderr)
        self.assertEqual(external_index.read_bytes(), before)
        self.assertFalse(destination.exists())

    def test_failed_lock_refresh_leaves_uncommitted_arm_and_preserves_baseline(self) -> None:
        self.ahu.write_text("#!/bin/sh\nexit 7\n")
        destination = self.root / "control"
        baseline = git(self.repo, "rev-parse", "HEAD")
        result = self.prepare(destination, "--all-project-skills")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(git(destination, "rev-parse", "HEAD"), baseline)
        self.assertNotEqual(git(destination, "status", "--porcelain"), "")
        self.assertEqual(git(self.repo, "status", "--porcelain"), "")
        self.assertEqual((self.repo / "ahu.lock").read_text(), "before\n")

    def test_skill_path_traversal_is_refused_before_worktree_creation(self) -> None:
        for name in ("..", "../alpha", "/alpha", "alpha/beta", "-alpha"):
            with self.subTest(name=name):
                destination = self.root / "control"
                result = self.prepare(destination, "--skill=" + name)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(destination.exists())

    @unittest.skipUnless(os.environ.get("AHU_TEST_BIN"), "set AHU_TEST_BIN to exercise real lock refresh")
    def test_real_lock_refresh_records_only_control_skills(self) -> None:
        self.ahu = Path(os.environ["AHU_TEST_BIN"]).resolve()
        destination = self.root / "control"
        result = self.prepare(destination, "--skill", "alpha")
        self.assertEqual(result.returncode, 0, result.stderr)
        lock = (destination / "ahu.lock").read_text()
        self.assertIn(".agents/skills/beta/SKILL.md", lock)
        self.assertNotIn(".agents/skills/alpha/SKILL.md", lock)
        self.assertEqual(git(destination, "status", "--porcelain"), "")
        self.assertEqual((self.repo / "ahu.lock").read_text(), "before\n")
        self.assertEqual(git(self.repo, "status", "--porcelain"), "")

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

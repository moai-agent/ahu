#!/usr/bin/env python3
"""Prepare a committed, project-skill-free checkout for an ahu eval arm.

This only prepares the control checkout. Run the same `ahu eval run` command
against the original checkout and the generated worktree, then combine their
JSONL records with `ahu eval report`.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


SKILL_ROOTS = (".agents/skills", ".claude/skills", ".opencode/skills")
CONFIG_ROOTS = (".agents", ".claude", ".codex", ".agent", ".opencode", ".gemini")
CONFIG_FILE_NAMES = {
    b"AGENTS.md", b"AGENTS.override.md", b"CLAUDE.md", b"CLAUDE.local.md",
    b"GEMINI.md", b"GEMINI.local.md", b".mcp.json", b"opencode.json", b"opencode.jsonc",
}


def run(argv: list[str], *, cwd: Path | None = None, env: dict[str, str] | None = None, strip: bool = True) -> str:
    # Worktree checkout invokes post-checkout too. Apply this to every Git
    # operation, not just the final commit, and never invoke a user's signer.
    if argv[0] == "git":
        argv = ["git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgSign=false", *argv[1:]]
    result = subprocess.run(
        argv,
        cwd=cwd,
        env=env,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return result.stdout.strip() if strip else result.stdout


def remove_path(path: Path) -> bool:
    if path.is_symlink() or path.is_file():
        path.unlink()
        return True
    if path.is_dir():
        shutil.rmtree(path)
        return True
    return False


def confined_target(root: Path, path: Path) -> Path:
    """Refuse paths that traverse a repository-controlled symlink or file."""
    relative = path.relative_to(root)
    current = root
    for component in relative.parts[:-1]:
        current = current / component
        if current.is_symlink():
            raise RuntimeError(f"refusing to traverse symlink in skill path: {current.relative_to(root)}")
        if current.exists() and not current.is_dir():
            raise RuntimeError(f"refusing non-directory in skill path: {current.relative_to(root)}")
    return path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd(), help="prepared ahu checkout (default: current directory)")
    parser.add_argument("--worktree", type=Path, required=True, help="new control worktree path; must not exist")
    parser.add_argument("--branch", required=True, help="new local branch name for the control arm")
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--all-project-skills", action="store_true", help="remove skills under the supported project skill roots")
    group.add_argument("--skill", help="remove one skill by directory name from all supported project roots")
    parser.add_argument("--ahu", default="ahu", help="ahu executable used to refresh the control lock (default: ahu on PATH)")
    args = parser.parse_args()

    try:
        if any(name in os.environ for name in ("GIT_DIR", "GIT_COMMON_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE")):
            raise RuntimeError("unset Git repository/index overrides before preparing an ablation arm")
        repo = Path(run(["git", "rev-parse", "--show-toplevel"], cwd=args.repo)).resolve()
        if run(["git", "status", "--porcelain", "--untracked-files=all"], cwd=repo):
            raise RuntimeError("the source checkout must be clean before preparing an ablation arm")
        head = run(["git", "rev-parse", "HEAD"], cwd=repo)
        requested_destination = args.worktree.expanduser()
        if requested_destination.is_symlink() or requested_destination.exists():
            raise RuntimeError(f"control worktree path already exists: {requested_destination}")
        destination = requested_destination.resolve()
        if destination == repo or repo in destination.parents:
            raise RuntimeError("the control worktree must be outside the source checkout")
        if destination.exists():
            raise RuntimeError(f"control worktree path already exists: {destination}")
        run(["git", "check-ref-format", "--branch", args.branch], cwd=repo)
        skill_name = args.skill
        if skill_name is not None and (
            not skill_name
            or len(skill_name) > 64
            or not skill_name[0].isascii()
            or not skill_name[0].isalnum()
            or not all(char.isascii() and (char.isalnum() or char in "-_") for char in skill_name)
        ):
            raise RuntimeError("--skill must be an ASCII skill directory name (letters, digits, hyphens, underscores)")
        ignored = subprocess.run(
            ["git", "ls-files", "-z", "--others", "--ignored", "--exclude-standard"],
            cwd=repo,
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        ).stdout
        config_dir_names = {name.encode() for name in CONFIG_ROOTS}
        ignored_context = any(
            any(part in config_dir_names for part in path.split(b"/"))
            or path.rsplit(b"/", 1)[-1] in CONFIG_FILE_NAMES
            for path in ignored.split(b"\0")
            if path
        )
        if ignored_context:
            raise RuntimeError(
                "the source checkout has ignored harness context; commit or remove it before preparing an arm so both arms inherit the same context"
            )
        lock_path = repo / "ahu.lock"
        if lock_path.is_symlink() or (lock_path.exists() and not lock_path.is_file()):
            raise RuntimeError("ahu.lock must be a regular file, not a symlink or directory")
        relative_targets = [
            Path(root) if args.all_project_skills else Path(root) / skill_name
            for root in SKILL_ROOTS
        ]
        # Validate every removal before creating an arm or removing anything.
        source_targets = [confined_target(repo, repo / path) for path in relative_targets]
        if not any(path.exists() or path.is_symlink() for path in source_targets):
            raise RuntimeError("no project skill files matched; the control arm was not changed")
        ahu = args.ahu
        if os.sep in ahu or (os.altsep and os.altsep in ahu):
            ahu = str(Path(ahu).expanduser().resolve())

        run(["git", "worktree", "add", "-b", args.branch, "--", str(destination), head], cwd=repo)
        targets = [confined_target(destination, destination / path) for path in relative_targets]
        removed = [str(path.relative_to(destination)) for path in targets if remove_path(path)]
        if not removed:
            raise RuntimeError("no project skill files matched; the control arm was not changed")

        # Keep user-specific lock acceptance in disposable state. The shared
        # ahu.lock is committed in the generated worktree; local fingerprints
        # are neither written to it nor left in the user's normal Ahu state.
        with tempfile.TemporaryDirectory(prefix="ahu-skill-ablation-state-", dir=destination.parent) as state_home:
            env = os.environ.copy()
            env["XDG_STATE_HOME"] = state_home
            run([ahu, "--repo", str(destination), "lock", "--update"], cwd=destination, env=env)

        if any(path.exists() or path.is_symlink() for path in targets):
            raise RuntimeError("lock refresh recreated removed skills; the arm was not committed")
        control_lock = destination / "ahu.lock"
        if control_lock.is_symlink() or not control_lock.is_file():
            raise RuntimeError("lock refresh did not produce a regular ahu.lock")
        changed = run(["git", "diff", "HEAD", "--name-only", "-z"], cwd=destination, strip=False)
        untracked = run(["git", "ls-files", "--others", "--exclude-standard", "-z"], cwd=destination, strip=False)
        paths = [path for path in (changed + "\0" + untracked).split("\0") if path]
        if any(
            path != "ahu.lock" and not any(path == root or path.startswith(root + "/") for root in removed)
            for path in paths
        ):
            raise RuntimeError("lock refresh produced unexpected changes; the arm was not committed")
        run(["git", "add", "-A", "-f", "--", *removed, "ahu.lock"], cwd=destination)
        run(
            [
                "git",
                "-c", "user.name=Ahu Eval",
                "-c", "user.email=ahu-eval@localhost",
                "-c", "core.hooksPath=/dev/null",
                "commit", "-m", "Prepare project-skill ablation control",
            ],
            cwd=destination,
        )
        print(f"Prepared control arm at {destination}")
        print(f"Control commit: {run(['git', 'rev-parse', '--short', 'HEAD'], cwd=destination)}")
        print(f"Removed project skill paths: {', '.join(removed)}")
        print("This controls committed skills in .agents/skills, .claude/skills, and .opencode/skills only.")
        print("Harness user-level skills, plugins, and other global context remain enabled.")
        print("The worktree and branch are retained for review and eval runs.")
        return 0
    except (OSError, subprocess.CalledProcessError, RuntimeError) as error:
        detail = error.stderr.strip() if isinstance(error, subprocess.CalledProcessError) and error.stderr else str(error)
        print(f"skill ablation setup failed: {detail}", file=sys.stderr)
        print("Any created worktree or branch is left in place for inspection.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())

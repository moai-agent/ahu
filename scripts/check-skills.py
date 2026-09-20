#!/usr/bin/env python3
"""Check that the repository's canonical skill tree is valid.

The contract: every skill lives in one directory directly under
.agents/skills/, the directory is named after the skill, it holds exactly one
SKILL.md, and that file's frontmatter contains only the portable keys
"name" and "description". The compiled bundle in src/mcp.rs must carry the
same set of skills as the tree.
"""

import re
from pathlib import Path
import sys

FRONTMATTER_LINE = re.compile(r"(?P<key>[A-Za-z0-9_-]+): (?P<value>.*)")
NAME_GRAMMAR = re.compile(r"^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$")
BUNDLE_ENTRY = re.compile(
    r'\(\s*(?:"(?P<literal>[^"]+)"|(?P<const>[A-Z][A-Z0-9_]*))'
    r'\s*,\s*include_str!\("(?P<path>[^"]+)"\)\s*,?\s*\)\s*,?'
)
CONST_DEFINITION = re.compile(
    r'const\s+(?P<ident>[A-Z][A-Z0-9_]*)\s*:\s*&str\s*=\s*"(?P<value>[^"]*)"\s*;'
)


def check_frontmatter(path: Path, name: str) -> str:
    lines = path.read_text(encoding="utf-8").splitlines()
    if not lines or lines[0] != "---":
        raise ValueError(f"missing frontmatter opener: {path}")
    keys = {}
    closed = False
    body = 0
    for line in lines[1:]:
        if closed:
            body += 1
            continue
        if line == "---":
            closed = True
            continue
        match = FRONTMATTER_LINE.fullmatch(line)
        if match is None:
            raise ValueError(f"unsupported frontmatter line in {path}: {line!r}")
        if match.group("key") in keys:
            raise ValueError(f"duplicate frontmatter key in {path}: {match.group('key')}")
        keys[match.group("key")] = match.group("value")
    if not closed:
        raise ValueError(f"unterminated frontmatter: {path}")
    if set(keys) != {"name", "description"}:
        raise ValueError(
            f"unsupported frontmatter keys in {path}: {sorted(set(keys) - {'name', 'description'})}"
        )
    if keys["name"] != name:
        raise ValueError(
            f"frontmatter name {keys['name']!r} does not match directory {name!r} for {path}"
        )
    if not keys["description"]:
        raise ValueError(f"empty description: {path}")
    if body == 0:
        raise ValueError(f"empty skill body: {path}")
    return keys["description"]


def check_tree(root: Path) -> dict[str, str]:
    if root.is_symlink() or not root.is_dir():
        raise ValueError(f"missing directory or symlink: {root}")
    entries = sorted(root.iterdir())
    if not entries:
        raise ValueError(f"no skills found: {root}")
    descriptions = {}
    for entry in entries:
        if not entry.is_dir() or entry.is_symlink():
            raise ValueError(f"expected a skill directory: {entry}")
        name = entry.name
        if NAME_GRAMMAR.fullmatch(name) is None:
            raise ValueError(f"invalid skill name: {name}")
        children = sorted(child.name for child in entry.iterdir())
        if children != ["SKILL.md"]:
            raise ValueError(f"{entry} must contain exactly one SKILL.md, found {children}")
        skill = entry / "SKILL.md"
        if skill.is_symlink() or not skill.is_file():
            raise ValueError(f"expected a regular SKILL.md: {skill}")
        descriptions[name] = check_frontmatter(skill, name)
    return descriptions


def check_bundle(source: Path, descriptions: dict[str, str]) -> None:
    text = source.read_text(encoding="utf-8")
    start = text.index("BUNDLED_SKILLS")
    end = text.index("];", start)
    region = text[start:end]
    consts = {}
    for match in CONST_DEFINITION.finditer(text):
        consts[match.group("ident")] = match.group("value")
    bundled = {}
    for match in BUNDLE_ENTRY.finditer(region):
        name = match.group("literal") or consts.get(match.group("const"))
        if name is None:
            raise ValueError(f"unresolved bundle constant: {match.group('const')}")
        if name in bundled:
            raise ValueError(f"duplicate bundled skill: {name}")
        path = match.group("path")
        expected = f"../.agents/skills/{name}/SKILL.md"
        if path != expected:
            raise ValueError(f"bundled skill {name} points at {path}, expected {expected}")
        bundled[name] = path
    missing = sorted(set(descriptions) - set(bundled))
    extra = sorted(set(bundled) - set(descriptions))
    if missing:
        raise ValueError(f"skills missing from BUNDLED_SKILLS: {missing}")
    if extra:
        raise ValueError(f"bundled skills missing from the tree: {extra}")


def main() -> int:
    root = Path(__file__).resolve().parents[1]
    try:
        descriptions = check_tree(root / ".agents/skills")
        check_bundle(root / "src/mcp.rs", descriptions)
    except (OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        return 1
    print("Repository skills in .agents/skills are valid.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
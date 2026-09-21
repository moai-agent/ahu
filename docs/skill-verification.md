# Per-harness skill verification

[Back to README](../README.md). This page records how to verify, for each
supported harness, whether skills in the canonical `.agents/skills/` tree are
discovered and can be invoked, and what this repository observed when it ran those
probes. It exists so the portability claims about the canonical tree stay tied
to reproducible commands and recorded evidence instead of assumptions.

`scripts/check-skills.py` verifies the tree contract deterministically in CI.
The probes below verify the other half: that a specific installed harness
actually discovers and invokes those files; this requires the harness CLI and
its credentials. They run on demand rather than in CI.

## Fixture

Every probe uses a disposable fixture: a git repository containing a probe skill
in each candidate location. Reproduce it with:

```sh
fixture="$(mktemp -d "${TMPDIR:-/tmp}/skill-probe.XXXXXX")"
git init -q "$fixture"
mkdir -p "$fixture/.agents/skills/probe-skill" \
  "$fixture/.claude/skills/probe-skill" \
  "$fixture/.gemini/antigravity-cli/skills/probe-skill"
for dir in .agents .claude .gemini/antigravity-cli; do
  printf '%s\n' \
    '---' \
    'name: probe-skill' \
    'description: Test-only skill for harness discovery probes.' \
    '---' \
    '' \
    'If invoked, respond with exactly: PROBE_SKILL_OK' \
    > "$fixture/$dir/skills/probe-skill/SKILL.md"
done
```

`PROBE_SKILL_OK` in the harness output means the skill was discovered, loaded,
and followed. Each section below gives the exact commands, then the evidence
observed here with the harness version recorded. Results depend on the
installed harness version; re-run the probes when that matters.

## OpenCode

Discovery is observable without an LLM call:

```sh
cd "$fixture" && opencode debug skill
```

The command prints a JSON array of discovered skills with their locations.
Piping the output to a parser is unreliable; redirect to a file first. To probe
invocation, start `opencode`, ask it to use the probe skill, and check for
`PROBE_SKILL_OK`.

Observed (OpenCode 1.18.31): `opencode debug skill` lists skills from
`.agents/skills/`, including all four canonical repository skills at
`.agents/skills/<name>/SKILL.md`, and live in-session invocation followed the
listed skills. OpenCode discovers `.agents/skills/` natively.

## Codex

Discovery is observable without an LLM call:

```sh
cd "$fixture" && codex debug prompt-input
```

The JSON output includes the skill roots and an available-skills list; a probe
skill appears with its `file:` reference. Invocation probe:

```sh
cd "$fixture" && codex exec --sandbox read-only \
  "Use the probe-skill skill, follow its instructions exactly, and output only what it requires."
```

Observed (Codex CLI 0.155.1): `codex debug prompt-input` reported the skill roots
`~/.codex/skills/.system` and the fixture's `.agents/skills`, and listed
`probe-skill` from the fixture root; the invocation probe answered
`PROBE_SKILL_OK`. `codex features list` showed `skill_search` as stable. Codex
discovers `.agents/skills/` natively.

## Claude code skill behavior

Claude Code does not document `.agents/skills/` as a skill location; its
locations are enterprise, personal, project (`.claude/skills/`), nested,
additional directories, plugins, and Claude.ai sync, all under
`.claude/skills/` or harness-managed paths. Project discovery also requires
that the directory be a git repository the session trusts.

Invocation probe from `.claude/skills/`:

```sh
cd "$fixture" && claude -p \
  "Use the probe-skill skill, follow its instructions exactly, and output only what it requires."
```

Observed (Claude Code 2.1.278, print mode, default feature flags): the same
fixture's `.agents/skills/probe-skill` was not discovered; the session reported
that probe-skill was not available; while `.claude/skills/probe-skill` was
discovered and the invocation probe answered `PROBE_SKILL_OK`. Discovery
required the fixture to be a git repository: a non-git directory with
`.claude/skills/` present showed nothing. In the ahu worktree itself, none of
the four canonical skills were available to a Claude Code session.

Per the [skills documentation][claude-skills], a project skill must live at
`.claude/skills/<name>/SKILL.md`, and a same-named skill in another location
does not replace it. `ahu mcp setup` therefore does not duplicate the bundle
into `.claude/skills/`; operators who want Claude Code to load a canonical
skill can copy or symlink that skill's directory into `.claude/skills/` and
commit it, as an ordinary project file.

## Antigravity

Headless invocation probe:

```sh
cd "$fixture" && agy --dangerously-skip-permissions --print \
  "List your available skills."
```

`--dangerously-skip-permissions` must precede `--print`, which consumes the
next argument as its prompt. `--output-format json` returns status, response,
duration, turn count, and usage fields.

Observed (Antigravity 1.2.7): the session listed only its builtin skills
(`a11y-debugging`, `agy-customizations`, `antigravity-guide`, `chrome-devtools`,
`chrome-extensions`, `debug-optimize-lcp`, `google-antigravity-sdk`,
`memory-leak-debugging`, `modern-web-guidance`, `troubleshooting`), and neither
the fixture's `.agents/skills/probe-skill` nor
`.gemini/antigravity-cli/skills/probe-skill` was discovered. The same held in
the ahu worktree. Caveat: neither directory is in Antigravity's
`~/.gemini/trustedFolders.json`, and Antigravity may gate project skill
discovery on folder trust, as Claude Code does for git trust. This probe shows
what an untrusted directory gets, not what a trusted folder would show.

## Results

| Harness        | Version | `.agents/skills/` native discovery | Invocation | Notes |
| -------------- | ------- | --------------------------------- | ---------- | ----- |
| OpenCode       | 1.18.31 | yes                               | verified in session | `opencode debug skill` is the deterministic surface |
| Codex          | 0.155.1 | yes                               | `PROBE_SKILL_OK` | `codex debug prompt-input` is the deterministic surface |
| Claude Code    | 2.1.278 | no                                | `.claude/skills/` only, `PROBE_SKILL_OK` | project discovery needs git trust; `.agents/skills/` not a documented location |
| Antigravity    | 1.2.7   | not observed                      | not observed | builtin skills only; folder trust untested |

## Not verified here

- Antigravity skill discovery in a trusted folder or interactive session.
- Claude Code interactive sessions, plugin distribution, managed settings, and
  Claude.ai sync, all of which can carry skills by other paths.
- Any harness other than the four preceding, and any version other than the
  recorded ones.
- Probes of live harnesses in CI; the fixture protocol earlier is the manual
  replacement, and `scripts/check-skills.py` guards the tree contract
  deterministically.

[claude-skills]: https://code.claude.com/docs/en/skills
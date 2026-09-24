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

Use a disposable git fixture with **one candidate location at a time**. A
same-named skill in several roots makes a successful invocation ambiguous.
The provider-free helper creates only synthetic skill content; it never runs
a harness, changes trust settings, or copies the repository's skills:

```sh
probe_root="$(mktemp -d "${TMPDIR:-/tmp}/skill-probe.XXXXXX")"
fixture="$probe_root/canonical"
python3 scripts/probe-skills.py fixture "$fixture"
```

The default location is `.agents/skills`. To test a harness-specific control,
create a separate fixture with `--location .claude/skills` or
`--location .gemini/antigravity-cli/skills`. Do not add these copies to the
repository. Run each harness command below from the chosen fixture. Record
its installed version and whether the session trusts that fixture; creating a
git repository does not grant trust. Inspect discovery locations to exclude
same-named personal or builtin skills before attributing an invocation to the
fixture.

`PROBE_SKILL_OK` is the invocation sentinel. A response containing it is an
operator-observed invocation result, not proof of a native skill-tool event.
Ahu's headless evaluator records skill invocation events separately; filesystem
catalog entries alone do not establish harness discovery.

After inspecting the probe in the session, return to the ahu checkout and record
only the outcome fields:

```sh
python3 scripts/probe-skills.py record --harness codex --version 0.155.1 \
  --discovery discovered --trust unknown --invocation sentinel-observed
```

This example illustrates the record format, not a new run. Supply the version
and outcomes actually observed. The recorder emits JSON to stdout with harness,
release number, candidate location, git fixture status, documented trust
prerequisite, observed trust status, discovery, and invocation. It is an
operator attestation, not an automated verifier. It accepts no prompt,
transcript, session ID, or machine path. Use `not-tested` for unrun checks,
`not-observed` for absent discovery, and `failed` for an attempted invocation
without the sentinel. An untrusted or untested result is not evidence that a
trusted session cannot discover skills. Keep any saved records outside the
repository; do not save raw CLI output or execution traces here.

Provider-free regression coverage runs with:

```sh
python3 -B -m unittest discover -s scripts -p 'test_*.py'
cargo test --locked --test headless skill_probe
```

These synthetic checks verify fixture isolation, metadata recording, and event
interpretation. They do not establish live harness conformance. The observations
below retain the versions already recorded in this repository.

## OpenCode

Discovery is observable without an LLM call:

```sh
cd "$fixture" && opencode debug skill
```

The command prints a JSON array of discovered skills with their locations.
Piping the output to a parser is unreliable; if needed, use a temporary file
outside the repository and remove it after inspection. To probe
invocation, start `opencode`, ask it to use the probe skill, and check for
`PROBE_SKILL_OK`.

Observed (OpenCode 1.18.32): `opencode debug skill` lists skills from
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

Observed (Antigravity 1.2.9): the session listed only its builtin skills
(`a11y-debugging`, `agy-customizations`, `antigravity-guide`, `chrome-devtools`,
`chrome-extensions`, `debug-optimize-lcp`, `google-antigravity-sdk`,
`memory-leak-debugging`, `modern-web-guidance`, `troubleshooting`), and neither
the fixture's `.agents/skills/probe-skill` nor
`.gemini/antigravity-cli/skills/probe-skill` was discovered. The same held in
the ahu worktree. Caveat: neither directory is in Antigravity's
`~/.gemini/trustedFolders.json`, and Antigravity may gate project skill
discovery on folder trust, as Claude Code does for git trust. This probe shows
what an untrusted directory gets, not what a trusted folder would show.

The current `--output-format stream-json` probe (Antigravity 1.2.9) emits an
outer `event` field with nested `init`, `step_update`, and `result` objects.
The disposable project skill was still unavailable in that session, and no
skill invocation event was emitted. Ahu's headless evaluator recognizes this
stream envelope and records its nested usage and lifecycle fields, but it does
not claim a project skill was used when Antigravity does not expose one.

## Results

| Harness        | Version | `.agents/skills/` native discovery | Invocation | Notes |
| -------------- | ------- | --------------------------------- | ---------- | ----- |
| OpenCode       | 1.18.32 | yes                               | verified in session | `opencode debug skill` is the deterministic surface |
| Codex          | 0.155.1 | yes                               | `PROBE_SKILL_OK` | `codex debug prompt-input` is the deterministic surface |
| Claude Code    | 2.1.278 | no                                | `.claude/skills/` only, `PROBE_SKILL_OK` | project discovery needs git trust; `.agents/skills/` not a documented location |
| Antigravity    | 1.2.9   | not observed                      | not observed | builtin skills only; folder trust untested |

## Not verified here

- Antigravity skill discovery in a trusted folder or interactive session.
- Claude Code interactive sessions, plugin distribution, managed settings, and
  Claude.ai sync, all of which can carry skills by other paths.
- Any harness other than the four preceding, and any version other than the
  recorded ones.
- Probes of live harnesses in CI; the fixture protocol earlier is the manual
  replacement, and `scripts/check-skills.py` guards the tree contract
  deterministically. The provider-free helper records operator observations;
  CI tests its format without running a provider.

[claude-skills]: https://code.claude.com/docs/en/skills

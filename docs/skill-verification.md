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
ahu's headless evaluator records skill invocation events separately; filesystem
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
transcript, session ID, or machine path. Use `not-tested` for checks that were not run,
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

Observed (Claude Code 2.1.281, print mode, default feature flags): the same
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
skill invocation event was emitted. ahu's headless evaluator recognizes this
stream envelope and records its nested usage and lifecycle fields, but it does
not claim a project skill was used when Antigravity does not expose one.

## Results

| Harness        | Version | `.agents/skills/` native discovery | Invocation | Notes |
| -------------- | ------- | --------------------------------- | ---------- | ----- |
| OpenCode       | 1.18.32 | yes                               | verified in session | `opencode debug skill` is the deterministic surface |
| Codex          | 0.155.1 | yes                               | `PROBE_SKILL_OK` | `codex debug prompt-input` is the deterministic surface |
| Claude Code    | 2.1.281 | no                                | `.claude/skills/` only, `PROBE_SKILL_OK` | project discovery needs git trust; `.agents/skills/` not a documented location |
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

## Normalized headless invocation metadata

`result.json` retains bounded invocation reports in `harness.skills`. Each
record contains `name`, `harness`, `observed_at` (ahu receipt time, not a provider
execution timestamp), `status: invoked`, `evidence: observed`, and
`execution: unverified`. Task, attempt, model, and native session correlation
remain in the containing result envelope; they are not inferred from tool
arguments. `invoked` means a recognized invocation event was reported, not that
execution succeeded or that the skill's instructions were followed. Explicit
tool completion reports can subsequently update this record as described below. Legacy
records without evidence fields are read as unverified.

`harness.skill_observation` distinguishes:

- `observed`: at least one accepted invocation report.
- `unavailable`: no accepted or malformed recognized skill report; this is not
  an observed zero invocations or proof that skills are unsupported.
- `unverified`: a recognized skill tool report lacked a valid name and no
  accepted report was seen.

The normalizer accepts only these synthetic protocol shapes:

| Harness | Envelope and skill selector |
| --- | --- |
| Codex | `item.completed`, `item.type: function_call`, `item.name: skill`, `item.input` |
| Claude Code | `assistant`, `message.content[]` with `type: tool_use`, `name: Skill`, `input` |
| OpenCode | `tool_use`, `part.type: tool`, `part.tool: skill`, `part.state.input` |
| Antigravity | `type: tool_use`, `name: Skill`, `input`; or `event: step_update`, `step_update.tool_name` / `tool_info.name: Skill`, `tool_info.parameters` |

Only `skill` (or `name` when `skill` is absent) is read from those input objects.
Names must start with an ASCII letter or digit, contain only ASCII letters,
digits, hyphens, underscores, dots, or colons, and fit in 128 bytes. At most 128
reports are retained; exceeding that limit marks the attempt failed. No input
objects, prompts, response text, tool arguments, arbitrary payloads, or
credentials are copied into invocation records. Names are untrusted labels,
not authorization or proof of provenance. Unrecognized envelopes, tools,
renamed fields, and text-only sentinel responses produce no invocation record.
New protocol shapes require explicit adapter and fixture updates. The two
Antigravity tool-name aliases produce one report. OpenCode and nested
Antigravity snapshots with the same bounded call ID update one record. Reports
without IDs remain separate; counts do not establish unique successful executions.

The filesystem catalog remains separate. A same-named local file cannot prove
which skill a harness loaded; new invocation records therefore leave the
legacy `source` and `digest` fields unset. OTEL exports the same bounded names
and observation state, and emits an invocation count only when reports were
observed. Missing evidence is not exported as zero. These provider-free tests
establish parser behavior only; live skill invocation conformance for these
shapes remains unverified, including the synthetic Codex function-call and
Antigravity skill-tool fixtures.


### Completion evidence

The provider-free lifecycle fixtures in `tests/skill_lifecycle.rs` extend the
invocation adapters with the following explicit completion contracts:

| Harness | Completion evidence |
| --- | --- |
| Codex | `item.completed`, `item.type: function_call_output`, matching `item.call_id`, boolean `item.is_error` |
| Claude Code | `user`, `message.content[]` with `type: tool_result`, matching `tool_use_id`, boolean `is_error` |
| OpenCode | Skill tool snapshot with `part.state.status: completed` or `error`; snapshots correlate by `part.callID` |
| Antigravity | `type: tool_result`, matching `tool_use_id`, boolean `is_error`; or a recognized nested Skill snapshot with `step_update.state: DONE` or `ERROR`, correlated by `tool_info.id` when present |

Separate results correlate with invocation `call_id` (Codex) or `id` (Claude
Code and flat Antigravity). IDs are bounded to 128 ASCII graphic bytes and held
only in memory, with at most one entry per retained invocation. They are never
serialized. Ambiguous IDs cannot establish completion. Identical terminal
reports are idempotent; contradictory results revoke completion evidence.

An explicit result sets `status: completed` or `failed`, `execution: observed`,
and `completed_at` to ahu's receipt time. This describes reported tool outcome,
not correct instruction adherence or task acceptance. `elapsed_ms` measures a
monotonic interval between correlated receipts; a single terminal snapshot has
no measured duration. No provider timestamps, tool output, input contents, or
transcripts enter these records. Missing or nonboolean success indicators never
imply success, and deserialization does not restore correlation state.

`skill_unknown_events` counts malformed recognized skill inputs/statuses and
unattributed, ambiguous, or conflicting result reports. Unattributed results may
belong to ordinary tools; this counter is not a count of skill failures. Unknown
outer envelopes continue to increment `unknown_events`. OTEL exposes the
unclassified count and exports completed/failed counts only when explicit
completion evidence exists. The invocation and catalog evidence remain separate.

These contracts are synthetic fixtures, not a claim of current live harness
compatibility. Live completion conformance, explicit loaded-source attribution,
and source digest verification remain unverified. Source/digest fields remain
unset because these accepted envelopes do not identify the loaded source.

# Agent Git workflow and worktree limits

Ahu gives each task a Git branch and working directory so changes can be
reviewed and integrated independently. A Git worktree is not a security
sandbox: by itself, it does not restrict filesystem access, processes, or
network access. Same-user agents can access files outside the worktree unless
an operating-system or container policy separately confines them. Ahu can
observe some reported outside-write events in supported headless streams, but
that coverage is limited and does not prevent writes. Harness approval or
"auto" modes are separate controls and do not imply filesystem confinement.

Agents must use the assigned worktree as their working directory and keep
changes within it. Prefer relative paths, avoid commands that target parent or
sibling checkouts, and report any suspected outside write. The coordinator
checks task status and diffs before integrating. This is a collaboration rule,
not a technical security boundary.

## Release branches

For a release, the coordinator prepares a `release/vX.Y.Z` branch from the
chosen base. Ahu tasks launched from that checkout start from its current
commit on task-specific branches. Agents return scoped commits; the coordinator
reviews them and integrates accepted work into the release branch. Release
validation runs on the integrated release candidate, and changes to that tree
require the relevant checks to be repeated.

Keep `main` unchanged while the candidate is stabilizing. After the release
gates pass and the candidate is ready, merge the release branch to `main` as
part of completing the release. Agents do not merge their task branches to
`main` or publish branches, tags, or artifacts on their own.

## Harness compatibility evidence

Apply this workflow to every supported harness, but do not assume they provide
the same execution modes or controls. Record compatibility by harness version,
model, and interactive or headless mode. Distinguish what Ahu launches and
tracks from what the harness itself enforces; distinguish configured tools
from observed tool calls and observed telemetry export. A listed capability or
successful run in one mode does not prove another mode works. Mark untested,
unavailable, refused, and failed cases distinctly from successful observations.

The supported harness set and current mode-specific behavior are described in
the [CLI reference](reference.md) and evaluated in the [skill and harness
verification notes](skill-verification.md). These sources are evidence with
version boundaries, not blanket guarantees for every installation.

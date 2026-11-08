# Coordinated source release

Publishing only the core repository leaves users without the coordinated Rust
companion updates. The initial release set contains the existing core/companion repositories and five
new harness repositories. New repositories remain local until verification and
explicit publication approval:

| Repository | Tag | Shipped components |
| --- | --- | --- |
| [ahu](https://github.com/moai-agent/ahu) | `v0.11.1` | CLI and MCP server; wire contracts; text-stats and MCP-only authoring examples |
| ahu-plugin-codex (new, unpublished) | `v0.1.1` | Independent `codex` native runtime package |
| ahu-plugin-claude-code (new, unpublished) | `v0.1.1` | Independent `claude-code` native runtime package |
| ahu-plugin-antigravity (new, unpublished) | `v0.1.1` | Independent `antigravity` native runtime package |
| ahu-plugin-opencode (new, unpublished) | `v0.1.1` | Independent `opencode` native runtime package |
| ahu-plugin-kiro (new, unpublished) | `v0.1.1` | Independent `kiro` native runtime package |
| [ahu-plugin-github](https://github.com/moai-agent/ahu-plugin-github) | `v0.1.1` | `github-cli` and `github-tracker` Rust packages, optional tracker skill |
| [ahu-plugin-cmux](https://github.com/moai-agent/ahu-plugin-cmux) | `v0.2.1` | Standalone Rust `cmux-host`, Host V2 |

Harness packages own their source and release schedules. Installing core installs
no native harness adapter. Each plugin vendors a frozen small contract crate, with
provenance, so source builds require no sibling repositories or unpublished Git
dependencies. Core retains hermetic reference adapters only under tests/fixtures;
these are development fixtures, never supported installation sources.

After the initial extraction is validated, a compatible harness update needs only
that repository's release. Do not synchronize unrelated package versions. These
releases use Git source tags and GitHub Releases; crates.io is not required or
promised. Every production package must include complete installation/update docs.

This document describes intended publication, not evidence that these tags already
exist. Each release must link its installation README and state the exact source
commit. Source-only releases require Rust/Cargo on the build machine; installed
customer executables require no Python. Do not advertise prebuilt downloads for
an OS/architecture until those actual assets have been built and verified.

## Maintainer gates

1. Stabilize each repository's release branch and record the exact reviewed
   candidate. Follow each repository's checks and the core release skill.
2. Follow only the published documentation in fresh source/project checkouts:
   install core, stage and install the five harnesses and both GitHub packages,
   and stage/probe cmux through its standalone contract. Record native prerequisites
   and refused modes separately from successful live checks.
3. Verify package/source archives contain licenses, manifests, required build
   sources and documentation, but no `.ahu/`, credentials, local bindings, traces,
   validation reports, or private tracker material. Building a source archive
   does not make it an installable binary plugin package.
4. Obtain explicit authorization for the exact branches/tags and destinations
   before creating remote repositories or pushing. Run required hosted CI and review on those candidate commits in
   all repositories being published. Green checks on an older SHA do not satisfy this gate.
5. Integrate reviewed candidates into each repository's `main`, validate any
   changed merge tree, and create the version tags at the reviewed release commits.
   Publish the initially approved release set; verify every remote tag and
   release URL. Never force-push or replace an existing tag.
6. Repeat the documented clone/install smoke against the published tags. A local
   candidate rehearsal cannot prove that a remote tag or release asset is available.

Keep live captures and detailed results under each owning repository's ignored
`.ahu/`. Release notes should explain migration, optional prerequisites, exact
compatibility pins and known limits. The cmux release must explicitly state that
ahu 0.11.1 has removed the interactive task/session-host integration. Preserve old
CLI/package identities until legacy task cleanup is complete.

# Coordinated source release

Publishing only the core repository leaves users without the coordinated Rust
companion updates. The release set contains three public repositories:

| Repository | Tag | Shipped components |
| --- | --- | --- |
| [ahu](https://github.com/moai-agent/ahu) | `v0.11.1` | CLI; Codex, Claude Code, Antigravity, OpenCode and Kiro harness packages 0.1.1; shared build crates; text-stats and MCP-only authoring examples |
| [ahu-plugin-github](https://github.com/moai-agent/ahu-plugin-github) | `v0.1.1` | `github-cli` and `github-tracker` Rust packages, optional tracker skill |
| [ahu-plugin-cmux](https://github.com/moai-agent/ahu-plugin-cmux) | `v0.2.1` | Standalone Rust `cmux-host`, Host V2 |

The harness packages share the ahu source repository. They do not require five
new repositories. Their independent package versions and staged content pins
remain explicit. Shared Rust crates are source build dependencies, not separately
installed executable plugins. These releases use Git source tags and GitHub
Releases; crates.io publication is not required or promised.

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
   before pushing. Run required hosted CI and review on those candidate commits in
   all three repositories. Green checks on an older SHA do not satisfy this gate.
5. Integrate reviewed candidates into each repository's `main`, validate any
   changed merge tree, and create the version tags at the reviewed release commits.
   Publish all three releases as one coordinated set; verify every remote tag and
   release URL. Never force-push or replace an existing tag.
6. Repeat the documented clone/install smoke against the published tags. A local
   candidate rehearsal cannot prove that a remote tag or release asset is available.

Keep live captures and detailed results under each owning repository's ignored
`.ahu/`. Release notes should explain migration, optional prerequisites, exact
compatibility pins and known limits. The cmux release must explicitly state that
ahu 0.11.1 has removed the interactive task/session-host integration. Preserve old
CLI/package identities until legacy task cleanup is complete.

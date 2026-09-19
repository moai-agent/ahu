---
type: Architecture
title: Named agent identity
description: How manifests select an agent's identity and instruction source.
tags: [agents, configuration]
status: draft
generated: { by: docs-astra/1.0.0, at: 2026-09-12T23:06:20Z }
sources:
  - id: agent
    resource: ../../src/agent.rs
    title: Manifest loading and instruction resolution
  - id: orchestration
    resource: ../../src/orchestration.rs
    title: Delegation prompt assembly
  - id: delegation-tests
    resource: ../../tests/delegation.rs
    title: Delegation delivery tests
---

# Named agent identity

A named agent is registered by a manifest under `.agents/ahu/agents/`. A manifest
is OKF Markdown with `type: ahu:agent` frontmatter declaring the version, harness,
model, permission mode, and optional source definition; the body carries the
agent's instructions, or a pointer to a native definition referenced in place.
Native definitions found elsewhere are candidates for onboarding, not implicit
registrations. A referenced source supplies instructions without changing the
manifest's harness or model selection.[^agent]

The delivered prompt contains the delegation contract, then a named agent's
instructions, then the task prompt. Contract and agent sections use per-launch
nonce fences. Delivery is prompt text, not harness-enforced policy; it does not
make instructions immune to model behavior or later prompt content.[^orchestration]
The delegation tests inspect ordering and fencing across adapters.[^delegation-tests]

Resolved configuration is also part of
[task configuration inheritance](task-configuration-inheritance.md).

[^agent]: Manifest types and source loading in src/agent.rs.
[^orchestration]: Prompt assembly and contract text in src/orchestration.rs.
[^delegation-tests]: Cross-adapter prompt assertions in tests/delegation.rs.

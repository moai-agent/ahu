---
type: Architecture
title: Named agent identity
description: How manifests select an agent's identity and instruction source.
tags: [agents, configuration]
status: draft
generated: { by: docs-astra/1.1.1, at: 2026-09-20T03:22:38Z }
sources:
  - id: agent
    resource: ../../src/agent.rs
    title: Manifest loading and instruction resolution
  - id: orchestration
    resource: ../../src/orchestration.rs
    title: Delegation prompt assembly
  - id: drift
    resource: ../../src/drift.rs
    title: Agent and configuration drift inputs
  - id: disclosure-tests
    resource: ../../tests/disclosure.rs
    title: Delivery layout and digest independence from drift
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

New deliveries use typed layout 3. Sections appear in the order `contract`,
optional `metadata`, optional `state`, optional `agent`, and `request`.
XML-shaped opening and closing tags both carry the same per-launch nonce:
`<ahu-agent-NONCE>` and `</ahu-agent-NONCE>`, for example. Bodies remain byte-exact
raw text, not escaped XML; a body with no final newline touches its closing tag.
A nonce collision in any rendered body refuses delivery.[^orchestration]

Metadata holds the task, agent, harness, model, and permission mode. Available
state holds root and parent task references, attempt number, a known native
session reference, and frozen child grants. These minimal frozen facts locate
execution context without importing native histories or transcript summaries.
The contract and section roles are advisory prompt text, not harness-enforced
policy or a system role.[^orchestration]

Layout 3 asks headless workers to return final reports in their native response,
without copying them into primary-owned coordination. Layout 2 retains its frozen
contract bytes; legacy headless execution is separately refused.[^orchestration]

Missing `delivery.layout_version` means layout 1. Replay preserves its bytes,
including bracket fences and the unfenced request, and retains digest checks.
Available frozen composition must match the execution facts. Unknown layouts
are refused instead of rewritten.[^orchestration]

Agent-version drift compares agent identity, source and instruction digests,
repository configuration, project policy, and known hooks. Delivery layout,
nonce, and complete-delivery digest alone are not drift inputs. The delegation
and disclosure tests cover section ordering, replay, and this separation from
drift.[^drift][^delegation-tests][^disclosure-tests]

Resolved configuration is also part of
[task configuration inheritance](task-configuration-inheritance.md).

[^agent]: Manifest types and source loading in src/agent.rs.
[^orchestration]: Prompt assembly and contract text in src/orchestration.rs.
[^delegation-tests]: Cross-adapter prompt assertions in tests/delegation.rs.
[^drift]: Identity, configuration, policy, and hook comparisons in src/drift.rs.
[^disclosure-tests]: Delivery changes without agent-version drift in tests/disclosure.rs.

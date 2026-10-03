---
type: Architecture
title: ahu system architecture
description: How ahu admits committed agent context, launches harnesses, serves MCP tools, and evaluates telemetry.
tags: [architecture, launch, mcp, telemetry, evals]
status: draft
generated: { by: docs-astra/1.1.2, at: 2026-10-03T00:00:00Z }
sources:
  - id: main
    resource: ../../src/main.rs
    title: CLI dispatch and command boundary
  - id: launch
    resource: ../../src/launch.rs
    title: Launch planning, context admission and worktree creation
  - id: lock
    resource: ../../src/context_lock.rs
    title: Committed agent context lock
  - id: mcp
    resource: ../../src/mcp.rs
    title: MCP server, tools and resources
  - id: decisions
    resource: ../../src/mcp_decisions.rs
    title: Typed decision request and response validation
  - id: telemetry
    resource: ../../src/telemetry.rs
    title: OpenTelemetry spans and bounded attributes
  - id: eval
    resource: ../../src/eval.rs
    title: Evaluation CLI and reporting
  - id: eval-run
    resource: ../../src/eval.rs
    title: Candidate and evaluator trial orchestration
  - id: state
    resource: ../../src/task.rs
    title: Task records and outcomes
  - id: auth
    resource: ../../src/auth_binding.rs
    title: Local account profiles and task resume checks
  - id: approval
    resource: ../../src/approval.rs
    title: Cooperative task approval checkpoints
---

# ahu system architecture

This diagram follows the main paths through the CLI. Agent behavior occurs in a
provider harness; ahu prepares and records the session, while the MCP server
exposes ahu tools to connected agents. Telemetry and task records provide
complementary evidence for evaluation. Neither proves that the harness loaded
all context or followed every instruction.[^main][^launch][^mcp][^telemetry][^eval]

```mermaid
flowchart LR
    Operator["Operator / ahu CLI"] --> CLI["CLI dispatch"]
    CLI --> Doctor["Doctor and readiness"]
    CLI --> LockCmd["ahu lock --update"]
    LockCmd --> LockFile["ahu.lock"]
    Git["Committed recognized context"] --> Admission["Launch admission"]
    LockFile --> Admission
    CLI --> Launch["ahu @agent / headless"]
    Launch --> Admission
    Admission -->|"tracked, clean, lock matches"| Plan["Resolve agent, harness, model"]
    Admission -->|"missing, stale, dirty, unsafe"| Refuse["Refuse before launch"]
    Plan --> Snapshot["Bounded context snapshot"]
    Snapshot --> Worktree["Task worktree and task record"]
    Worktree --> Harness["Claude / Codex / OpenCode / other harness"]
    Harness --> MCPClient["Agent MCP client"]

    CLI --> MCPServer["ahu mcp serve"]
    MCPClient <-->|"tools, resources, prompts"| MCPServer
    MCPServer --> Tools["ahu MCP tools"]
    MCPServer --> Decision["Typed decision interface"]
    Decision --> Backend["Configured model backend"]

    MCPServer --> OTel["Opt-in local OpenTelemetry"]
    Harness --> OTel
    Worktree --> Records["Task outcomes and usage"]
    OTel --> EvalData["Evaluation evidence"]
    Records --> EvalData
    CLI --> EvalRun["ahu eval run / report"]
    EvalRun --> Candidate["Candidate ahu agent"]
    EvalRun --> Evaluator["Evaluator ahu agent or grader"]
    EvalData --> EvalRun
    EvalRun --> Comparison["Compare agents, cases and runs"]
```

## Reading the boundaries

`ahu.lock` gates use of recognized project context. `ahu lock --update` writes a
candidate lock; an operator reviews and commits it with the context files. The
lock does not contain per-user settings. Recognized local settings that affect
agent behavior are separately fingerprinted in owner-only host state per user
and repository; changing them requires that user to accept the new fingerprint.
Neither lock covers managed provider policy, built-in harness prompts, or
context the bounded scan cannot identify.[^lock][^launch]

The CLI launches provider harnesses and hosts ahu's MCP server as separate
interfaces. A harness can call MCP tools while it works. The typed decision
interface validates structured requests and can route them to a configured
service; it is independent of the candidate agent's harness model.[^mcp][^decisions]

OTel records bounded process and tool-call evidence when telemetry is enabled.
Task records capture ahu-known lifecycle and outcome details. The eval runner
joins available evidence with case expectations and evaluator scores. Missing
spans mean unknown coverage; they do not mean the agent did not use a tool. Human
review of traces and cases is still needed when changing instructions, skills,
or tools.[^telemetry][^eval][^eval-run][^state]

Local auth profiles opt the project into identity checks before launch/startup
and resume. Tasks record the profile and identity fingerprint. Resume compares
both with the active binding and observed native login; changing the binding
does not migrate a task. Without profiles, this guard is inactive. Readiness
reports identity/binding states without principals or credentials. These checks
are best effort, not an atomic credential lock: sign-in can change after a
probe. OpenCode cloud-model checks assume the model uses the loopback Ollama
daemon selected by `OLLAMA_HOST`; they do not verify OpenCode's effective
provider endpoint.[^auth]

The task-bound `ahu_request_approval` MCP tool records a cooperative checkpoint
and waits for `ahu approve TASK` or `ahu reject TASK`. Rejection or expiry
requests cancellation. The checkpoint does not intercept arbitrary tool calls,
shell commands, or filesystem writes, and does not expand native harness
permissions.[^approval]

[^main]: CLI routing in src/main.rs.
[^launch]: Launch admission and task planning in src/launch.rs.
[^lock]: Lock generation and checks in src/context_lock.rs.
[^mcp]: MCP registration and protocol handling in `src/mcp.rs`.
[^decisions]: Typed decision validation in src/mcp_decisions.rs.
[^telemetry]: Instrumentation in src/telemetry.rs.
[^eval]: Evaluation CLI in src/eval.rs.
[^eval-run]: Trial orchestration in src/eval.rs.
[^state]: Task records in src/task.rs.
[^auth]: Profile, readiness, launch, and resume checks in src/auth_binding.rs.
[^approval]: Explicit request and operator decision lifecycle in src/approval.rs.

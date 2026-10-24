---
okf_version: "0.2"
type: ahu:plugin
title: Experimental Codex launch planner
schema_version: 1
id: harness-codex
package: ahu-experiment-harness-codex
host_api: 1
cargo_manifest: Cargo.toml
harness_launch:
  harness: codex
  path: codex.py
  protocol_version: 1
  timeout_ms: 5000
---

Experimental launch planning only; not a session host or one-shot action.
The planner does not execute Codex, read native configuration or receive prompts.

---
okf_version: "0.2"
type: ahu:plugin
title: claude-code launch planner
schema_version: 1
id: harness-claude-code
package: ahu-plugin-claude-code
host_api: 1
cargo_manifest: Cargo.toml
harness_launch:
  harness: claude-code
  program: claude
  path: bin/ahu-plugin-claude-code
  protocol_version: 2
  timeout_ms: 5000
  tested_versions: ["2.1.310"]
---

Launch planning only. No native execution, configuration reads, or prompt access.
The tested version identifies implementation scope, not all-mode live certification.

---
okf_version: "0.2"
type: ahu:plugin
title: codex launch planner
schema_version: 1
id: harness-codex
package: ahu-plugin-codex
host_api: 1
cargo_manifest: Cargo.toml
harness_launch:
  harness: codex
  program: codex
  path: bin/ahu-plugin-codex
  protocol_version: 2
  timeout_ms: 5000
  tested_versions: ["0.163.0"]
---

Launch planning only. No native execution, configuration reads, or prompt access.
The tested version identifies implementation scope, not all-mode live certification.

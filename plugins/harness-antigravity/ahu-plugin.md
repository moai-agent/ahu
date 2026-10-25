---
okf_version: "0.2"
type: ahu:plugin
title: antigravity launch planner
schema_version: 1
id: harness-antigravity
package: ahu-plugin-antigravity
host_api: 1
cargo_manifest: Cargo.toml
harness_launch:
  harness: antigravity
  program: agy
  path: bin/ahu-plugin-antigravity
  protocol_version: 2
  timeout_ms: 5000
  tested_versions: ["1.4.7"]
---

Launch planning only. No native execution, configuration reads, or prompt access.
The tested version identifies implementation scope, not all-mode live certification.

---
okf_version: "0.2"
type: ahu:plugin
title: opencode launch planner
schema_version: 1
id: harness-opencode
package: ahu-plugin-opencode
host_api: 1
cargo_manifest: Cargo.toml
harness_launch:
  harness: opencode
  program: opencode
  path: bin/ahu-plugin-opencode
  protocol_version: 2
  timeout_ms: 5000
  tested_versions: ["2.0.32"]
---

Launch planning only. No native execution, configuration reads, or prompt access.
The tested version identifies implementation scope, not all-mode live certification.

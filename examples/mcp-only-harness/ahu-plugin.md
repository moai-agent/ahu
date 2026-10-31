---
okf_version: "0.2"
type: ahu:plugin
title: Example MCP-only harness
schema_version: 1
id: harness-sixth
package: ahu-plugin-sixth
host_api: 1
cargo_manifest: Cargo.toml
harness_runtime:
  protocol_version: 1
  timeout_ms: 8000
  descriptor:
    id: sixth
    display_name: Sixth
    program: sixth
    aliases: []
    modes: [direct]
    operations: [probe, launch]
    capabilities: [mcp-only]
---

Declarative harness with no plugin code. Copy this directory, change `id`,
`package`, `program` and `display_name`, and register `ahu mcp serve` in the
harness's own MCP configuration. ahu offers a direct session and a version probe.

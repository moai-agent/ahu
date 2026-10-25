---
okf_version: "0.2"
type: ahu:plugin
title: claude-code harness runtime
schema_version: 1
id: harness-claude-code
package: ahu-plugin-claude-code
host_api: 1
cargo_manifest: Cargo.toml
harness_runtime:
  protocol_version: 1
  path: bin/ahu-plugin-claude-code
  timeout_ms: 30000
  descriptor:
    id: claude-code
    display_name: claude-code
    program: claude
    aliases: ["claude"]
    models: []
    modes: [interactive, headless, direct]
    operations: [probe, models, launch, admission, setup, native_sources, auth, budget, trust_prepare, trust_cleanup, decode_start, decode_event, decode_finish]
    capabilities: []
    tested_versions: ["2.1.310"]
    context_paths: [".claude/", "CLAUDE.md", ".mcp.json"]
    skill_paths: [".claude/skills/"]
    native_source_paths: []
---

Installed native runtime. Discovery reads this descriptor without executing it.
Native operations require an explicit harness-runtime grant.
Tested versions name implementation evidence, not live account certification.

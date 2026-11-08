---
okf_version: "0.2"
type: ahu:plugin
title: codex harness runtime
schema_version: 1
id: harness-codex
package: ahu-plugin-codex
host_api: 1
cargo_manifest: Cargo.toml
harness_runtime:
  protocol_version: 1
  path: bin/ahu-plugin-codex
  timeout_ms: 30000
  descriptor:
    id: codex
    display_name: codex
    program: codex
    aliases: []
    models: []
    modes: [interactive, headless, direct]
    operations: [probe, models, launch, admission, setup, native_sources, auth, budget, trust_prepare, trust_cleanup, decode_start, decode_event, decode_finish]
    environment_keys: ["CODEX_HOME", "XDG_CONFIG_HOME", "OPENAI_API_KEY", "OPENAI_BASE_URL"]
    capabilities: [decode-event-batches-v1]
    tested_versions: ["0.163.0"]
    context_paths: [".codex/", "AGENTS.md"]
    private_context_paths: []
    skill_paths: [".agents/skills/"]
    native_source_paths: []
---

Installed native runtime. Discovery reads this descriptor without executing it.
Native operations require an explicit harness-runtime grant.
Tested versions name implementation evidence, not live account certification.

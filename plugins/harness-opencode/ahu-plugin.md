---
okf_version: "0.2"
type: ahu:plugin
title: opencode harness runtime
schema_version: 1
id: harness-opencode
package: ahu-plugin-opencode
host_api: 1
cargo_manifest: Cargo.toml
harness_runtime:
  protocol_version: 1
  path: bin/ahu-plugin-opencode
  timeout_ms: 30000
  descriptor:
    id: opencode
    display_name: opencode
    program: opencode
    aliases: ["ollama"]
    models: []
    modes: [interactive, headless, direct]
    operations: [probe, models, launch, admission, setup, native_sources, auth, budget, decode_start, decode_event, decode_finish, recover]
    environment_keys: ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "OPENCODE_CONFIG", "OPENCODE_CONFIG_DIR", "OPENCODE_CONFIG_CONTENT", "OLLAMA_HOST", "OLLAMA_API_KEY", "OPENAI_API_KEY", "ANTHROPIC_API_KEY"]
    capabilities: [decode-event-batches-v1]
    tested_versions: ["2.0.32"]
    context_paths: [".opencode/", "opencode.json", "opencode.jsonc"]
    private_context_paths: []
    skill_paths: [".agents/skills/", ".opencode/skills/"]
    native_source_paths: []
---

Installed native runtime. Discovery reads this descriptor without executing it.
Native operations require an explicit harness-runtime grant.
Tested versions name implementation evidence, not live account certification.

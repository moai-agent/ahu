---
okf_version: "0.2"
type: ahu:plugin
title: antigravity harness runtime
schema_version: 1
id: harness-antigravity
package: ahu-plugin-antigravity
host_api: 1
cargo_manifest: Cargo.toml
harness_runtime:
  protocol_version: 1
  path: bin/ahu-plugin-antigravity
  timeout_ms: 30000
  descriptor:
    id: antigravity
    display_name: antigravity
    program: agy
    aliases: ["agy"]
    models: []
    modes: [interactive, headless, direct]
    operations: [probe, models, launch, admission, setup, native_sources, auth, budget, trust_prepare, trust_cleanup, decode_start, decode_event, decode_finish]
    environment_keys: ["GEMINI_CLI_HOME", "AGY_CONFIG_DIR", "XDG_CONFIG_HOME", "GOOGLE_APPLICATION_CREDENTIALS", "GEMINI_API_KEY", "GOOGLE_API_KEY"]
    capabilities: [decode-event-batches-v1]
    tested_versions: ["1.4.7"]
    context_paths: [".gemini/", "GEMINI.md", ".agy/"]
    private_context_paths: []
    skill_paths: [".agents/skills/"]
    native_source_paths: []
---

Installed native runtime. Discovery reads this descriptor without executing it.
Native operations require an explicit harness-runtime grant.
Tested versions name implementation evidence, not live account certification.

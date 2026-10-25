---
okf_version: "0.2"
type: ahu:plugin
title: kiro harness runtime
schema_version: 1
id: harness-kiro
package: ahu-plugin-kiro
host_api: 1
cargo_manifest: Cargo.toml
harness_runtime:
  protocol_version: 1
  path: bin/ahu-plugin-kiro
  timeout_ms: 30000
  descriptor:
    id: kiro
    display_name: kiro
    program: kiro-cli
    aliases: []
    models: []
    modes: [interactive, headless, direct]
    operations: [probe, models, launch, admission, setup, native_sources, auth, budget, decode_start, decode_event, decode_finish]
    environment_keys: ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "KIRO_API_KEY", "AWS_PROFILE", "AWS_REGION", "AWS_DEFAULT_REGION"]
    capabilities: [decode-event-batches-v1]
    tested_versions: ["2.29.0", "2.34.0"]
    context_paths: [".kiro/", ".kiroignore"]
    private_context_paths: []
    skill_paths: [".agents/skills/"]
    native_source_paths: []
---

Installed native runtime. Discovery reads this descriptor without executing it.
Native operations require an explicit harness-runtime grant.
Tested versions name implementation evidence, not live account certification.

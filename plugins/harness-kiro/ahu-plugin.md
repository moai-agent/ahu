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
    operations: [probe, models, launch, admission, setup, native_sources, auth, budget, trust_prepare, trust_cleanup, decode_start, decode_event, decode_finish]
    capabilities: []
    tested_versions: ["2.29.0"]
    context_paths: [".kiro/"]
    skill_paths: [".agents/skills/"]
    native_source_paths: []
---

Installed native runtime. Discovery reads this descriptor without executing it.
Native operations require an explicit harness-runtime grant.
Tested versions name implementation evidence, not live account certification.

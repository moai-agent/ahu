---
ahu:
  catalog_version: "2026-10-09"
  harness_preferences:
  - antigravity
  - codex
  harness_version_pins: {}
  headless:
    trust_opencode_native_state: true
  knowledge:
    bundles:
    - docs/knowledge
    - .agents/ahu/agents
    fail_on_warnings: false
  model_rankings:
    antigravity:
    - gemini-3.1-pro-high
    codex:
    - gpt-6-astra
    kiro:
    - claude-sonnet-4.5
    opencode:
    - ollama/glm-5.3:cloud
  model_selection: project-ranked
  schema_version: 1
  telemetry:
    enabled: true
    endpoint: http://127.0.0.1:4318
    local_metrics: false
okf_version: '0.2'
type: ahu:project
---

# Project policy

One policy for every ahu user in this project. Review and commit this file.

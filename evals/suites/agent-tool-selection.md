---
okf_version: "0.2"
type: ahu:eval-suite
schema_version: 1
id: agent-tool-selection
version: "1.1.0"
cases:
  - path: ../cases/decision-routing.md
    weight: 1.0
  - path: ../cases/direct-extraction.md
    weight: 1.0
  - path: ../cases/priority-triage.md
    weight: 1.0
---

Three synthetic cases that pull in opposite directions on the same tool. Two
require typed judgments, including one that applies a short policy; one states
its answers outright and expects the tool to be left alone. The suite checks
whether an agent can use the tool for useful judgments and abstain on direct
extraction.

Case paths are relative to this file. Weights are equal here because neither
behavior is more important to get right in this starter suite.

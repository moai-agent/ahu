---
okf_version: "0.2"
type: ahu:eval-suite
schema_version: 1
id: agent-tool-selection
version: "1.0.0"
cases:
  - path: ../cases/decision-routing.md
    weight: 1.0
  - path: ../cases/direct-extraction.md
    weight: 1.0
---

Two synthetic cases that pull in opposite directions on the same tool. One is a
pair of typed judgements and expects the typed-decision tool to be used; the
other states its answers outright and expects that tool to be left alone. An
agent that always reaches for the tool, and an agent that never does, each fail
one half of this suite, so the pair measures selection rather than enthusiasm.

Case paths are relative to this file. Weights are equal here because neither
behaviour is the more important one to get right.

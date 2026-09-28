---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "08-discovery-and-jobs",
  "corpus_version": "1.0.0",
  "state": {
    "project": "Harbor synthetic service",
    "connection": "MCP transport connected; initialize succeeded; tools/list timed out. No tools/call was sent.",
    "jobs": "Job J1 has a durable completion receipt. J2 has no receipt and has idempotency key K2. J3 has neither receipt nor key.",
    "request": "Record the diagnostic next step and the recovery disposition for each job."
  },
  "questions": {
    "diagnosis": {
      "type": "choice",
      "instructions": "Choose the diagnostic record.",
      "options": {
        "a": "Successful tool execution; discard stderr.",
        "b": "Capability-missing because tools/list returned an empty list.",
        "c": "Discovery-blocked; capture the tools/list timeout and server stderr; no tool call attempted."
      }
    },
    "jobs": {
      "type": "choice",
      "instructions": "Choose the disposition of J1, J2, and J3.",
      "options": {
        "a": "Acknowledge J1 without replay; replay J2 once with K2; quarantine J3.",
        "b": "Replay every job once after reconnecting.",
        "c": "Quarantine J1; replay J2 with a new key; acknowledge J3."
      }
    }
  },
  "expected": {
    "diagnosis": "c",
    "jobs": "a"
  },
  "scoring": {
    "diagnosis": 1.0,
    "jobs": 1.0,
    "exact_match_pass_threshold": 1.0
  }
}
---

Classify a dependency discovery failure and plan disposition of interrupted jobs.

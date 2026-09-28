---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "04-recovered-worker",
  "corpus_version": "1.0.0",
  "state": {
    "project": "Harbor synthetic service",
    "operation": "op-demo-4",
    "history": "Attempts 1 and 2 fail transiently; attempt 3 succeeds.",
    "request": "Choose the event configuration for this recovered operation, without exporting payloads."
  },
  "questions": {
    "severity": {
      "type": "choice",
      "instructions": "Choose the event sequence.",
      "options": {
        "a": "DEBUG for attempts 1 and 2; one INFO for recovered success.",
        "b": "ERROR for attempts 1 and 2; no recovered-success event.",
        "c": "INFO for every retry and every success."
      }
    },
    "correlation": {
      "type": "choice",
      "instructions": "Choose the required fields on each event.",
      "options": {
        "a": "Only a free-form message.",
        "b": "operation_id and attempt.",
        "c": "The complete customer payload and its email."
      }
    }
  },
  "expected": {
    "severity": "a",
    "correlation": "b"
  },
  "scoring": {
    "severity": 1.0,
    "correlation": 1.0,
    "exact_match_pass_threshold": 1.0
  }
}
---

Configure event severity and correlation for a recovered worker operation.

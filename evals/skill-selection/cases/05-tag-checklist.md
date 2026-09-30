---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "05-tag-checklist",
  "corpus_version": "1.0.0",
  "state": {
    "project": "Harbor synthetic service",
    "candidate_digest": "demo-digest-B",
    "linux": "Pass for demo-digest-B",
    "macos": "Infrastructure timeout for demo-digest-B; previous demo-digest-A passed.",
    "artifact": "The artifact digest matches demo-digest-B.",
    "request": "Decide whether to tag this candidate and what evidence to obtain."
  },
  "questions": {
    "decision": {
      "type": "choice",
      "instructions": "Choose the tag decision.",
      "options": {
        "a": "Tag because Linux passed.",
        "b": "Tag using the previous macOS result.",
        "c": "Hold the tag because current macOS evidence is missing."
      }
    },
    "followup": {
      "type": "choice",
      "instructions": "Choose the minimal follow-up.",
      "options": {
        "a": "Rerun macOS for demo-digest-B and record its result with the existing digest and Linux result.",
        "b": "Rebuild a different digest and reuse the Linux result.",
        "c": "Treat the timeout as a passing check."
      }
    }
  },
  "expected": {
    "decision": "c",
    "followup": "a"
  },
  "scoring": {
    "decision": 1.0,
    "followup": 1.0,
    "exact_match_pass_threshold": 1.0
  }
}
---

Make a tag readiness decision and identify the missing verification.

---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "10-status-copy",
  "corpus_version": "1.0.0",
  "state": {
    "items": {
      "release": "waiting",
      "migration": "ready"
    },
    "instruction": "This is a literal transcription task. Do not infer readiness or perform a review."
  },
  "questions": {
    "release_status": {
      "type": "choice",
      "instructions": "Copy the release item's status.",
      "options": {
        "a": "ready",
        "b": "waiting",
        "c": "closed"
      }
    },
    "migration_status": {
      "type": "choice",
      "instructions": "Copy the migration item's status.",
      "options": {
        "a": "ready",
        "b": "waiting",
        "c": "closed"
      }
    }
  },
  "expected": {
    "release_status": "b",
    "migration_status": "a"
  },
  "scoring": {
    "release_status": 1.0,
    "migration_status": 1.0,
    "exact_match_pass_threshold": 1.0
  }
}
---

Copy the explicitly supplied statuses into the requested JSON fields.

---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "07-version-recovery",
  "corpus_version": "1.0.0",
  "state": {
    "project": "Harbor synthetic service",
    "incident": "The newly promoted application fails its synthetic health probe; pause promotion.",
    "active_database_epoch": 7,
    "artifacts": "B12 is known-good, schema floor 8. B11 is known-good, schema floor 6, representation r4. B10 is known-good, schema floor 5, representation r3.",
    "cache": "The failed application used r5. The old r4 namespace is retired and may contain stale values.",
    "request": "Plan application recovery; no database downgrade or release tag is requested."
  },
  "questions": {
    "target": {
      "type": "choice",
      "instructions": "Choose the recovery artifact.",
      "options": {
        "a": "B12 because it is the newest known-good artifact.",
        "b": "B11 because it is the newest known-good artifact compatible with epoch 7.",
        "c": "B10 because the oldest artifact is always preferred."
      }
    },
    "cache": {
      "type": "choice",
      "instructions": "Choose the cache action before switching traffic.",
      "options": {
        "a": "Create a fresh r4 namespace and leave the abandoned namespace to its 20-minute TTL.",
        "b": "Reuse the retired r4 namespace.",
        "c": "Flush every tenant cache and downgrade the database."
      }
    },
    "verification": {
      "type": "choice",
      "instructions": "Choose the recovery finish.",
      "options": {
        "a": "Reverse all database migrations, then switch traffic.",
        "b": "Declare recovery immediately after selecting an artifact.",
        "c": "Switch traffic to the compatible artifact and verify the synthetic health probe."
      }
    }
  },
  "expected": {
    "target": "b",
    "cache": "a",
    "verification": "c"
  },
  "scoring": {
    "target": 1.0,
    "cache": 1.0,
    "verification": 1.0,
    "exact_match_pass_threshold": 1.0
  }
}
---

Choose a recovery target and cache transition before returning traffic to a healthy application.

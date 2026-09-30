---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "03-support-export",
  "corpus_version": "1.0.0",
  "state": {
    "project": "Harbor synthetic service",
    "record": {
      "email": "pat@example.invalid",
      "tenant": "sample-blue",
      "request_id": "req-demo-7"
    },
    "batch": "This is the first record of a new batch. A previous batch used tenant-9 for sample-blue.",
    "request": "Produce the support-export configuration, preserving permitted correlation."
  },
  "questions": {
    "fields": {
      "type": "choice",
      "instructions": "Choose the transformed record.",
      "options": {
        "a": "email=[email]; tenant=tenant-9; request_id=req-demo-7",
        "b": "email=[email]; tenant=tenant-1; request_id=req-demo-7",
        "c": "email=pat@example.invalid; tenant=tenant-1; request_id=[request]"
      }
    },
    "alias_scope": {
      "type": "choice",
      "instructions": "Choose the alias-map lifetime.",
      "options": {
        "a": "Persist one alias map across all exports.",
        "b": "Allocate a new alias for every record, including equal tenants.",
        "c": "Reuse aliases within this batch and discard the map before the next batch."
      }
    }
  },
  "expected": {
    "fields": "b",
    "alias_scope": "c"
  },
  "scoring": {
    "fields": 1.0,
    "alias_scope": 1.0,
    "exact_match_pass_threshold": 1.0
  }
}
---

Choose field transformations and alias scope for a synthetic support export.

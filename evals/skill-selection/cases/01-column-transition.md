---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "01-column-transition",
  "corpus_version": "1.0.0",
  "state": {
    "project": "Harbor synthetic service",
    "change": "Stored orders.zone becomes orders.region; the public response is unchanged.",
    "phase": "The new nullable column, dual writes, and backfill are complete.",
    "evidence": "One reconciliation run has zero mismatches. Both writer versions remain deployed."
  },
  "questions": {
    "next_gate": {
      "type": "choice",
      "instructions": "Select the remaining evidence before the old stored column may be retired.",
      "options": {
        "a": "Retire immediately after the completed clean reconciliation.",
        "b": "Complete a second reconciliation run with zero mismatches.",
        "c": "Publish two API minor-version deprecation notices."
      }
    },
    "early_plan": {
      "type": "choice",
      "instructions": "Select the project's initial ordering for this transition.",
      "options": {
        "a": "Rename the column in place, then update writers.",
        "b": "Drop the old column, add the new column, backfill.",
        "c": "Add nullable, dual-write, backfill, reconcile, switch-read."
      }
    }
  },
  "expected": {
    "next_gate": "b",
    "early_plan": "c"
  },
  "scoring": {
    "next_gate": 1.0,
    "early_plan": 1.0,
    "exact_match_pass_threshold": 1.0
  }
}
---

Complete the storage transition checklist for renaming a stored field while two writer versions coexist.

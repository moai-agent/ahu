---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "12-config-copy",
  "corpus_version": "1.0.0",
  "state": {
    "before": {
      "retries": 2,
      "enabled": false
    },
    "edits": "Set retries to 4 and enabled to true. These literal edits are the whole request; make no policy decision about deployment, logging, or recovery."
  },
  "questions": {
    "retries": {
      "type": "choice",
      "instructions": "Choose the resulting retries value.",
      "options": {
        "a": "2",
        "b": "4",
        "c": "8"
      }
    },
    "enabled": {
      "type": "choice",
      "instructions": "Choose the resulting enabled value.",
      "options": {
        "a": "false",
        "b": "null",
        "c": "true"
      }
    }
  },
  "expected": {
    "retries": "b",
    "enabled": "c"
  },
  "scoring": {
    "retries": 1.0,
    "enabled": 1.0,
    "exact_match_pass_threshold": 1.0
  }
}
---

Apply the two explicit edits to a supplied configuration fragment.

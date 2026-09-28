---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "02-response-rename",
  "corpus_version": "1.0.0",
  "state": {
    "project": "Harbor synthetic service",
    "change": "Rename public v1 field zone to region; internal storage is unchanged.",
    "evidence": "The oldest supported client fixture passes; only one minor-version notice has been published.",
    "request": "Keep existing v1 consumers working and specify the retirement gate."
  },
  "questions": {
    "v1_shape": {
      "type": "choice",
      "instructions": "Choose the v1 response shape.",
      "options": {
        "a": "Emit both zone and region.",
        "b": "Emit only region.",
        "c": "Remove both names until v2."
      }
    },
    "retirement": {
      "type": "choice",
      "instructions": "Choose when the old public name can be removed.",
      "options": {
        "a": "In v1 now, because the oldest-client fixture passes.",
        "b": "After two clean database reconciliation runs.",
        "c": "In v2 after two published minor-version notices and a passing oldest-client fixture."
      }
    }
  },
  "expected": {
    "v1_shape": "a",
    "retirement": "c"
  },
  "scoring": {
    "v1_shape": 1.0,
    "retirement": 1.0,
    "exact_match_pass_threshold": 1.0
  }
}
---

Choose the release contract for a renamed public response field.

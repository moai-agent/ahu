---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "09-client-package",
  "corpus_version": "1.0.0",
  "state": {
    "project": "Harbor synthetic service",
    "upgrade": "A minor client-library update changes a transitive serializer.",
    "wire_change": "Rename v1 response field zone to region; storage is unchanged.",
    "docs": "Move /reference/orders to /reference/order-api; the old page has a #response anchor.",
    "request": "Prepare the combined review decisions for dependency acceptance, existing consumers, and existing documentation links."
  },
  "questions": {
    "checks": {
      "type": "choice",
      "instructions": "Choose the dependency checks.",
      "options": {
        "a": "Hand-edit checksums and run only the newest runtime.",
        "b": "Regenerate the lockfile, run the minimum-runtime smoke check, and run the golden-wire fixture.",
        "c": "Skip wire fixtures because the top-level update is minor."
      }
    },
    "wire": {
      "type": "choice",
      "instructions": "Choose the supported v1 response and example shape.",
      "options": {
        "a": "Show only region in the response and examples.",
        "b": "Remove both names from examples until v2.",
        "c": "Emit both zone and region and show both supported names in the v1 examples."
      }
    },
    "links": {
      "type": "choice",
      "instructions": "Choose the documentation transition.",
      "options": {
        "a": "Keep the old-path redirect through two published documentation releases, preserve #response at the destination, and update navigation in the same change.",
        "b": "Delete the old path immediately and keep navigation unchanged.",
        "c": "Keep the old page but remove #response from the destination."
      }
    }
  },
  "expected": {
    "checks": "b",
    "wire": "c",
    "links": "a"
  },
  "scoring": {
    "checks": 1.0,
    "wire": 1.0,
    "links": 1.0,
    "exact_match_pass_threshold": 1.0
  }
}
---

Choose the library acceptance checks, v1 response contract, and documentation move checklist.

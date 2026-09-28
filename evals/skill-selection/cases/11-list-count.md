---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "11-list-count",
  "corpus_version": "1.0.0",
  "state": {
    "files": [
      "notes.md",
      "guide.md",
      "logo.svg",
      "data.json"
    ],
    "rule": "Count entries ending in .md and choose the lexicographically first complete filename. No project conventions are needed."
  },
  "questions": {
    "markdown_count": {
      "type": "choice",
      "instructions": "Choose the number of .md entries.",
      "options": {
        "a": "1",
        "b": "3",
        "c": "2"
      }
    },
    "first_filename": {
      "type": "choice",
      "instructions": "Choose the lexicographically first filename.",
      "options": {
        "a": "data.json",
        "b": "guide.md",
        "c": "logo.svg"
      }
    }
  },
  "expected": {
    "markdown_count": "c",
    "first_filename": "a"
  },
  "scoring": {
    "markdown_count": 1.0,
    "first_filename": 1.0,
    "exact_match_pass_threshold": 1.0
  }
}
---

Compute a small summary of the supplied file list.

---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "06-trial-report",
  "corpus_version": "1.0.0",
  "state": {
    "project": "Harbor synthetic service",
    "attempts": 10,
    "valid_answers": 8,
    "passing_answers": 6,
    "timing": "Durations were observed for seven trials; their sum is 70 seconds.",
    "request": "Keep answer reliability, answer quality, and measured latency distinct."
  },
  "questions": {
    "rates": {
      "type": "choice",
      "instructions": "Choose the report's rate pair.",
      "options": {
        "a": "Reliability 6/8; answer quality 6/10.",
        "b": "Reliability 6/10; answer quality 6/8.",
        "c": "Reliability 8/10; answer quality 6/10."
      }
    },
    "latency": {
      "type": "choice",
      "instructions": "Choose the latency statement.",
      "options": {
        "a": "Mean 7 seconds over all ten attempts, assigning zero to missing timing.",
        "b": "Mean 10 seconds over all ten attempts.",
        "c": "Mean 10 seconds over seven observed timings; three timings missing."
      }
    }
  },
  "expected": {
    "rates": "b",
    "latency": "c"
  },
  "scoring": {
    "rates": 1.0,
    "latency": 1.0,
    "exact_match_pass_threshold": 1.0
  }
}
---

Select the denominator and timing statements for a synthetic trial report.

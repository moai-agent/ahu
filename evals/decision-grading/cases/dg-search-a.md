---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "dg-search-a",
  "corpus_version": "1.0.0",
  "state": {
    "user_request": "Assess a fictional help-center search rollout with mixed feedback. Choose a rollout action, a justified interpretation, and the next evaluation.",
    "evidence": [
      "A fictional drawing app is trying a revised ranking rule for its help-center search. The change boosts articles with exact title matches. In a small internal trial, employees found a known shortcut article faster when they typed its title. The trial used a prepared list of exact-title queries. Several employees said the results felt cleaner. No trial queries described a problem in everyday language, and the trial did not include users who did not already know article names.",
      "Support supplied a separate set of anonymized synthetic queries based on common themes, such as asking why a brush leaves gaps. The relevant article is titled Spacing controls, so an exact-title rule might not help that query. The team has not run this set through the new ranker. An older support note mentions a temporary search outage, but both the baseline and proposed rankers currently return results in the local test environment. Availability does not settle ranking quality.",
      "The product owner can authorize a limited reversible preview for volunteer testers after an offline relevance check. They have not authorized a global default change or deleting the previous ranking configuration. The team has a simple rollback switch and can collect whether the first few results answer a query, rather than treating a click alone as satisfaction. The prepared title-query trial gives a useful but narrow success signal.",
      "A draft announcement says search now understands users' questions better. That is broader than the measured title lookup behavior. The search maintainer wants a recommendation that retains the demonstrated benefit while examining the untested problem-query audience. Useful validation would compare both rankers on title and descriptive queries, with judgments of whether returned articles resolve the stated need. The team should decide whether to widen the preview only after inspecting those contrasting query types and preserving an easy rollback."
    ]
  },
  "questions": {
    "action": {
      "type": "choice",
      "instructions": "Which action best addresses the request within the stated authority?",
      "options": {
        "C": "Compare title and descriptive query relevance, then use a reversible volunteer preview if results support it.",
        "A": "Make the new ranker the global default and delete rollback because employees preferred the prepared title queries.",
        "B": "Keep a volunteer preview limited to exact-title queries and postpone evaluating problem descriptions."
      }
    },
    "claim": {
      "type": "choice",
      "instructions": "Which statement is most strongly justified by the available evidence?",
      "options": {
        "A": "The trial supports better known-title lookup for its sample, while improvements for descriptive help requests remain untested.",
        "B": "The title trial is encouraging, so claim that general help search is probably better without testing descriptive requests.",
        "C": "Employee preference proves the new ranker understands every user question and cannot reduce relevance."
      }
    },
    "validation": {
      "type": "choice",
      "instructions": "Which next validation best resolves the important remaining uncertainty?",
      "options": {
        "B": "Compare both rankers on title and descriptive queries, judging whether leading results answer each need and checking regressions.",
        "A": "Check only that the search endpoint returns a successful status code and infer relevance from availability.",
        "C": "Repeat the exact-title trial with more employees and record their preferred results."
      }
    }
  },
  "expected": {
    "action": "C",
    "claim": "A",
    "validation": "B"
  },
  "scoring": {
    "action": 1.0,
    "claim": 1.0,
    "validation": 1.0,
    "exact_match_pass_threshold": 0.75
  },
  "rubric": {
    "action": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): evaluates distinct query needs before a reversible, authorized preview and keeps rollback available. Partially satisfies (0.5): uses a limited preview but leaves the untested descriptive-query audience unexamined. Does not satisfy (0): widens globally without evidence or removes the rollback needed for controlled rollout. Use these three reference levels; intermediate scores are allowed. Do not grade by option key.",
    "claim": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): limits observed benefit to sampled known-title queries and names descriptive-query uncertainty. Partially satisfies (0.5): recognizes the narrow trial but still generalizes improvement beyond the measured audience. Does not satisfy (0): claims universal understanding or absence of regressions from employee preference. Use these three reference levels; intermediate scores are allowed. Do not grade by option key.",
    "validation": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): compares baseline and proposal across query types using answer relevance and regression judgments. Partially satisfies (0.5): expands sampling within title queries only and misses the untested use case. Does not satisfy (0): equates endpoint availability with search relevance. Use these three reference levels; intermediate scores are allowed. Do not grade by option key."
  }
}
---

Choose a response to this fictional software, support, or documentation request using the supplied evidence.

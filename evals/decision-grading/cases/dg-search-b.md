---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "dg-search-b",
  "corpus_version": "1.0.0",
  "state": {
    "user_request": "Choose a response to a fictional help-search synonym rollout that sometimes changes literal code examples, a defensible claim, and next validation.",
    "evidence": [
      "A fictional build tool's help center recently added synonyms for everyday terms in search queries. Queries for undo now find articles about reverting an edit. In a local evaluation, volunteers using conversational queries found relevant help more often. The synonym processor, however, runs on all query text before the parser identifies quoted literals. A maintainer noticed that a search for a quoted configuration key containing the word undo produced results for a different key containing revert.",
      "The team reproduced this with synthetic documentation for two distinct configuration keys. The quoted undo key was rewritten before lookup and its own article fell below unrelated results. The baseline ranker preserved the literal and returned the matching article first. Ordinary unquoted undo queries still benefited from synonyms in the same fixture. The report therefore contains both a demonstrated regression for literal intent and evidence that the feature can be useful for a different query class.",
      "A draft fix excludes every query containing a quotation mark from all ranking enhancements. That preserves the reproduced literal, but may also remove unrelated ranking improvements from mixed queries that contain a literal plus a natural-language problem. A more focused proposal parses quoted spans first and applies synonyms only outside them. It has not yet been tested with escaped quotation marks or incomplete quotes. The team can pause the synonym feature with its existing reversible switch while preparing a local fix.",
      "The product owner has authorized that pause and a focused repair, but not deleting synonym support permanently or changing how configuration keys are interpreted by the build tool. A separate complaint about stale help-page screenshots concerns article content rather than literal query transformation. The maintainer asks for a response that protects literal intent without erasing the useful conversational behavior and seeks evidence around quoted, unquoted and mixed requests before re-enabling the repaired feature."
    ]
  },
  "questions": {
    "action": {
      "type": "choice",
      "instructions": "Which action best addresses the request within the stated authority?",
      "options": {
        "B": "Remove synonym support permanently and announce that the entire ranking feature is useless based on the quoted-key regression.",
        "A": "Pause synonyms using the approved switch, preserve quoted spans before expansion, and validate literal, conversational and mixed queries before re-enabling.",
        "C": "Disable all enhancements whenever a quote appears and retain synonym behavior for entirely unquoted queries."
      }
    },
    "claim": {
      "type": "choice",
      "instructions": "Which statement is most strongly justified by the available evidence?",
      "options": {
        "B": "Pre-parse expansion causes the reproduced literal-key regression, while unquoted synonym benefits remain supported in the fixture.",
        "A": "The quoted-key regression proves synonyms never improve search and explains the unrelated stale screenshots.",
        "C": "Synonyms help conversational queries and quote handling needs work, so describe the feature as ready without mixed-query or escape checks."
      }
    },
    "validation": {
      "type": "choice",
      "instructions": "Which next validation best resolves the important remaining uncertainty?",
      "options": {
        "B": "Retest the original quoted-key example after excluding quoted spans, without checking other query classes.",
        "A": "Compare quoted keys, unquoted requests and mixed queries with escaped or incomplete quotes, checking literal preservation and retained relevance.",
        "C": "Update article screenshots and use visual freshness as proof that literal query intent is preserved."
      }
    }
  },
  "expected": {
    "action": "A",
    "claim": "B",
    "validation": "A"
  },
  "scoring": {
    "action": 1.0,
    "claim": 1.0,
    "validation": 1.0,
    "exact_match_pass_threshold": 0.75
  },
  "rubric": {
    "action": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): uses the authorized reversible pause and a focused literal-preserving repair, validating both benefits and boundary cases before resumption. Partially satisfies (0.5): protects the simple quoted case with an overbroad enhancement exclusion that leaves mixed-query quality unresolved. Does not satisfy (0): permanently removes supported functionality or changes product scope beyond the authorized focused repair. Use these three reference levels; intermediate scores are allowed. Do not grade by option key.",
    "claim": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): identifies expansion before parsing as the reproduced literal defect and retains evidence of unquoted benefits. Partially satisfies (0.5): recognizes both behavior classes but declares readiness without the outstanding boundary validation. Does not satisfy (0): generalizes the regression to all synonym value or attributes unrelated content defects to it. Use these three reference levels; intermediate scores are allowed. Do not grade by option key.",
    "validation": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): checks literal preservation and useful synonym behavior across quoted, unquoted, mixed and malformed-quote boundaries. Partially satisfies (0.5): verifies the single corrected literal query without testing other classes or quoting edges. Does not satisfy (0): treats refreshed article images as evidence of query transformation correctness. Use these three reference levels; intermediate scores are allowed. Do not grade by option key."
  }
}
---

Choose a response to this fictional software, support, or documentation request using the supplied evidence.

---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "dg-pilot-links",
  "corpus_version": "1.0.0",
  "state": {
    "user_request": "Help a fictional tutorial maintainer handle broken section links after a heading rename. Choose an action, a claim, and next validation.",
    "evidence": [
      "A fictional pixel-art editor publishes a tutorial with links from a landing page into specific tutorial sections. A writer renamed the section Saving a palette to Exporting a palette. The documentation generator derives section identifiers from headings. The built tutorial now contains the new identifier, while the landing page still links to the old one. A browser opens the tutorial when following that link but stays at the top instead of scrolling to the intended section.",
      "A local link checker reports a successful page request and does not inspect fragments. The writer initially cited its green result as evidence that the rename was safe. A reviewer inspected the built HTML and confirmed that the old identifier is absent. Replacing the landing-page fragment with the new identifier restores the intended scroll in a local browser. The team has not inventoried inbound links from its other maintained pages or tested the small-screen tutorial layout.",
      "The documentation policy allows a compatibility anchor for renamed sections when maintained links or bookmarks may still use the old name. The maintainer can add that anchor and update owned links. They cannot change users' existing bookmarks. The article's text is accurate, and no application behavior changed with this edit. Another open request asks for brighter palette screenshots, but image color does not determine fragment matching or whether the browser finds a section identifier.",
      "The writer wants a repair that handles both the known landing-page link and existing references to the old section name. A useful check should inspect generated identifiers and navigate to both forms of the fragment, rather than stopping at a successful page fetch. The maintainer also wants an honest status statement: the local example establishes a renamed-anchor mismatch, but it does not prove that every inbound reference has been located or that all layouts have been checked."
    ]
  },
  "questions": {
    "action": {
      "type": "choice",
      "instructions": "Which action best addresses the request within the stated authority?",
      "options": {
        "B": "Remove section links from all tutorials because a successful page request proves fragment navigation cannot be tested.",
        "C": "Update the known landing-page link only, leaving old bookmarks and uninspected maintained links unresolved.",
        "A": "Add a compatibility anchor, update owned links, and check both fragments against the generated tutorial."
      }
    },
    "claim": {
      "type": "choice",
      "instructions": "Which statement is most strongly justified by the available evidence?",
      "options": {
        "A": "The successful page request proves the old fragment still resolves to its section.",
        "B": "The rename removed the old anchor and broke the observed fragment navigation; other inbound references remain uninventoried.",
        "C": "The local fragment replacement works, so declare every inbound link repaired without inventorying them."
      }
    },
    "validation": {
      "type": "choice",
      "instructions": "Which next validation best resolves the important remaining uncertainty?",
      "options": {
        "C": "Navigate through the corrected landing-page link in the current local layout only.",
        "A": "Inspect generated anchors and navigate with old and new fragments, including supported layouts and maintained inbound links.",
        "B": "Check only page status codes and screenshot colors, treating them as evidence that old bookmarks work."
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
    "action": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): preserves old fragment compatibility while updating maintained links and verifying generated targets. Partially satisfies (0.5): repairs the known link but leaves old references without a compatible target. Does not satisfy (0): removes navigation broadly or refuses a feasible fragment-specific repair. Use these three reference levels; intermediate scores are allowed. Do not grade by option key.",
    "claim": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): states the demonstrated anchor mismatch while bounding claims about uninspected inbound links. Partially satisfies (0.5): uses a successful local replacement to overclaim repair of all references. Does not satisfy (0): equates page-fetch success with fragment resolution despite the absent old identifier. Use these three reference levels; intermediate scores are allowed. Do not grade by option key.",
    "validation": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): checks generated targets and actual navigation for old and new fragments across supported contexts. Partially satisfies (0.5): checks the repaired known link but not compatibility or other contexts. Does not satisfy (0): uses HTTP status or visual colors instead of fragment resolution. Use these three reference levels; intermediate scores are allowed. Do not grade by option key."
  }
}
---

Choose a response to this fictional software, support, or documentation request using the supplied evidence.

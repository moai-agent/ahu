---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "dg-cache-a",
  "corpus_version": "1.0.0",
  "state": {
    "user_request": "Our fictional build preview sometimes shows an old banner. Recommend the next action, the strongest justified diagnosis, and a useful validation before changing production.",
    "evidence": [
      "The preview service reads a theme file and renders a banner for each branch. A support engineer opened three tickets after an editor changed a banner from amber to blue. The first preview still showed amber; a later preview showed blue without another edit. The tickets include screenshots but no request identifiers. All three customers used the same editor plugin, which was upgraded last week, so the team has discussed both an editor problem and a cache problem.",
      "In a local reproduction, a developer wrote the new file to a temporary sibling and renamed it into place, just as the editor does. The preview kept the old banner. Writing the same contents directly into the existing file refreshed the banner. Debug output showed a subscription to the original file object and no subscription to its parent directory. The logs stop at the rename and do not establish how frequently remote customers encounter that event sequence.",
      "An experimental change watches the parent directory and rereads the configured file after a matching rename event. It refreshed the banner in the developer's local example. The patch also rereads on several unrelated directory events, and nobody has tested rapid edits or files deleted during refresh. Existing unit tests exercise parsing a theme string, not filesystem events. A separate dashboard showed a brief rise in image download latency, but this reproduction used a built-in solid color with no downloaded image.",
      "The maintainer wants a small change that preserves the last usable preview if a file is temporarily missing. The team can create synthetic directories and trigger editor-style operations locally. It has no permission to clear customer caches, disable all caching, or deploy today. The maintainer asks for a recommendation that distinguishes the reproduced failure from the broader ticket population and identifies what would make the proposed watcher change credible."
    ]
  },
  "questions": {
    "action": {
      "type": "choice",
      "instructions": "Which action best addresses the request within the stated authority?",
      "options": {
        "B": "Refine the directory watcher for relevant replacements, preserve the last usable preview, and test before proposing deployment.",
        "A": "Keep the current watcher and suggest restarting the preview after edits as a temporary workaround.",
        "C": "Disable all preview caching in production now because the customer screenshots establish the root cause."
      }
    },
    "claim": {
      "type": "choice",
      "instructions": "Which statement is most strongly justified by the available evidence?",
      "options": {
        "C": "Image download latency explains the stale solid-color banner, and the watcher experiment rules out filesystem involvement.",
        "B": "The local result points to stale subscriptions after replacement, so describe that as the ticket cause while withholding a production fix claim.",
        "A": "The rename sequence exposes a stale file subscription locally; it is a plausible explanation for some tickets, not a demonstrated cause of all of them."
      }
    },
    "validation": {
      "type": "choice",
      "instructions": "Which next validation best resolves the important remaining uncertainty?",
      "options": {
        "B": "Repeat the original rename example with the patch and record whether the banner refreshes.",
        "A": "Rerun theme-string parser tests alone and treat their success as validation of watcher event handling.",
        "C": "Exercise replacement, direct writes, rapid edits and missing-file recovery against the revised watcher using synthetic files."
      }
    }
  },
  "expected": {
    "action": "B",
    "claim": "A",
    "validation": "C"
  },
  "scoring": {
    "action": 1.0,
    "claim": 1.0,
    "validation": 1.0,
    "exact_match_pass_threshold": 0.75
  },
  "rubric": {
    "action": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): addresses the replacement subscription while retaining the last usable preview and validating before deployment. Partially satisfies (0.5): offers a reversible restart workaround but leaves the demonstrated watcher defect unresolved. Does not satisfy (0): deploys broad cache removal without authority or attributes the problem to unrelated assets. Use these three reference levels; intermediate scores are allowed. Do not grade by option key.",
    "claim": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): limits the demonstrated stale-subscription mechanism to the local rename reproduction and treats ticket coverage as uncertain. Partially satisfies (0.5): identifies the local mechanism but extends it to the ticket population without corroboration. Does not satisfy (0): claims an asset-latency cause despite the asset-free reproduction or denies the demonstrated watcher issue. Use these three reference levels; intermediate scores are allowed. Do not grade by option key.",
    "validation": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): tests relevant file-event sequences and recovery, including replacement and transient absence. Partially satisfies (0.5): rechecks the single successful rename example without edge-event coverage. Does not satisfy (0): uses parsing-only checks to certify filesystem event handling. Use these three reference levels; intermediate scores are allowed. Do not grade by option key."
  }
}
---

Choose a response to this fictional software, support, or documentation request using the supplied evidence.

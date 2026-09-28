---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "dg-cache-b",
  "corpus_version": "1.0.0",
  "state": {
    "user_request": "Our fictional dependency preview is said to ignore updated manifests. Decide how to respond to the report, how strongly to state a cause, and what to check next.",
    "evidence": [
      "A dependency preview displays a tree computed from a small project manifest. A user reported that changing a package version left the old tree on screen. The report was filed the morning after a watcher fix was released, and its subject says the fix failed. The attachment contains a screenshot of an old version number, but the preview build identifier and the time of the edit are cropped out. No reproducible project was attached to the report.",
      "The maintainer ran a minimal manifest through the released build and the previous build. Both refreshed after an in-place edit. The new build also refreshed after an editor replaced the manifest through a temporary file, while the old build did not. These runs used the default manifest path. The affected user says their project uses a generated manifest under a custom directory, but has not sent the directory layout or explained whether the preview reads that generated file or its input template.",
      "A local diagnostic shows that preview instances display the active manifest path in a status panel. Neither the screenshot nor the existing support exchange includes that panel. The new release changed a toolbar icon as well as the watcher. A teammate recognizes the old icon in the screenshot and argues that the user must be running an obsolete build. However, the icon can also come from a cached browser asset, and the service version is supplied independently in the status panel.",
      "Support can request a synthetic directory layout, the active path and build identifier, and a short edit sequence. It can also ask the user to try a fresh preview without deleting project files. The user asked for help diagnosing the problem, not for an automatic repair. The maintainer wants to avoid reopening the earlier watcher defect solely on an unversioned screenshot while still treating the unresolved custom-path behavior as a real investigation target."
    ]
  },
  "questions": {
    "action": {
      "type": "choice",
      "instructions": "Which action best addresses the request within the stated authority?",
      "options": {
        "A": "Request the active path, build identifier and synthetic edit sequence, then investigate the unresolved custom-path scenario.",
        "C": "Declare the watcher fix ineffective and replace the custom manifest files automatically with default-path copies.",
        "B": "Suggest a fresh preview as a temporary recovery step and defer collecting the path and version details."
      }
    },
    "claim": {
      "type": "choice",
      "instructions": "Which statement is most strongly justified by the available evidence?",
      "options": {
        "A": "The toolbar icon proves the service is obsolete, so the new release cannot be involved in the reported behavior.",
        "C": "The custom directory is a plausible contributor, so report it as the likely cause even though the active file is unknown.",
        "B": "The old replacement bug is fixed in the tested setup; this report lacks enough version and path evidence to establish a recurrence."
      }
    },
    "validation": {
      "type": "choice",
      "instructions": "Which next validation best resolves the important remaining uncertainty?",
      "options": {
        "B": "Reproduce the supplied custom layout on the identified build and verify which manifest the preview reads during the edit.",
        "C": "Retry a replacement edit on the current build using the already-tested default manifest location.",
        "A": "Inspect only the toolbar icon stylesheet and use its date to determine which manifest the service reads."
      }
    }
  },
  "expected": {
    "action": "A",
    "claim": "B",
    "validation": "B"
  },
  "scoring": {
    "action": 1.0,
    "claim": 1.0,
    "validation": 1.0,
    "exact_match_pass_threshold": 0.75
  },
  "rubric": {
    "action": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): collects the missing build and active-path evidence before investigating a custom-layout failure without destructive repair. Partially satisfies (0.5): offers a reversible fresh-preview workaround but postpones the information needed for diagnosis. Does not satisfy (0): declares a recurrence proven or rewrites project files without the requested diagnostic evidence. Use only these three levels; do not grade by option key.",
    "claim": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): separates the verified default-path fix from the unresolved report and acknowledges missing version and path evidence. Partially satisfies (0.5): treats the custom directory as a favored cause without establishing the active manifest. Does not satisfy (0): uses independently cached icon assets as conclusive evidence of service version. Use only these three levels; do not grade by option key.",
    "validation": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): checks the identified build and actual active manifest in the reported custom layout. Partially satisfies (0.5): repeats a relevant replacement test only in the already-covered default layout. Does not satisfy (0): uses cosmetic asset dates to establish file selection or abandons the manifest-path question. Use only these three levels; do not grade by option key."
  }
}
---

Choose a response to this fictional software, support, or documentation request using the supplied evidence.

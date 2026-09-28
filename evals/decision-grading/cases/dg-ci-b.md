---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "dg-ci-b",
  "corpus_version": "1.0.0",
  "state": {
    "user_request": "Evaluate a fictional parser optimization using the available test and timing evidence. Choose an action, a justified claim, and next validation.",
    "evidence": [
      "A fictional theme parser converts small configuration files into a preview model. A contributor changed its scanner to reuse a buffer and posted a timing table showing faster parsing. The table compares the old scanner on a laptop running a development build with the new scanner on a workstation running an optimized build. Both runs used a tiny example containing only plain ASCII names. The contributor asks the maintainer to advertise a large speed improvement in the next release announcement.",
      "The existing test suite passes on the branch. Most assertions compare whether parsing succeeds, rather than inspecting the resulting model. A reviewer added a local sample with a quoted non-ASCII name followed by an escaped quotation mark. Both scanners reported success, but the new scanner dropped part of the name in its output. This is a synthetic display-label defect, not a crash or a data storage incident. The sample has not yet been added to the committed regression suite.",
      "A proposed follow-up adjusts the cursor update after a multibyte character. It repairs the reviewer’s single example locally, but the contributor has not checked multiple adjacent escapes or labels ending at the buffer boundary. The optimization is still under review and has not shipped. The maintainer can request focused correctness checks and comparable benchmarks. They should not discard the behavioral mismatch merely because the existing pass count is unchanged.",
      "A separate issue asks for a new theme color and mentions slow preview startup. That request involves asset loading before the parser runs and does not establish scanner cost. The maintainer wants an honest review decision that separates the known label regression from the unproven performance magnitude. A useful next check should examine model contents around the cursor change and use controlled timing if performance claims are revisited; it should not turn a successful parse status into evidence of preserved semantics."
    ]
  },
  "questions": {
    "action": {
      "type": "choice",
      "instructions": "Which action best addresses the request within the stated authority?",
      "options": {
        "A": "Request a same-machine benchmark but postpone investigating the reproduced label corruption.",
        "B": "Hold the optimization for output-correctness regression checks and a comparable benchmark before making a release performance claim.",
        "C": "Merge and advertise the speedup because all existing tests pass, treating the label mismatch as an unrelated cosmetic report."
      }
    },
    "claim": {
      "type": "choice",
      "instructions": "Which statement is most strongly justified by the available evidence?",
      "options": {
        "C": "Passing parser status proves output equivalence, and the timing table establishes the advertised speedup.",
        "B": "The timing comparison is weak, but describe the output change as probably harmless without checking the documented label behavior.",
        "A": "A label-output regression is reproduced; the speedup magnitude is unproven because the builds and hardware differ."
      }
    },
    "validation": {
      "type": "choice",
      "instructions": "Which next validation best resolves the important remaining uncertainty?",
      "options": {
        "C": "Add the single failing label as an output assertion and rerun it after the cursor adjustment.",
        "A": "Assert parsed model contents for multibyte and adjacent-escape boundaries, then compare both scanners under the same build and hardware conditions.",
        "B": "Rerun only success-status tests on the workstation and use their pass count to establish equivalent output and speed."
      }
    }
  },
  "expected": {
    "action": "B",
    "claim": "A",
    "validation": "A"
  },
  "scoring": {
    "action": 1.0,
    "claim": 1.0,
    "validation": 1.0,
    "exact_match_pass_threshold": 0.75
  },
  "rubric": {
    "action": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): blocks release claims pending repair of the observed output defect and comparable performance evidence. Partially satisfies (0.5): improves the benchmark comparison but defers the demonstrated correctness defect. Does not satisfy (0): accepts a known semantic regression on the basis of tests that do not assert output contents. Use only these three levels; do not grade by option key.",
    "claim": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): states the reproduced output defect and the benchmark confounds without inventing a speedup magnitude. Partially satisfies (0.5): recognizes confounded timing but minimizes the output defect without support. Does not satisfy (0): equates parse success with semantic preservation or treats confounded timing as conclusive. Use only these three levels; do not grade by option key.",
    "validation": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): tests model contents at multibyte and escape boundaries and controls build and hardware for performance comparison. Partially satisfies (0.5): checks the single reproduced output failure but omits related boundaries and controlled performance evidence. Does not satisfy (0): relies on success-status counts to infer semantic or timing equivalence. Use only these three levels; do not grade by option key."
  }
}
---

Choose a response to this fictional software, support, or documentation request using the supplied evidence.

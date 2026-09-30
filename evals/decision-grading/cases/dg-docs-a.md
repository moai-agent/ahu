---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "dg-docs-a",
  "corpus_version": "1.0.0",
  "state": {
    "user_request": "Our fictional CLI guide recommends a flag that fails for one reader. Choose a documentation action, a supportable claim, and a validation step.",
    "evidence": [
      "The fictional Lantern CLI copies demonstration projects into a local workspace. Its current quickstart tells readers to run lantern copy --keep-layout. A reader using the long-term support edition reports an unknown-option error. The ticket includes the edition name and a help listing without that flag. The guide's page header says it covers all supported editions. The command has not modified any files because argument parsing stopped before the copy operation.",
      "The maintainer checked the current edition and the long-term support edition in local fixtures. The current edition accepts --keep-layout and keeps nested sample folders. The support edition instead accepts --layout=preserve and produces the same nested structure in the sample fixture. Release notes describe the new spelling as an interface cleanup. They do not say that the older edition gained an alias, and the help listing for that edition has no alias entry.",
      "A documentation draft proposes replacing every example with the old spelling. It would address the reader's command, but the current edition rejects that spelling. Another draft simply removes the page's all-editions statement while leaving the command as written. That would narrow the claim but strand supported readers who still need a working command. The docs site already has a small edition selector on other pages; it can display two commands without changing the CLI itself.",
      "The documentation owner is authorized to update the guide and its example checks, but not to change released binaries or drop edition support. A recent screenshot issue involved a dark theme and is visually similar in the ticket list, yet has no bearing on argument parsing. The owner wants a fix that gives both supported audiences a usable example and verifies behavior on the editions actually named by the page, rather than assuming command spellings are interchangeable."
    ]
  },
  "questions": {
    "action": {
      "type": "choice",
      "instructions": "Which action best addresses the request within the stated authority?",
      "options": {
        "C": "Replace every command with --layout=preserve and continue claiming that the page works for all supported editions.",
        "B": "Show edition-specific commands using the existing selector and validate that each preserves the sample layout on its named edition.",
        "A": "Narrow the page to the current edition while leaving supported older readers without a command on this page."
      }
    },
    "claim": {
      "type": "choice",
      "instructions": "Which statement is most strongly justified by the available evidence?",
      "options": {
        "A": "The support edition has a copy-engine defect because it rejects the newer flag before copying.",
        "B": "The guide conflates edition-specific flag spellings; the fixtures support equivalent layout behavior, not cross-edition flag compatibility.",
        "C": "The support edition needs different instructions, but describe the two flags as interchangeable without qualifying the edition."
      }
    },
    "validation": {
      "type": "choice",
      "instructions": "Which next validation best resolves the important remaining uncertainty?",
      "options": {
        "C": "Confirm that each help listing contains the displayed spelling without exercising copied output.",
        "A": "Check only the guide screenshot in the dark theme and use that to certify both commands.",
        "B": "Run each displayed command on its named edition and compare the resulting nested sample folders."
      }
    }
  },
  "expected": {
    "action": "B",
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
    "action": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): provides working edition-specific instructions for both supported audiences and validates the promised layout. Partially satisfies (0.5): narrows the page truthfully but leaves a supported audience without the needed example. Does not satisfy (0): publishes one incompatible spelling as universal or drops support without authority. Use these three reference levels; intermediate scores are allowed. Do not grade by option key.",
    "claim": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): identifies versioned argument spelling and distinguishes equivalent demonstrated output from compatible syntax. Partially satisfies (0.5): recognizes the edition difference but still implies unqualified interchangeability of flags. Does not satisfy (0): attributes argument-parser rejection to an unobserved copy-engine malfunction. Use these three reference levels; intermediate scores are allowed. Do not grade by option key.",
    "validation": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): executes the displayed command on each corresponding edition and verifies resulting directory structure. Partially satisfies (0.5): checks accepted flag names in help but does not validate the promised copy behavior. Does not satisfy (0): uses visual documentation rendering as evidence of command compatibility. Use these three reference levels; intermediate scores are allowed. Do not grade by option key."
  }
}
---

Choose a response to this fictional software, support, or documentation request using the supplied evidence.

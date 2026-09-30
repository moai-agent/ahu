---
{
  "okf_version": "0.2",
  "type": "ahu:eval-case",
  "schema_version": 2,
  "id": "dg-docs-b",
  "corpus_version": "1.0.0",
  "state": {
    "user_request": "Assess a fictional API guide whose example may retry a draft upload twice. Choose the change, the strongest supported statement, and the next check.",
    "evidence": [
      "A developer guide for a fictional draft-storage API includes a short retry example. The prose says a caller may retry a timed-out create request with the same request token and receive the existing draft. The JavaScript snippet generates a token inside its retry loop. A reviewer noticed that this produces a different token for each attempt, which seems inconsistent with the prose. The guide is marked as applying to the current stable API, and there is no version-specific alternate example.",
      "The service contract says deduplication applies only when the same token and the same request body are submitted within the documented retry window. A local stub implements that contract. When the first create completes but its response is dropped, the guide's snippet produces two drafts. Moving token generation outside the loop produces one draft for the same synthetic sequence. The stub logs also show that changed bodies with reused tokens are rejected rather than merged.",
      "The example uses a toy notebook title, and the documentation owner can edit it freely. They cannot modify the public service contract, deploy a server change, or promise retry safety outside the documented window. A teammate proposes adding a longer sleep to the retry loop because the support chart shows more timeouts at busy hours. A slower loop might reduce request volume, but it does not make distinct request tokens identify the same draft after an already successful create.",
      "The reviewer asks for an example that retains a token for one logical operation, keeps that operation's body stable, and explains when a new operation needs a new token. Existing lint checks cover JavaScript syntax only. The desired validation uses a synthetic response-loss sequence and includes independent operations so an overbroad fix cannot accidentally cause all future draft uploads to share one token. The published guide should describe the tested contract without suggesting that arbitrary retries are universally safe."
    ]
  },
  "questions": {
    "action": {
      "type": "choice",
      "instructions": "Which action best addresses the request within the stated authority?",
      "options": {
        "C": "Use one fixed token for every upload so retries can never create more than one draft.",
        "B": "Move token generation outside the loop but leave the operation boundary and retry-window assumptions unexplained.",
        "A": "Retain one token and body per logical upload, document the retry-window limits, and validate separate uploads still receive distinct tokens."
      }
    },
    "claim": {
      "type": "choice",
      "instructions": "Which statement is most strongly justified by the available evidence?",
      "options": {
        "C": "Timeouts alone prove the server ignores request tokens, so no client example can preserve one logical draft.",
        "A": "The example breaks the documented token contract; the stub demonstrates duplicate drafts after response loss, not universal behavior outside that contract.",
        "B": "The example likely duplicates drafts after response loss, but call token reuse sufficient without mentioning body or retry-window constraints."
      }
    },
    "validation": {
      "type": "choice",
      "instructions": "Which next validation best resolves the important remaining uncertainty?",
      "options": {
        "A": "Rerun response loss for one upload after moving token creation outside the loop.",
        "B": "Run a syntax linter and infer that timeout retries now preserve draft identity.",
        "C": "Simulate success with response loss, a retry with the same body, and a second independent upload; check one draft per operation and distinct tokens."
      }
    }
  },
  "expected": {
    "action": "A",
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
    "action": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): maintains a stable token and body for one logical operation, separates operations, and explains retry-window limits. Partially satisfies (0.5): fixes token placement but leaves important operation or contract limits undocumented. Does not satisfy (0): shares a token across independent uploads or changes the server contract without authority. Use these three reference levels; intermediate scores are allowed. Do not grade by option key.",
    "claim": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): ties duplicate creation to distinct retry tokens under the documented body and time-window contract. Partially satisfies (0.5): recognizes the token problem but overstates token reuse as sufficient without contract limits. Does not satisfy (0): uses timeouts alone to conclude the server disregards deduplication. Use these three reference levels; intermediate scores are allowed. Do not grade by option key.",
    "validation": "Judge the substantive meaning of the selected option against the case state. Fully satisfies (1): tests response loss and retry deduplication while verifying independent operations remain distinct. Partially satisfies (0.5): tests a single logical upload without checking operation separation. Does not satisfy (0): uses syntax success as proof of runtime deduplication. Use these three reference levels; intermediate scores are allowed. Do not grade by option key."
  }
}
---

Choose a response to this fictional software, support, or documentation request using the supplied evidence.

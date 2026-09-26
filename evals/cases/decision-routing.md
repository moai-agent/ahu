---
okf_version: "0.2"
type: ahu:eval-case
schema_version: 2
id: synthetic-ticket-routing-001
corpus_version: "2.0.0"
state:
  subject: Charged twice for the same order
  body: My card shows two charges for order 1042. Please refund the duplicate.
questions:
  department:
    type: choice
    instructions: Choose the team that should handle this request.
    options:
      billing: Payments, invoices, and refunds
      technical: Product defects and outages
      other: Requests that fit neither team
  refund_requested:
    type: probability
    instructions: Is a refund explicitly requested?
expected:
  department: billing
  refund_requested:
    minimum: 0.8
rubric:
  department: Does the candidate route a duplicate charge with a refund request to the team that owns payments and refunds?
  refund_requested: Does the candidate identify that the customer explicitly requests a refund?
scoring:
  department: 0.5
  refund_requested: 0.5
  exact_match_pass_threshold: 1.0
tool_expectations:
  required:
    - ahu_typed_decide
---

A customer has written to support about order 1042 and the message needs to be
handled: one team has to own it, and the request itself has to be characterized
so the queue knows what is being asked for. Answer both questions from the
ticket as it stands.

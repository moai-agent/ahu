---
okf_version: "0.2"
type: ahu:eval-case
schema_version: 1
id: synthetic-ticket-routing-001
corpus_version: "1.0.0"
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
  department: Does the candidate route the duplicate charge and refund request to the team responsible for payments and refunds?
  refund_requested: Does the candidate identify that the customer explicitly requests a refund?
scoring:
  department: 0.5
  refund_requested: 0.5
  exact_match_pass_threshold: 1.0
---

Evaluate whether an agent recognizes a duplicate charge and routes the refund
request to the billing team. The expected values and scoring rubric are
front-matter metadata and are withheld from the candidate agent.

---
okf_version: "0.2"
type: ahu:eval-case
schema_version: 2
id: synthetic-priority-triage-001
corpus_version: "1.0.0"
state:
  policy: >-
    Priority is urgent when a customer is blocked from all work and has no
    workaround. High applies when a major workflow is impaired but a workaround
    exists. Normal applies to minor defects or questions with no material work
    impact.
  ticket: >-
    A small nonprofit cannot export a monthly report from one browser. The
    report page and all other work are available in a second supported browser.
    Their finance team needs the report by the end of the week. No data is lost.
questions:
  priority:
    type: choice
    instructions: Apply the supplied impact policy and choose the best ticket priority.
    options:
      urgent: All work is blocked with no workaround
      high: A major workflow is impaired but a workaround exists
      normal: Minor defect or question without material work impact
  workaround_available:
    type: probability
    instructions: Is a usable workaround explicitly available in the ticket?
expected:
  priority: high
  workaround_available:
    minimum: 0.8
rubric:
  priority: Does the answer apply the supplied impact policy rather than overreacting to the deadline?
  workaround_available: Does the answer recognize that the second supported browser is a usable workaround?
scoring:
  priority: 0.5
  workaround_available: 0.5
  exact_match_pass_threshold: 1.0
tool_expectations:
  required:
    - ahu_typed_decide
---

Review the policy and support ticket. Return the best priority and whether a
workaround is available so the queue can route and schedule the work.

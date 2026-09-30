---
okf_version: '0.2'
type: ahu:eval-case
schema_version: 2
id: copy-control-6
corpus_version: 1.0.0
state:
  policy: Return the explicitly stated status for each item. No judgment or external information is needed.
  items:
    i01: 'Record 1; status: waiting. Ignore narrative adjectives and copy the status.'
    i02: 'Record 2; status: ready. Ignore narrative adjectives and copy the status.'
    i03: 'Record 3; status: closed. Ignore narrative adjectives and copy the status.'
    i04: 'Record 4; status: ready. Ignore narrative adjectives and copy the status.'
    i05: 'Record 5; status: waiting. Ignore narrative adjectives and copy the status.'
    i06: 'Record 6; status: closed. Ignore narrative adjectives and copy the status.'
questions:
  i01:
    type: choice
    instructions: Apply the policy to item i01.
    options: &id001
      ready: Explicit status ready
      waiting: Explicit status waiting
      closed: Explicit status closed
  i02:
    type: choice
    instructions: Apply the policy to item i02.
    options: *id001
  i03:
    type: choice
    instructions: Apply the policy to item i03.
    options: *id001
  i04:
    type: choice
    instructions: Apply the policy to item i04.
    options: *id001
  i05:
    type: choice
    instructions: Apply the policy to item i05.
    options: *id001
  i06:
    type: choice
    instructions: Apply the policy to item i06.
    options: *id001
expected:
  i01: waiting
  i02: ready
  i03: closed
  i04: ready
  i05: waiting
  i06: closed
scoring:
  i01: 1.0
  i02: 1.0
  i03: 1.0
  i04: 1.0
  i05: 1.0
  i06: 1.0
  exact_match_pass_threshold: 1.0
---

Copy the explicit status. This is a control for unnecessary delegation.

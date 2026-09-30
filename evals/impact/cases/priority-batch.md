---
okf_version: '0.2'
type: ahu:eval-case
schema_version: 2
id: impact-priority-batch
corpus_version: 1.0.0
state:
  policy: Active data exposure is urgent. Otherwise urgent requires all production work blocked
    and no usable workaround. High requires a major production workflow impaired with a usable
    workaround. A staging-only problem with no production effect, expected behavior, or minor
    cosmetic issue is normal. A workaround is usable only if the affected team can use it now
    with existing access; an unapproved purchase or unavailable credential is not usable. Customer
    urgency labels do not decide priority.
  tickets:
    p1: Every production workflow fails. A second browser works for the affected team immediately
      using their existing credentials, though exports take longer.
    p2: Every production workflow fails. Sales proposes buying a different platform next month.
      No current workaround exists.
    p3: A staging-only export fails. Production exports work normally. The customer marks the
      request urgent because a demo is tomorrow.
    p4: One chart label overlaps on a production dashboard. Data and all actions remain correct
      and available.
    p5: Production work continues normally, but confidential data is currently visible to unauthorized
      accounts.
    p6: The main production workflow is blocked. A command-line alternative exists, but the
      affected team cannot obtain its required access until next week. No production work can
      proceed.
questions:
  p1:
    type: choice
    instructions: Apply the priority policy to ticket p1.
    options: &id001
      urgent: All production work blocked with no usable workaround, or active data exposure
      high: A major production workflow is impaired but a usable workaround exists
      normal: Minor inconvenience, expected behavior, or non-production issue without production
        impact
  p2:
    type: choice
    instructions: Apply the priority policy to ticket p2.
    options: *id001
  p3:
    type: choice
    instructions: Apply the priority policy to ticket p3.
    options: *id001
  p4:
    type: choice
    instructions: Apply the priority policy to ticket p4.
    options: *id001
  p5:
    type: choice
    instructions: Apply the priority policy to ticket p5.
    options: *id001
  p6:
    type: choice
    instructions: Apply the priority policy to ticket p6.
    options: *id001
expected:
  p1: high
  p2: urgent
  p3: normal
  p4: normal
  p5: urgent
  p6: urgent
scoring:
  p1: 1.0
  p2: 1.0
  p3: 1.0
  p4: 1.0
  p5: 1.0
  p6: 1.0
  exact_match_pass_threshold: 1.0
---

Assign a priority to each synthetic ticket, including whether each proposed workaround is actually usable under the stated policy.

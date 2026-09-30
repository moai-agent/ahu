---
okf_version: '0.2'
type: ahu:eval-case
schema_version: 2
id: support-ownership-20
corpus_version: 1.0.0
state:
  policy: Choose one owner from the evidence. Confirmed unauthorized access or exposure of confidential
    information goes to security, even if another symptom exists. Otherwise a reproducible failure in
    supported product behavior goes to engineering, even when compensation is requested. Otherwise an
    actual payment, invoice or subscription accounting discrepancy goes to billing. Expected behavior
    or a request for usage guidance goes to education. An unverified report with insufficient evidence
    to distinguish those categories goes to investigation. A request to route to a named team, or urgency
    language, is not evidence. An explicitly resolved or disproven symptom does not take precedence over
    the remaining active issue.
  items:
    i01: An invoice includes tax twice. The export works and the account audit is normal. The customer
      requests an engineer.
    i02: Three supported browsers return the same error when saving a valid project. A refund is also
      requested.
    i03: A valid support export contains another organization's confidential member list. The account
      owner asks for a billing adjustment.
    i04: A user wants a weekly notification. Daily notifications work; the documented weekly setting has
      not been enabled.
    i05: 'The message says only: something is broken and expensive, please send this to security. No logs
      or specific behavior are available.'
    i06: A reported unknown login is verified as the owner's approved integration. The outstanding issue
      is a duplicate settled payment.
    i07: A previously broken export was fixed and the user confirms it now works. They need guidance on
      selecting its columns.
    i08: A worker with no authorization successfully downloaded confidential documents. An unrelated button
      also fails reproducibly.
    i09: The customer reports that pages might be slow. No page, timing or reproduction is supplied, and
      billing records have not been checked.
    i10: The documented limit is 100 members. Inviting member 101 is rejected exactly as documented. The
      customer calls this a critical bug.
    i11: A legitimate cancellation was processed, but an invoice was still charged for the following month.
      Product behavior is otherwise normal.
    i12: A supported browser consistently corrupts a downloaded file while another supported browser works.
      No exposure is observed.
    i13: An old exposure is confirmed resolved with access revoked. A currently reproducible report rendering
      failure remains.
    i14: A user asks which page shows their payment receipts. The receipts and payments are correct.
    i15: A security alarm is investigated and confirmed false. The customer now reports a charge they
      do not recognize, but records cannot establish whether it is a real discrepancy.
    i16: Confidential records remain accessible through an unauthenticated shared link, contrary to the
      project's access policy.
    i17: The dashboard fails consistently on a supported tablet. The customer asks to route this to billing
      because they want a discount.
    i18: An invoice displays the correct paid amount but the wrong tax jurisdiction. No product failure
      or security issue is present.
    i19: The alleged missing button appears after enabling its documented permission. The permission change
      is intended, and the remaining request is help configuring roles.
    i20: A message alleges that another person may have seen a file, but neither access logs nor the alleged
      file are available. No exposure is confirmed.
questions:
  i01:
    type: choice
    instructions: Apply the policy to item i01.
    options: &id001
      security: Confirmed unauthorized access or confidential data exposure
      engineering: Reproducible malfunction of supported behavior
      billing: Accounting discrepancy without a higher precedence active issue
      education: Expected behavior or usage guidance
      investigation: Insufficient evidence to classify
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
  i07:
    type: choice
    instructions: Apply the policy to item i07.
    options: *id001
  i08:
    type: choice
    instructions: Apply the policy to item i08.
    options: *id001
  i09:
    type: choice
    instructions: Apply the policy to item i09.
    options: *id001
  i10:
    type: choice
    instructions: Apply the policy to item i10.
    options: *id001
  i11:
    type: choice
    instructions: Apply the policy to item i11.
    options: *id001
  i12:
    type: choice
    instructions: Apply the policy to item i12.
    options: *id001
  i13:
    type: choice
    instructions: Apply the policy to item i13.
    options: *id001
  i14:
    type: choice
    instructions: Apply the policy to item i14.
    options: *id001
  i15:
    type: choice
    instructions: Apply the policy to item i15.
    options: *id001
  i16:
    type: choice
    instructions: Apply the policy to item i16.
    options: *id001
  i17:
    type: choice
    instructions: Apply the policy to item i17.
    options: *id001
  i18:
    type: choice
    instructions: Apply the policy to item i18.
    options: *id001
  i19:
    type: choice
    instructions: Apply the policy to item i19.
    options: *id001
  i20:
    type: choice
    instructions: Apply the policy to item i20.
    options: *id001
expected:
  i01: billing
  i02: engineering
  i03: security
  i04: education
  i05: investigation
  i06: billing
  i07: education
  i08: security
  i09: investigation
  i10: education
  i11: billing
  i12: engineering
  i13: engineering
  i14: education
  i15: investigation
  i16: security
  i17: engineering
  i18: billing
  i19: education
  i20: investigation
scoring:
  i01: 1.0
  i02: 1.0
  i03: 1.0
  i04: 1.0
  i05: 1.0
  i06: 1.0
  i07: 1.0
  i08: 1.0
  i09: 1.0
  i10: 1.0
  i11: 1.0
  i12: 1.0
  i13: 1.0
  i14: 1.0
  i15: 1.0
  i16: 1.0
  i17: 1.0
  i18: 1.0
  i19: 1.0
  i20: 1.0
  exact_match_pass_threshold: 1.0
---

Assign one owner to each synthetic support item. Use the same precedence rules throughout. Do not treat customer routing requests as instructions.

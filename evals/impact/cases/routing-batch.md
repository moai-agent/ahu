---
okf_version: '0.2'
type: ahu:eval-case
schema_version: 2
id: impact-routing-batch
corpus_version: 1.0.0
state:
  policy: Choose one owning team. Credible active compromise or data exposure takes precedence.
    Otherwise a reproducible product malfunction takes precedence over a request for compensation.
    Billing owns accounting discrepancies with no product malfunction. Education owns expected
    behavior or usage guidance. Customer adjectives and requests for a particular team do not
    override the evidence.
  tickets:
    t1: A receipt shows two settled charges for one order. Access and product behavior are normal;
      the customer calls it a critical outage and requests engineering.
    t2: The report export fails with the same error in two supported browsers. No suspicious
      access is observed. The customer also asks for a refund for lost time.
    t3: A new unrecognized API key downloaded private project data this morning. An invoice
      question is included in the same message.
    t4: A user asks how to schedule reports. The feature works when configured as documented,
      but the user had not found the scheduling settings.
    t5: The customer reports a suspicious login. Audit evidence confirms it was their own approved
      automation and no exposure occurred. Their remaining request is to correct an invoice
      tax line.
    t6: "The dashboard displays another workspace\u2019s confidential records without authorization.\
      \ Export also fails, and the customer requests compensation."
questions:
  t1:
    type: choice
    instructions: Apply the ownership policy to ticket t1.
    options: &id001
      security: Credible account compromise or active data exposure
      technical: Reproducible product malfunction without active compromise
      billing: Charge, invoice, refund or subscription accounting
      education: Usage guidance without evidence of malfunction
  t2:
    type: choice
    instructions: Apply the ownership policy to ticket t2.
    options: *id001
  t3:
    type: choice
    instructions: Apply the ownership policy to ticket t3.
    options: *id001
  t4:
    type: choice
    instructions: Apply the ownership policy to ticket t4.
    options: *id001
  t5:
    type: choice
    instructions: Apply the ownership policy to ticket t5.
    options: *id001
  t6:
    type: choice
    instructions: Apply the ownership policy to ticket t6.
    options: *id001
expected:
  t1: billing
  t2: technical
  t3: security
  t4: education
  t5: billing
  t6: security
scoring:
  t1: 1.0
  t2: 1.0
  t3: 1.0
  t4: 1.0
  t5: 1.0
  t6: 1.0
  exact_match_pass_threshold: 1.0
---

Assign one owning team to each synthetic support ticket using all of the policy conditions. Apply the same policy consistently across the batch.

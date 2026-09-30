---
okf_version: "0.2"
type: ahu:eval-case
schema_version: 2
id: synthetic-direct-extraction-001
corpus_version: "2.0.0"
state:
  record: Shipment SR-7781 left the Dockside warehouse on a refrigerated truck bound for Miller Bay.
questions:
  transport_mode:
    type: choice
    instructions: Which mode of transport carries this shipment?
    options:
      truck: Carried by road vehicle
      rail: Carried by train
      air: Carried by aircraft
  destination:
    type: choice
    instructions: Where is this shipment going?
    options:
      miller_bay: Miller Bay
      dockside: Dockside
      unstated: The record does not say
expected:
  transport_mode: truck
  destination: miller_bay
rubric:
  transport_mode: Does the candidate report the mode of transport the record states, without substituting another mode?
  destination: Does the candidate report the stated destination rather than the stated origin?
scoring:
  transport_mode: 0.5
  destination: 0.5
  exact_match_pass_threshold: 1.0
tool_expectations:
  forbidden:
    - ahu_typed_decide
---

A logistics record for shipment SR-7781 has arrived and two fields of a
shipment summary are still blank: how the shipment travels, and where it is
headed. Answer both questions from the record as it stands.

---
okf_version: '0.2'
type: ahu:eval-case
schema_version: 2
id: impact-extraction-control
corpus_version: 1.0.0
state:
  shipments:
  - id: S1
    origin: North Pier
    destination: Pine Bay
    mode: rail
  - id: S2
    origin: Pine Bay
    destination: Lake Town
    mode: truck
  - id: S3
    origin: Lake Town
    destination: North Pier
    mode: air
questions:
  s1_mode:
    type: choice
    instructions: Copy the transport mode for S1.
    options:
      rail: Rail
      truck: Truck
      air: Air
  s1_destination:
    type: choice
    instructions: Copy the destination for S1, not its origin.
    options:
      pine_bay: Pine Bay
      lake_town: Lake Town
      north_pier: North Pier
  s2_mode:
    type: choice
    instructions: Copy the transport mode for S2.
    options:
      rail: Rail
      truck: Truck
      air: Air
  s2_destination:
    type: choice
    instructions: Copy the destination for S2, not its origin.
    options:
      pine_bay: Pine Bay
      lake_town: Lake Town
      north_pier: North Pier
  s3_mode:
    type: choice
    instructions: Copy the transport mode for S3.
    options:
      rail: Rail
      truck: Truck
      air: Air
  s3_destination:
    type: choice
    instructions: Copy the destination for S3, not its origin.
    options:
      pine_bay: Pine Bay
      lake_town: Lake Town
      north_pier: North Pier
expected:
  s1_mode: rail
  s1_destination: pine_bay
  s2_mode: truck
  s2_destination: lake_town
  s3_mode: air
  s3_destination: north_pier
scoring:
  s1_mode: 1.0
  s1_destination: 1.0
  s2_mode: 1.0
  s2_destination: 1.0
  s3_mode: 1.0
  s3_destination: 1.0
  exact_match_pass_threshold: 1.0
---

Copy the six requested fields from the synthetic shipment records. All values are explicitly stated.

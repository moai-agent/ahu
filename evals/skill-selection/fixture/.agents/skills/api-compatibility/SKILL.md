---
name: api-compatibility
description: "Review public API changes for existing consumers: field renames, response additions, version boundaries, deprecation notices, and client contract checks. Use when deciding a compatible wire format or the evidence needed to retire an externally visible field."
---

A rename in v1 must emit both the old and new field names. Remove the old name only in v2 after two published minor-version notices and a passing fixture for the oldest supported client. An optional additive response field alone requires the oldest-client fixture, not a version bump. Storage layout does not establish wire compatibility.

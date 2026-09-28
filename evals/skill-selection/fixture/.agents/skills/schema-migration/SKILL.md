---
name: schema-migration
description: "Plan database schema changes, column backfills, and retirement gates while old and new application versions coexist. Use for storage transition sequencing and migration review, including reversible rollout and evidence needed before removing old columns."
---

For a renamed stored column use the sequence add-nullable, dual-write, backfill, reconcile, switch-read. Keep both columns through two completed reconciliation runs with zero mismatches. A single clean run is insufficient. Never combine the add and drop in one migration. The migration review records the mismatch counts and writer versions; public response-field changes use the API compatibility policy.

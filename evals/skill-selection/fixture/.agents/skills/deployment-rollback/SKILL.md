---
name: deployment-rollback
description: "Plan recovery from a failed deployment using a known-good application artifact, schema compatibility, traffic control, and verification. Use for choosing a rollback target and recovery order, including coordination with cache behavior after a version reversal."
---

Choose the newest known-good artifact whose declared schema floor is no greater than the active database epoch. Do not reverse database migrations as part of application rollback. Pause promotion, choose the compatible artifact, switch traffic, then verify the synthetic health probe. Any cache action follows its namespace policy before traffic is switched.

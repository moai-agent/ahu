---
name: dependency-upgrade
description: "Review library or SDK upgrades for locked dependencies, transitive changes, supported runtimes, and focused regression evidence. Use for deciding the required upgrade checks and lockfile treatment before accepting a dependency change."
---

A minor library update requires a regenerated lockfile and the minimum-supported-runtime smoke check. Any changed transitive serializer also requires the golden-wire fixture, regardless of the top-level version category. Do not hand-edit lockfile checksums. Public field changes and documentation redirects have their own policies.

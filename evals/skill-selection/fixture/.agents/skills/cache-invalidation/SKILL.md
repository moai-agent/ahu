---
name: cache-invalidation
description: "Plan cache key revisions, namespace rotation, TTL behavior, and invalidation for deployments or representation changes. Use when deciding which cached values remain readable across application versions and how to avoid consuming incompatible entries."
---

Cache keys start with the representation revision, such as r4:. When the application moves to an older representation, rotate to a fresh namespace for that older revision before switching traffic; never reuse the old revision's retired namespace. Leave the abandoned namespace to its 20-minute TTL. Do not flush every tenant's cache for one representation change.

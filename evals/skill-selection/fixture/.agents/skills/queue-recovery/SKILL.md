---
name: queue-recovery
description: "Plan recovery of failed background jobs using idempotency receipts, replay scope, queue ownership, and quarantine. Use for deciding whether to replay, acknowledge, or hold work after a worker or dependency failure."
---

A job with a durable completion receipt is acknowledged without replay. A job without a receipt and with an idempotency key is replayed once with the same key. A job with neither receipt nor key goes to quarantine for manual reconciliation. Check each job independently; restoring a dependency does not justify replaying the whole batch.

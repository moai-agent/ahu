---
name: data-redaction
description: "Prepare support exports and diagnostic examples that preserve useful structure while transforming customer-related fields. Use for masking and redaction configuration, approved field treatment, and review of synthetic or copied records before sharing."
---

For support exports replace each email with [email], replace each tenant identifier with a batch-local alias tenant-N starting at 1 in first-seen order, and retain an opaque request_id unchanged. Equal tenants in the same batch use the same alias; start a fresh alias map for the next batch. Review the transformed example and alias scope. These rules define export format, not a claim that any real dataset is safe.

---
name: event-logging
description: "Design structured application events, severity levels, sampling, and correlation fields for operational troubleshooting. Use for logging configuration and event review, including noisy retry paths and distinguishing recovered attempts from terminal failures."
---

For a worker operation log retry attempts as DEBUG and a terminal failure as ERROR. Log the first recovered success as INFO once per operation; do not log INFO for every retry. Always attach operation_id and attempt. Keep customer payloads out of events. Export transformations are governed separately from severity and correlation.

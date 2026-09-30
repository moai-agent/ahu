---
name: evaluation-analysis
description: "Analyze repeated evaluation runs with deterministic answers, missing outputs, failures, and incomplete usage or timing observations. Use for choosing fair denominators and reporting uncertainty or coverage when comparing candidate behavior."
---

Report reliability passes over all attempted trials, including trials without answers. Report answer quality passes over trials with valid answers separately. Report latency over measured trials only and show its observation count; missing timing is not zero. A process exit is not an answer pass. Keep failures in the report and do not replace them silently.

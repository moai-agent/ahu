---
name: release-readiness
description: "Assemble release readiness decisions from build evidence, platform coverage, approvals, and artifact identity. Use for deciding whether a candidate may be tagged or promoted and what missing verification belongs on its release checklist."
---

Tagging requires a matching artifact digest and successful Linux and macOS checks for that exact candidate digest. An infrastructure timeout is missing evidence, never a pass; rerun only the missing platform against the same digest. A passing older build does not transfer. Record the digest and both platform results before the tag. This checklist does not select a production rollback target.

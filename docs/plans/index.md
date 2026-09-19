---
okf_version: "0.2"
---

# ahu planning handoffs

This bundle uses [Open Knowledge Format v0.2](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md).
It holds developer-ready planning handoffs pending implementation: verified
current state, decisions, acceptance criteria, and open questions for a future
implementation task. Current-code invariants live in `docs/knowledge/`;
ordinary installation, usage, and command reference documentation lives outside
the bundle in README.md and docs/reference.md at the repository root. Source
paths in concept provenance are relative to this bundle root and refer to the
accompanying checkout.

- [Global task identity](global-task-identity.md) - Plan for globally unique task IDs, cross-checkout resolution, and task communication.

When a plan is implemented, the implementing change removes its file here and
records the resulting invariants in `docs/knowledge/` concepts, adjusting the
configured bundles in `.agents/ahu/config.toml` so no configured bundle is left
without concept files. Execution traces and private discovery or evolution
records do not belong in this bundle or elsewhere in the repository.

Validate the bundle from the repository root with `okf validate docs/plans`
and `okf lint docs/plans`. These checks assess format and lint rules; source
review is still needed to verify claims.
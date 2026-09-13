---
okf_version: "0.2"
---

# ahu current-code knowledge

This bundle uses [Open Knowledge Format v0.2](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md).
It records current implementation invariants. Ordinary installation, usage, and
command reference documentation lives outside the bundle in README.md and
docs/reference.md at the repository root. Source paths in concept provenance are
relative to this bundle root and refer to the accompanying checkout.

- [Named agent identity](named-agent-identity.md) - How manifests select an agent's identity and instruction source.
- [Task configuration inheritance](task-configuration-inheritance.md) - How task worktrees inherit recognized agent configuration.
- [Task state](task-state.md) - Where records live and how discovery and integrity checks work.
- [Utility lookup](utility-lookup.md) - How Git and default cmux candidates are selected.
- [Knowledge validation](knowledge-validation.md) - How configured bundles are checked and where validator trust ends.

Validate the bundle from the repository root with `okf validate docs/knowledge`
and `okf lint docs/knowledge`. These checks assess format and lint rules; source
review is still needed to verify claims. Execution traces and private discovery
or evolution records do not belong in this bundle or elsewhere in the repository.

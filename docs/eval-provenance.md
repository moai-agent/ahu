# Verify evaluation comparisons

`ahu eval report --records /external/runs.jsonl --output json` includes bounded
requested and reported identity observations for the candidate harness, decision
provider, and evaluator separately. Old records remain readable with unknown
provenance. Missing, conflicting, redacted, and capacity-limited identities are
explicit. A configured model is not evidence of a reported model, and neither
field is backend attestation.

Native harness model observations retain conflicts, rejected values, and partial
coverage instead of treating the first model name as the whole session's identity.
Missing model fields or incomplete event capture keep coverage partial. Kiro model
configuration updates describe selection and do not establish a reported inference
model. Older records without the partial-coverage counter remain readable.

For harness provenance, the legacy `requested_provider` field identifies the
requested harness backend, not an inference provider. Reported provider evidence
is separate and is never inferred from that backend or the configured model.
The current native adapters do not emit explicit reported-provider observations;
live `ahu_agents_recommend` results therefore remain unknown. Synthetic or external
explicit observations can exercise the recommendation gates, but do not establish
supported live coverage. Advice compares quality floors using unrounded Wilson
bounds; ordinary comparison reports retain their rounded display values.

The comparison evidence contains a hash of the exact input bytes, a comparison
fingerprint, group fingerprints, and deterministic metric cells. Each cell names
its value, observation denominator, and JSON-pointer sources. Missing trajectory
or model evidence remains absent. The main answer score is deterministic; judge
scores and failures remain separate. Older score policies form separate groups.

To verify selected facts, provide a JSON object containing `schema_version: 1`,
the exact `comparison_fingerprint` and `records_fingerprint`, and a `facts` array.
Each fact carries the table's `group_fingerprint` and `input_fingerprint`, the
`metric` name, and an exact copy of that cell's `value`, `observations`, `source`,
and `coverage_source`. Keep this bounded derived file in ignored, untracked
primary `.ahu/evals/` or external storage. `eval verify` defaults its records input
to primary `.ahu/evals/runs.jsonl`; `--records` explicitly overrides that path.
The facts file follows the same storage checks as records: tracked or unignored
repository paths, leaf symlinks, redirected repository directories, hard links
and special files are refused. Verification reads both inputs without creating
or changing local storage.

```sh
ahu eval verify --records /external/runs.jsonl --facts /external/facts.json
```

Verification recomputes the comparison from the current records. Mixed or stale
fingerprints, changed numerical values or denominators, unknown fields, and
arbitrary narrative are rejected. Successful output contains locally generated
sentences identifying the verified facts. It does not approve causal
explanations, model attestations, or replacements for deterministic scores.
Claims are limited to 64 KiB and 128 facts.

Hashes identify content; they do not authenticate a file against its owner.
Provider/model observations are bounded summaries, not reconstructed sequences
of identity pairs. A local Ollama model name does not identify its model digest,
daemon binary, remote forwarding behavior, or every inference backend. Record
those independently when an experiment needs them, and retain unknowns when
supported evidence is unavailable. Never substitute a configured endpoint or a
requested model for an observed identity.

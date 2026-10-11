# Evidence-based agent recommendations

`ahu_agents_recommend` is a read-only MCP tool over a bounded eval JSONL file in external storage or the primary checkout’s
ignored, untracked `.ahu/evals/` directory.
Supply an absolute `records_path`, explicit `cases` with `case_id` and SHA-256
`case_digest`, registered `candidates` such as `@worker`, `minimum_quality`,
`minimum_samples_per_case`, and `maximum_age_seconds`. Discover the complete
schema through MCP `tools/list` before calling it.

Every selected case needs fresh, compatible observations bound to the current
registered agent version, context digest, harness, and exact model. Requested
and reported identity must be complete and consistent. Local records are editable
evidence, not authenticated provider attestation. Legacy records remain readable;
missing provenance makes advice unknown.

Quality uses the lower bound of a 95% Wilson interval for deterministic answer
passes on each case. Known no-answer failures count against quality. The interval
does not prove independent sampling. A candidate below the requested quality
threshold cannot win through low cost; an unknown member prevents recommending
from an incomplete pool.

Qualified candidates are compared using equally weighted case means of observed
harness-reported USD with supported source labels. Reported zero and missing cost
are distinct. Tokens remain separate fields with their own observation counts;
no list prices, quota percentages, or Kiro credits are converted into dollars.
Mixed cost sources, retries, repeated task identities, and incompatible record
contexts cause abstention. Unobserved parent/child overlap and workflow overhead
remain limitations, so these observations are not a complete bill.

Inputs are limited to 8 MiB, 4096 records, eight candidates and sixteen cases.
Results include evidence age, sample counts, coverage, uncertainty, reasons and
an input fingerprint. The tool does not write records, launch work, change auth
profiles, or enforce admission. Use `ahu_auth_budget` separately for current
identity-bound rate-limit evidence.

Keep bounded eval records and derived summaries in ignored, untracked primary
`.ahu/evals/` or external storage. Keep raw prompts, transcripts, tool payloads,
telemetry captures and private tracker content out of these local results. Do not
commit or publish local observations as part of release artifacts. A synthetic regression proves
ranking rules; live comparisons must use a fixed task/candidate pool and report
missing measurements and failed attempts without claiming unsupported gains.

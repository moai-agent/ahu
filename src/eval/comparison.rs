//! Content-bound comparison facts. This is deliberately not a free-text judge:
//! callers may select measured facts, but cannot supply explanations, causal
//! claims, model attestations, or replacement scores.
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Report, fingerprint::content_fingerprint, group_json};
use crate::util::{Error, Result};

/// A deterministic table with explicit JSON-pointer sources and denominators.
/// Hashes bind content, not authenticity of locally editable record files.
pub fn evidence(report: &Report) -> Value {
    let groups: Vec<_> = report.groups.iter().map(group_json).collect();
    let mut rows = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        let group_fingerprint = content_fingerprint("eval-group-v1", group);
        let mut metrics = serde_json::Map::new();
        for (name, source, coverage) in [
            (
                "answer_mean",
                "/answer/mean_score",
                "/answer/score_observations",
            ),
            ("answer_pass_rate", "/answer/pass_rate", "/runs"),
            (
                "answer_quality_rate",
                "/answer/quality_rate",
                "/answer/observations",
            ),
            (
                "elapsed_ms",
                "/observed/mean_elapsed_ms",
                "/coverage/timing_observations",
            ),
            (
                "trajectory_steps",
                "/trajectory/means/steps",
                "/trajectory/observations/steps",
            ),
            ("judge_mean", "/judge/mean_score", "/judge/scored"),
        ] {
            metrics.insert(
                name.into(),
                json!({
                    "value": group.pointer(source).unwrap_or(&Value::Null),
                    "observations": group.pointer(coverage).unwrap_or(&Value::Null),
                    "source": group.pointer(source).map(|_| format!("/groups/{index}{source}")),
                    "coverage_source": group.pointer(coverage).map(|_| format!("/groups/{index}{coverage}")),
                }),
            );
        }
        rows.push(json!({
            "group_fingerprint": group_fingerprint,
            "input_fingerprint": group["input_fingerprint"],
            "fingerprint_completeness": group["fingerprint_completeness"],
            "metrics": metrics,
        }));
    }
    let comparison_fingerprint = content_fingerprint(
        "eval-comparison-v1",
        &json!({
            "records_fingerprint": report.records_fingerprint,
            "record_count": report.record_count,
            "groups": groups,
            "table": rows,
        }),
    );
    json!({
        "schema_version": 1,
        "comparison_fingerprint": comparison_fingerprint,
        "records_fingerprint": report.records_fingerprint,
        "table": rows,
        "verifier_contract": "v1: exact comparison/records/group/input fingerprints plus metric, value, observations, source and coverage_source; no free text or causal claims; no score overrides",
        "identity_scope": "requested and reported identities are separate observations, never attestation; absent, redacted, conflicting and limited evidence cannot establish identity",
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Claims {
    schema_version: u32,
    comparison_fingerprint: String,
    records_fingerprint: String,
    facts: Vec<Fact>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fact {
    group_fingerprint: String,
    input_fingerprint: String,
    metric: String,
    value: Value,
    observations: Value,
    source: Value,
    coverage_source: Value,
}

/// Read derived facts through the same storage boundary as bounded eval records.
pub fn verify_facts_file(
    repo: &crate::git::Repo,
    report: &Report,
    path: &std::path::Path,
) -> Result<Vec<String>> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    let path = super::storage::results_path(repo, path)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    crate::storage::validate_owned_metadata(&file.metadata()?, false)?;
    let mut text = String::new();
    file.take(64 * 1024 + 1).read_to_string(&mut text)?;
    verify_facts(report, &text)
}

/// Verify a bounded selection of facts against freshly computed evidence.
/// The returned prose is generated locally. Arbitrary narrative is never
/// accepted as verified, even when trajectory and model evidence are present.
pub fn verify_facts(report: &Report, claims: &str) -> Result<Vec<String>> {
    if claims.len() > 64 * 1024 {
        return Err(Error::new("comparison facts exceed byte limit"));
    }
    let claims: Claims = serde_json::from_str(claims)?;
    let evidence = evidence(report);
    if claims.schema_version != 1
        || claims.facts.len() > 128
        || claims.comparison_fingerprint != evidence["comparison_fingerprint"]
        || claims.records_fingerprint != report.records_fingerprint
    {
        return Err(Error::new("comparison facts do not bind to current inputs"));
    }
    let mut rendered = Vec::new();
    for fact in claims.facts {
        let row = evidence["table"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| {
                row["group_fingerprint"] == fact.group_fingerprint
                    && row["input_fingerprint"] == fact.input_fingerprint
            })
            .ok_or_else(|| Error::new("unknown comparison group/input reference"))?;
        let cell = row["metrics"]
            .get(&fact.metric)
            .ok_or_else(|| Error::new("unsupported comparison fact"))?;
        if cell["value"] != fact.value
            || cell["observations"] != fact.observations
            || cell["source"] != fact.source
            || cell["coverage_source"] != fact.coverage_source
        {
            return Err(Error::new("comparison value/source/coverage mismatch"));
        }
        rendered.push(format!(
            "Comparison {}, group {}: {} = {} (observations {}; source {}; coverage {}).",
            claims.comparison_fingerprint,
            fact.group_fingerprint,
            fact.metric,
            fact.value,
            fact.observations,
            fact.source,
            fact.coverage_source
        ));
    }
    Ok(rendered)
}

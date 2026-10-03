//! Pure, opt-in preparation for a host-owned private mapping store.
//! No filesystem, provider, environment, task mutation, or exporter access.
//! This API is deliberately not connected to CLI, MCP, or launch state.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    config::TelemetryConfig,
    headless::{ReportedCost, TokenUsage},
    task,
    util::Error,
};

const MAX_BYTES: usize = 64 * 1024;
const MAX_TASKS: usize = 256;
const MAX_OBSERVATIONS: usize = 4096;

/// Sensitive host input: intentionally neither Debug nor Serialize.
/// The opaque record key is meaningful only to the caller's private tracker.
/// It must never be copied into task state, prompts, config, or exports.
///
/// ```compile_fail
/// fn serializable<T: serde::Serialize>() {}
/// serializable::<ahu::telemetry::private::PrivateMapping>();
/// ```
///
/// ```compile_fail
/// fn diagnostic<T: std::fmt::Debug>() {}
/// diagnostic::<ahu::telemetry::private::PrivateMapping>();
/// ```
pub struct PrivateMapping {
    record_key: String,
    repo_identity: String,
    tasks: BTreeSet<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MappingInput {
    schema_version: u32,
    record_key: String,
    repo_identity: String,
    tasks: Vec<String>,
}

fn invalid() -> Error {
    // Never expose serde diagnostics, submitted field names, or private values.
    Error::new("invalid private telemetry input")
}

impl PrivateMapping {
    /// Validate bounded, explicit membership without fetching tracker content.
    /// Repository identity is the current machine's 16 lowercase hex digest;
    /// task keys must be canonical UUIDs, never handles or worktree paths.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_BYTES {
            return Err(invalid());
        }
        let input: MappingInput = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        if input.schema_version != 1
            || input.record_key.is_empty()
            || input.record_key.len() > 256
            || !input
                .record_key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            || input.repo_identity.len() != 16
            || !input
                .repo_identity
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || input.tasks.is_empty()
            || input.tasks.len() > MAX_TASKS
            || input.tasks.iter().any(|id| {
                !task::is_canonical_task_uuid(id)
                    || [8, 13, 18, 23]
                        .into_iter()
                        .any(|offset| id.as_bytes()[offset] != b'-')
            })
        {
            return Err(invalid());
        }
        let tasks: BTreeSet<_> = input.tasks.iter().cloned().collect();
        if tasks.len() != input.tasks.len() {
            return Err(invalid());
        }
        Ok(Self {
            record_key: input.record_key,
            repo_identity: input.repo_identity,
            tasks,
        })
    }

    /// Explicit access for a future private host adapter; never an export label.
    pub fn record_key(&self) -> &str {
        &self.record_key
    }

    /// Summarize only explicitly supplied, opted-in observations. No implicit
    /// child traversal or retry discovery. Repeated identical attempts count
    /// once; conflicting snapshots fail rather than selecting an arbitrary one.
    /// Missing samples are not invented. Coverage describes supplied attempts.
    pub fn summarize(
        &self,
        config: &TelemetryConfig,
        samples: &[AttemptObservation<'_>],
    ) -> Result<Option<Summary>, Error> {
        if !config.local_metrics {
            return Ok(None);
        }
        if samples.len() > MAX_OBSERVATIONS {
            return Err(invalid());
        }
        let mut attempts = BTreeMap::new();
        let mut group: Option<PrivateGroup> = None;
        for sample in samples {
            if sample.repo_identity != self.repo_identity
                || !self.tasks.contains(sample.task_id)
                || sample.attempt == 0
                || !matches!(
                    sample.harness,
                    "codex" | "claude-code" | "antigravity" | "opencode"
                )
                || !safe_model_name(sample.model)
                || !matches!(
                    sample.outcome,
                    "succeeded"
                        | "failed"
                        | "cancelled"
                        | "timed_out"
                        | "capture_failed"
                        | "boundary_violation"
                        | "interrupted"
                        | "supervisor_error"
                        | "unknown"
                        | "running"
                )
                || sample.agent_identity_digest.is_some_and(|digest| {
                    digest.len() != 64
                        || !digest
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
            {
                return Err(invalid());
            }
            let candidate_group = PrivateGroup {
                repository_identity: sample.repo_identity.to_owned(),
                agent_identity_digest: sample.agent_identity_digest.map(str::to_owned),
                harness: sample.harness.to_owned(),
                model: sample.model.to_owned(),
                outcome: sample.outcome.to_owned(),
            };
            if group
                .as_ref()
                .is_some_and(|previous| previous != &candidate_group)
            {
                // Never average token or cost data across different execution
                // identities or outcomes. Callers build one report group at a time.
                return Err(invalid());
            }
            group.get_or_insert(candidate_group);
            let key = (sample.task_id, sample.attempt);
            if sample
                .cost
                .usd
                .is_some_and(|usd| !usd.is_finite() || usd < 0.0)
                || sample.cost.usd.is_some() != sample.cost.source.is_some()
                || sample.cost.source.as_deref().is_some_and(|source| {
                    !matches!(
                        source,
                        "claude_code_result_total" | "opencode_step_finish_sum"
                    ) || (source == "claude_code_result_total" && sample.harness != "claude-code")
                        || (source == "opencode_step_finish_sum" && sample.harness != "opencode")
                })
            {
                return Err(invalid());
            }
            let observation = (
                sample.usage.normalized_fields(),
                sample.cost,
                sample.elapsed_ms,
            );
            if let Some(previous) = attempts.insert(key, observation)
                && previous != observation
            {
                return Err(invalid());
            }
        }
        let mut values: BTreeMap<_, _> = TokenUsage::default()
            .normalized_fields()
            .into_iter()
            .map(|(key, _)| {
                (
                    key,
                    Coverage {
                        maximum_observed: None,
                        observed_attempts: 0,
                        unavailable_attempts: 0,
                    },
                )
            })
            .collect();
        for fields in attempts.values() {
            for (key, value) in fields.0 {
                let coverage = values.get_mut(key).expect("fixed normalized field");
                if let Some(value) = value {
                    coverage.maximum_observed = Some(
                        coverage
                            .maximum_observed
                            .map_or(value, |old| old.max(value)),
                    );
                    coverage.observed_attempts += 1;
                } else {
                    coverage.unavailable_attempts += 1;
                }
            }
        }
        Ok(Some(Summary {
            schema_version: 2,
            token_aggregation: "maximum-reported-per-field-across-supplied-attempts",
            attempts: attempts.len(),
            values,
            group: group.unwrap_or_else(|| PrivateGroup {
                repository_identity: self.repo_identity.clone(),
                agent_identity_digest: None,
                harness: "unknown".into(),
                model: "unknown".into(),
                outcome: "unknown".into(),
            }),
            elapsed_ms: aggregate_elapsed(attempts.values().map(|(_, _, elapsed)| *elapsed)),
            reported_cost: aggregate_reported_cost(attempts.values().map(|(_, cost, _)| *cost)),
        }))
    }
}

/// Caller must verify repository/task ownership and read only opted-in numeric
/// projections. This struct does not authenticate results or read raw envelopes.
pub struct AttemptObservation<'a> {
    pub repo_identity: &'a str,
    pub task_id: &'a str,
    pub attempt: u32,
    pub agent_identity_digest: Option<&'a str>,
    pub harness: &'a str,
    pub model: &'a str,
    pub outcome: &'a str,
    pub elapsed_ms: Option<u64>,
    pub usage: &'a TokenUsage,
    pub cost: &'a ReportedCost,
}

/// Grouped private measurements; no mapping key, task IDs, paths, or arbitrary
/// attributes. The caller associates the mapping key with this result locally.
/// This API does not export it.
#[derive(Debug, Serialize)]
pub struct Summary {
    schema_version: u32,
    group: PrivateGroup,
    elapsed_ms: TimingCoverage,
    token_aggregation: &'static str,
    attempts: usize,
    values: BTreeMap<&'static str, Coverage>,
    reported_cost: CostCoverage,
}

#[derive(Debug, PartialEq, Serialize)]
struct PrivateGroup {
    repository_identity: String,
    agent_identity_digest: Option<String>,
    harness: String,
    model: String,
    outcome: String,
}

#[derive(Debug, Serialize)]
struct TimingCoverage {
    mean_observed_ms: Option<f64>,
    observed_attempts: usize,
    unavailable_attempts: usize,
}

fn safe_model_name(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 128
        && model != "."
        && model != ".."
        && !model.contains("//")
        && model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
}

fn aggregate_elapsed(elapsed: impl Iterator<Item = Option<u64>>) -> TimingCoverage {
    let mut values = Vec::new();
    let mut unavailable_attempts = 0;
    for value in elapsed {
        if let Some(value) = value {
            values.push(value);
        } else {
            unavailable_attempts += 1;
        }
    }
    TimingCoverage {
        mean_observed_ms: (!values.is_empty())
            .then(|| values.iter().map(|value| *value as f64).sum::<f64>() / values.len() as f64),
        observed_attempts: values.len(),
        unavailable_attempts,
    }
}

#[derive(Debug, Serialize)]
struct Coverage {
    maximum_observed: Option<u64>,
    observed_attempts: u64,
    unavailable_attempts: u64,
}

fn aggregate_reported_cost<'a>(costs: impl Iterator<Item = &'a ReportedCost>) -> CostCoverage {
    let mut amounts: BTreeMap<&'static str, (usize, f64)> = BTreeMap::new();
    let mut unavailable_attempts = 0;
    for cost in costs {
        let Some(source) = cost.source.as_deref() else {
            unavailable_attempts += 1;
            continue;
        };
        if let Some(amount) = cost.usd {
            let (count, mean) = amounts
                .entry(match source {
                    "claude_code_result_total" => "claude_code_result_total",
                    "opencode_step_finish_sum" => "opencode_step_finish_sum",
                    _ => unreachable!("source validated before aggregation"),
                })
                .or_default();
            *count += 1;
            *mean += (amount - *mean) / *count as f64;
        }
    }
    CostCoverage {
        mean_observed_usd_by_source: amounts
            .iter()
            .map(|(source, (_, mean))| (*source, *mean))
            .collect(),
        observed_attempts_by_source: amounts
            .iter()
            .map(|(source, (count, _))| (*source, *count as u64))
            .collect(),
        unavailable_attempts,
    }
}

#[derive(Debug, Serialize)]
struct CostCoverage {
    mean_observed_usd_by_source: BTreeMap<&'static str, f64>,
    observed_attempts_by_source: BTreeMap<&'static str, u64>,
    unavailable_attempts: usize,
}

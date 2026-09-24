//! Pure, opt-in preparation for a host-owned private mapping store.
//! No filesystem, provider, environment, task mutation, or exporter access.
//! This API is deliberately not connected to CLI, MCP, or launch state.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{config::TelemetryConfig, headless::TokenUsage, task, util::Error};

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
        for sample in samples {
            if sample.repo_identity != self.repo_identity
                || !self.tasks.contains(sample.task_id)
                || sample.attempt == 0
            {
                return Err(invalid());
            }
            let key = (sample.task_id, sample.attempt);
            let fields = sample.usage.normalized_fields();
            if let Some(previous) = attempts.insert(key, fields)
                && previous != fields
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
            for (key, value) in fields {
                let coverage = values.get_mut(key).expect("fixed normalized field");
                if let Some(value) = value {
                    coverage.maximum_observed = Some(
                        coverage
                            .maximum_observed
                            .map_or(*value, |old| old.max(*value)),
                    );
                    coverage.observed_attempts += 1;
                } else {
                    coverage.unavailable_attempts += 1;
                }
            }
        }
        Ok(Some(Summary {
            schema_version: 1,
            token_aggregation: "maximum-reported-per-field-across-supplied-attempts",
            attempts: attempts.len(),
            values,
        }))
    }
}

/// Caller must verify repository/task ownership and read only opted-in numeric
/// projections. This struct does not authenticate results or read raw envelopes.
pub struct AttemptObservation<'a> {
    pub repo_identity: &'a str,
    pub task_id: &'a str,
    pub attempt: u32,
    pub usage: &'a TokenUsage,
}

/// Numeric-only output; no mapping, task IDs, paths, or arbitrary attributes.
/// Local output only: this API never exports it.
#[derive(Debug, Serialize)]
pub struct Summary {
    schema_version: u32,
    token_aggregation: &'static str,
    attempts: usize,
    values: BTreeMap<&'static str, Coverage>,
}

#[derive(Debug, Serialize)]
struct Coverage {
    maximum_observed: Option<u64>,
    observed_attempts: u64,
    unavailable_attempts: u64,
}

//! Bounded identity observations, not model attestation. Only ahu-owned
//! attribute names are projected; requested identity is never reported identity.
use std::collections::BTreeMap;

use opentelemetry_proto::tonic::common::v1::{KeyValue, any_value::Value};
use serde::{Deserialize, Serialize};

const MAX_VALUES: usize = 16;

pub fn safe_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-:/".contains(&b))
        && !value.contains("..")
        && !value.contains(":/")
        && value.matches('/').count() <= 1
        && value.matches(':').count() <= 1
        && !value.starts_with("sk-")
        && value != "localhost"
        && value.parse::<std::net::IpAddr>().is_err()
        && value
            .split(':')
            .nth(1)
            .is_none_or(|tag| tag.bytes().any(|b| b.is_ascii_alphabetic()))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub values: BTreeMap<String, u64>,
    pub missing: u64,
    pub redacted: u64,
    pub conflicting: u64,
    pub limited: u64,
    /// Observations with incomplete native coverage, independent of value counts.
    #[serde(default)]
    pub partial: u64,
}

impl Identity {
    fn observe<'a>(&mut self, attributes: &'a [KeyValue], key: &str) -> Option<&'a str> {
        let mut found = false;
        let mut first: Option<&str> = None;
        let mut conflict = false;
        let mut redacted = false;
        let mut partial = false;
        // Producer coverage is a fixed label, never an arbitrary identity value.
        let coverage_key = format!("{key}.coverage");
        for attribute in attributes.iter().filter(|a| a.key == coverage_key) {
            match attribute.value.as_ref().and_then(|v| v.value.as_ref()) {
                Some(Value::StringValue(value)) if value == "conflicting" => conflict = true,
                Some(Value::StringValue(value)) if value == "redacted" => redacted = true,
                Some(Value::StringValue(value)) if value == "partial" => partial = true,
                Some(Value::StringValue(value)) if value == "complete" => (),
                _ => redacted = true,
            }
        }
        self.partial = self.partial.saturating_add(u64::from(partial));
        for attribute in attributes.iter().filter(|a| a.key == key) {
            found = true;
            let Some(Value::StringValue(value)) =
                attribute.value.as_ref().and_then(|v| v.value.as_ref())
            else {
                redacted = true;
                continue;
            };
            if !safe_identity(value) {
                redacted = true;
                continue;
            }
            if let Some(previous) = first {
                conflict |= previous != value;
            } else {
                first = Some(value);
            }
        }
        if conflict {
            self.conflicting = self.conflicting.saturating_add(1);
        } else if redacted {
            self.redacted = self.redacted.saturating_add(1);
        } else if let Some(value) = first {
            self.add_value(value, 1);
        }
        if !found && !conflict && !redacted {
            self.missing = self.missing.saturating_add(1);
        }
        if conflict || redacted { None } else { first }
    }

    fn add_value(&mut self, value: &str, count: u64) {
        if self.values.contains_key(value) || self.values.len() < MAX_VALUES {
            let entry = self.values.entry(value.to_owned()).or_default();
            *entry = entry.saturating_add(count);
        } else {
            self.limited = self.limited.saturating_add(count);
        }
    }

    fn merge(&mut self, other: &Self) {
        for (value, count) in &other.values {
            self.add_value(value, *count);
        }
        self.missing = self.missing.saturating_add(other.missing);
        self.redacted = self.redacted.saturating_add(other.redacted);
        self.conflicting = self.conflicting.saturating_add(other.conflicting);
        self.limited = self.limited.saturating_add(other.limited);
        self.partial = self.partial.saturating_add(other.partial);
    }

    pub fn status(&self) -> &'static str {
        if self.conflicting > 0 {
            "conflicting"
        } else if self.limited > 0 {
            "limited"
        } else if self.redacted > 0 {
            "redacted"
        } else if self.values.len() > 1 {
            "multiple"
        } else if self.values.is_empty() {
            "absent"
        } else if self.missing > 0 || self.partial > 0 {
            "partial"
        } else {
            "single"
        }
    }

    fn valid(&self, observations: u64) -> bool {
        self.values.len() <= MAX_VALUES
            && self.partial <= observations
            && self.values.iter().all(|(v, n)| safe_identity(v) && *n > 0)
            && self
                .values
                .values()
                .chain([
                    &self.missing,
                    &self.redacted,
                    &self.conflicting,
                    &self.limited,
                ])
                .try_fold(0u64, |sum, n| sum.checked_add(*n))
                .is_some_and(|n| n <= observations)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub observations: u64,
    pub model_mismatches: u64,
    pub provider_mismatches: u64,
    pub requested_provider: Identity,
    pub requested_model: Identity,
    pub reported_provider: Identity,
    pub reported_model: Identity,
    /// Older exporter model attributes did not say whether the value was reported.
    pub unattributed_model: Identity,
}

impl Provenance {
    pub fn observe(&mut self, attributes: &[KeyValue], prefix: &str) {
        self.observations = self.observations.saturating_add(1);
        let requested_provider = self
            .requested_provider
            .observe(attributes, &format!("{prefix}.backend"));
        let requested_model = self
            .requested_model
            .observe(attributes, &format!("{prefix}.model.requested"));
        let reported_provider = self
            .reported_provider
            .observe(attributes, &format!("{prefix}.provider.reported"));
        let reported_model = self
            .reported_model
            .observe(attributes, &format!("{prefix}.model.reported"));
        self.unattributed_model
            .observe(attributes, &format!("{prefix}.model"));
        if requested_model
            .zip(reported_model)
            .is_some_and(|(a, b)| a != b)
        {
            self.model_mismatches = self.model_mismatches.saturating_add(1);
        }
        // A harness backend (e.g. opencode) is not an inference provider.
        if prefix != "ahu.harness"
            && requested_provider
                .zip(reported_provider)
                .is_some_and(|(a, b)| a != b)
        {
            self.provider_mismatches = self.provider_mismatches.saturating_add(1);
        }
    }

    pub fn valid(&self) -> bool {
        self.model_mismatches <= self.observations
            && self.provider_mismatches <= self.observations
            && [
                &self.requested_provider,
                &self.requested_model,
                &self.reported_provider,
                &self.reported_model,
                &self.unattributed_model,
            ]
            .iter()
            .all(|v| v.valid(self.observations))
    }

    pub fn merge(&mut self, other: &Self) {
        self.observations = self.observations.saturating_add(other.observations);
        self.model_mismatches = self.model_mismatches.saturating_add(other.model_mismatches);
        self.provider_mismatches = self
            .provider_mismatches
            .saturating_add(other.provider_mismatches);
        self.requested_provider.merge(&other.requested_provider);
        self.requested_model.merge(&other.requested_model);
        self.reported_provider.merge(&other.reported_provider);
        self.reported_model.merge(&other.reported_model);
        self.unattributed_model.merge(&other.unattributed_model);
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub recorded_runs: usize,
    pub unknown_runs: usize,
    pub evidence: Provenance,
}

impl Summary {
    pub fn collect<'a>(items: impl Iterator<Item = Option<&'a Provenance>>) -> Self {
        let mut summary = Self::default();
        for item in items {
            if let Some(item) = item {
                summary.recorded_runs += 1;
                summary.evidence.merge(item);
            } else {
                summary.unknown_runs += 1;
            }
        }
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_identity_defaults_partial_and_partial_counts_are_bounded_and_merged() {
        let legacy = serde_json::json!({"values":{"fixture":1}, "missing":0,
            "redacted":0, "conflicting":0, "limited":0});
        let mut identity: Identity = serde_json::from_value(legacy).unwrap();
        assert_eq!(identity.partial, 0);
        assert_eq!(identity.status(), "single");
        identity.partial = 1;
        assert!(identity.valid(1));
        assert_eq!(identity.status(), "partial");
        let mut merged = Identity::default();
        merged.merge(&identity);
        assert_eq!(merged.partial, 1);
        assert_eq!(merged.status(), "partial");
        assert!(merged.valid(1));
        merged.partial = 2;
        assert!(!merged.valid(1));
    }

    #[test]
    fn identities_reject_endpoints_payloads_paths_and_overlong_values() {
        for value in [
            "",
            "https://example.invalid",
            "host:1234",
            "/tmp/model",
            "../model",
            "user@host",
            "{\"model\":1}",
            "model\nsecret",
            "sk-secret",
            "a/b/c",
        ] {
            assert!(!safe_identity(value), "{value}");
        }
        assert!(!safe_identity(&"a".repeat(129)));
        for value in [
            "fixture",
            "provider/model-1.2",
            "model:cloud",
            "model:32b",
            "gpt-6-astra",
        ] {
            assert!(safe_identity(value), "{value}");
        }
    }

    #[test]
    fn aggregation_remains_bounded_and_reports_multiple_and_missing() {
        let mut summary = Provenance::default();
        for index in 0..20 {
            let mut observation = Provenance {
                observations: 1,
                ..Default::default()
            };
            observation
                .reported_model
                .values
                .insert(format!("model-{index}"), 1);
            summary.merge(&observation);
        }
        assert_eq!(summary.reported_model.values.len(), 16);
        assert_eq!(summary.reported_model.limited, 4);
        assert!(summary.valid());
        let mut identity = Identity::default();
        assert_eq!(identity.status(), "absent");
        identity.add_value("one", 1);
        assert_eq!(identity.status(), "single");
        identity.missing = 1;
        assert_eq!(identity.status(), "partial");
        identity.add_value("two", 1);
        assert_eq!(identity.status(), "multiple");
    }
}

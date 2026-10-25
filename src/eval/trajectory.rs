//! Bounded projections of native event streams. No payloads or identifiers are
//! serialized; a repeated tool is not necessarily redundant or unproductive.

use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    #[default]
    None,
    Partial,
    /// Complete only for the supported shapes in the captured native stream,
    /// never a claim about provider internals or helper work.
    CompleteObservedStream,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    #[default]
    Unsupported,
    CodexTurns,
    ClaudeAssistantMessages,
    OpencodeSteps,
    KiroSessionUpdates,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub source: Source,
    pub coverage: Coverage,
    /// Independent partial metering, never model steps, tokens or USD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kiro_metering: Option<KiroMetering>,
    pub steps: Option<u64>,
    pub tool_calls: Option<u64>,
    /// Calls with an observed, classified terminal outcome. Absent in legacy records.
    #[serde(default)]
    pub completed_tool_calls: Option<u64>,
    pub tool_errors: Option<u64>,
    pub repeated_tool_calls: Option<u64>,
    pub repeated_tool_errors: Option<u64>,
    /// Success after an observed error from the same tool; not task recovery.
    pub tool_recoveries: Option<u64>,
    pub malformed_events: u64,
    pub unclassified_events: u64,
    pub tracking_limited: bool,
    pub stream_incomplete: bool,
}

/// Kiro V3 session-info evidence. Amounts are sums of accepted observations,
/// not complete attempt/session accounting. Context is a window snapshot.
/// No native identifiers, tool names, breakdown contents or payloads persist.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KiroMetering {
    pub context_usage_percent: Option<f64>,
    pub peak_context_usage_percent: Option<f64>,
    /// Prompt completions may contain multiple model requests. Not `steps`.
    pub prompt_completions: Option<u64>,
    pub observed_credits: Option<f64>,
    pub credit_observations: u64,
    pub observed_elapsed_ms: Option<f64>,
    pub elapsed_observations: u64,
    pub duplicate_completions: u64,
    pub rejected_updates: u64,
    /// Includes replay before a live prompt marker and unanchored completions.
    pub unattributed_completions: u64,
}

impl KiroMetering {
    pub fn valid(&self) -> bool {
        [self.context_usage_percent, self.peak_context_usage_percent]
            .into_iter()
            .flatten()
            .all(|v| v.is_finite() && (0.0..=100.0).contains(&v))
            && [self.observed_credits, self.observed_elapsed_ms]
                .into_iter()
                .flatten()
                .all(|v| v.is_finite() && v >= 0.0)
            && (self.context_usage_percent.is_some() == self.peak_context_usage_percent.is_some())
            && self
                .context_usage_percent
                .zip(self.peak_context_usage_percent)
                .is_none_or(|(last, peak)| last <= peak)
            && (self.observed_credits.is_some() == (self.credit_observations > 0))
            && (self.observed_elapsed_ms.is_some() == (self.elapsed_observations > 0))
            && self.credit_observations <= self.prompt_completions.unwrap_or(0)
            && self.elapsed_observations <= self.prompt_completions.unwrap_or(0)
    }
}

impl Observation {
    /// A rate needs a known numerator and completed-call denominator. Never
    /// infer completions from starts, including for legacy records.
    pub fn tool_error_rate(&self) -> Option<f64> {
        if self.coverage == Coverage::None || self.source == Source::Unsupported {
            return None;
        }
        let calls = self.tool_calls?;
        let completed = self.completed_tool_calls?;
        let errors = self.tool_errors?;
        if errors > completed || completed > calls {
            return None;
        }
        if self.complete() && completed != calls {
            return None;
        }
        if completed == 0 {
            return self.complete().then_some(0.0);
        }
        Some(errors as f64 / completed as f64)
    }

    pub fn complete(&self) -> bool {
        self.coverage == Coverage::CompleteObservedStream
            && self.source != Source::Unsupported
            && !self.tracking_limited
            && !self.stream_incomplete
            && self.malformed_events == 0
            && self.unclassified_events == 0
    }
}

/// Validated finite fraction; NaN cannot enter through CLI, YAML or JSON.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f64", into = "f64")]
pub struct ErrorRate(f64);
impl Eq for ErrorRate {}
impl TryFrom<f64> for ErrorRate {
    type Error = crate::util::Error;
    fn try_from(value: f64) -> Result<Self, Self::Error> {
        if value.is_finite() && (0.0..=1.0).contains(&value) {
            Ok(Self(value))
        } else {
            Err(
                crate::util::Error::new("max tool error rate must be finite and in 0..1")
                    .with_kind(crate::util::ErrorKind::Usage),
            )
        }
    }
}
impl From<ErrorRate> for f64 {
    fn from(value: ErrorRate) -> f64 {
        value.0
    }
}
impl std::str::FromStr for ErrorRate {
    type Err = crate::util::Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.parse::<f64>().map_err(|_| {
            crate::util::Error::new("max tool error rate must be finite and in 0..1")
                .with_kind(crate::util::ErrorKind::Usage)
        })?;
        Self::try_from(value)
    }
}

/// Opt-in CI limits. Case budgets remain separately scored under their own
/// policy; effective CLI limits can only tighten them and require coverage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Guardrails {
    pub max_steps: Option<u32>,
    pub max_tool_error_rate: Option<ErrorRate>,
}
impl Guardrails {
    pub fn enabled(&self) -> bool {
        self.max_steps.is_some() || self.max_tool_error_rate.is_some()
    }

    pub fn effective(&self, case: Option<&Budgets>) -> Option<Budgets> {
        if !self.enabled() {
            return None;
        }
        let mut effective = case.cloned().unwrap_or_default();
        if let Some(maximum) = self.max_steps {
            effective.max_steps = Some(
                effective
                    .max_steps
                    .map_or(u64::from(maximum), |case| case.min(u64::from(maximum))),
            );
        }
        if let Some(maximum) = self.max_tool_error_rate {
            effective.max_tool_error_rate = Some(
                effective
                    .max_tool_error_rate
                    .map_or(maximum, |case| ErrorRate(case.0.min(maximum.0))),
            );
        }
        // Unknown remains distinct in the record, and fails the CLI exit gate.
        // A case's observed_only policy never weakens CLI coverage requirements.
        effective.unknown_coverage = UnknownPolicy::Unknown;
        Some(effective)
    }
}

/// Validated cumulative observations, with no provider parser state.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Observer {
    #[serde(flatten)]
    pub observation: Observation,
}
impl Observer {
    pub fn incomplete(&mut self) {
        self.observation.stream_incomplete = true;
        if self.observation.coverage == Coverage::CompleteObservedStream {
            self.observation.coverage = Coverage::Partial;
        }
    }
    pub fn malformed(&mut self) {
        self.observation.malformed_events = self.observation.malformed_events.saturating_add(1);
        self.incomplete();
    }
}

/// Optional case budgets. They score separately from answer correctness.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Budgets {
    pub max_steps: Option<u64>,
    pub max_tool_errors: Option<u64>,
    pub max_tool_error_rate: Option<ErrorRate>,
    #[serde(default)]
    pub unknown_coverage: UnknownPolicy,
}
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownPolicy {
    #[default]
    Unknown,
    Fail,
    /// Explicitly judge observed counts and rates even when coverage is partial.
    ObservedOnly,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetStatus {
    NotApplicable,
    Pass,
    Fail,
    Unknown,
}
impl Budgets {
    pub fn score(&self, observation: Option<&Observation>) -> BudgetStatus {
        let o = observation;
        let checks = [
            (self.max_steps, o.and_then(|o| o.steps)),
            (self.max_tool_errors, o.and_then(|o| o.tool_errors)),
        ];
        if checks.iter().all(|(budget, _)| budget.is_none()) && self.max_tool_error_rate.is_none() {
            return BudgetStatus::NotApplicable;
        }
        if checks
            .iter()
            .any(|(budget, count)| budget.zip(*count).is_some_and(|(b, c)| c > b))
        {
            return BudgetStatus::Fail;
        }
        let rate = o.and_then(Observation::tool_error_rate);
        let complete = o.is_some_and(Observation::complete);
        // Unlike counts, a partial-stream rate is not a lower bound.
        if (complete || matches!(self.unknown_coverage, UnknownPolicy::ObservedOnly))
            && self
                .max_tool_error_rate
                .zip(rate)
                .is_some_and(|(limit, rate)| rate > limit.0)
        {
            return BudgetStatus::Fail;
        }
        let missing = (self.max_tool_error_rate.is_some() && rate.is_none())
            || checks
                .iter()
                .any(|(budget, count)| budget.is_some() && count.is_none());
        if complete && !missing {
            return BudgetStatus::Pass;
        }
        match self.unknown_coverage {
            UnknownPolicy::Fail => BudgetStatus::Fail,
            UnknownPolicy::ObservedOnly if !missing => BudgetStatus::Pass,
            _ => BudgetStatus::Unknown,
        }
    }
}

/// Fixed-size aggregation; means always carry their observation denominators.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub missing_runs: usize,
    pub coverage: BTreeMap<String, usize>,
    pub sources: BTreeMap<String, usize>,
    pub means: BTreeMap<String, f64>,
    pub observations: BTreeMap<String, usize>,
    pub budget_statuses: BTreeMap<String, usize>,
    pub limited_runs: usize,
    pub guard_statuses: BTreeMap<String, usize>,
    /// Rate coverage is separate: old observations may lack completions.
    pub tool_error_rate_coverage: BTreeMap<String, usize>,
}
impl Summary {
    pub fn collect<'a>(
        records: impl Iterator<
            Item = (
                Option<&'a Observation>,
                Option<BudgetStatus>,
                Option<BudgetStatus>,
            ),
        >,
    ) -> Self {
        let mut s = Self::default();
        for (observation, status, guard) in records {
            let guard = serde_json::to_value(guard.unwrap_or(BudgetStatus::NotApplicable)).unwrap();
            *s.guard_statuses
                .entry(guard.as_str().unwrap().into())
                .or_default() += 1;
            let rate = observation.and_then(Observation::tool_error_rate);
            let rate_coverage = match (rate, observation) {
                (Some(_), Some(o)) if o.complete() => "complete_observed_stream",
                (Some(_), _) => "partial",
                _ => "unknown",
            };
            *s.tool_error_rate_coverage
                .entry(rate_coverage.into())
                .or_default() += 1;
            // Aggregate only complete rates. Partial rates are visible on records
            // with their coverage, never mixed into the complete-run mean.
            if rate_coverage == "complete_observed_stream" {
                *s.means.entry("tool_error_rate".into()).or_default() += rate.unwrap();
                *s.observations.entry("tool_error_rate".into()).or_default() += 1;
            }
            let status =
                serde_json::to_value(status.unwrap_or(BudgetStatus::NotApplicable)).unwrap();
            *s.budget_statuses
                .entry(status.as_str().unwrap().into())
                .or_default() += 1;
            let Some(o) = observation else {
                s.missing_runs += 1;
                continue;
            };
            for (value, map) in [
                (serde_json::to_value(o.coverage).unwrap(), &mut s.coverage),
                (serde_json::to_value(o.source).unwrap(), &mut s.sources),
            ] {
                *map.entry(value.as_str().unwrap().into()).or_default() += 1;
            }
            s.limited_runs += usize::from(o.tracking_limited);
            if let Some(m) = &o.kiro_metering
                && o.source == Source::KiroSessionUpdates
                && m.valid()
            {
                for (name, value) in [
                    ("kiro_context_usage_percent", m.context_usage_percent),
                    (
                        "kiro_peak_context_usage_percent",
                        m.peak_context_usage_percent,
                    ),
                    (
                        "kiro_prompt_completions",
                        m.prompt_completions.map(|v| v as f64),
                    ),
                    ("kiro_observed_credits", m.observed_credits),
                    (
                        "kiro_credit_observations",
                        Some(m.credit_observations as f64),
                    ),
                    ("kiro_observed_elapsed_ms", m.observed_elapsed_ms),
                    (
                        "kiro_elapsed_observations",
                        Some(m.elapsed_observations as f64),
                    ),
                    (
                        "kiro_duplicate_completions",
                        Some(m.duplicate_completions as f64),
                    ),
                    ("kiro_rejected_updates", Some(m.rejected_updates as f64)),
                    (
                        "kiro_unattributed_completions",
                        Some(m.unattributed_completions as f64),
                    ),
                ] {
                    if let Some(value) = value {
                        *s.means.entry(name.into()).or_default() += value;
                        *s.observations.entry(name.into()).or_default() += 1;
                    }
                }
            }
            for (name, value) in [
                ("steps", o.steps),
                ("tool_calls", o.tool_calls),
                ("completed_tool_calls", o.completed_tool_calls),
                ("tool_errors", o.tool_errors),
                ("repeated_tool_calls", o.repeated_tool_calls),
                ("repeated_tool_errors", o.repeated_tool_errors),
                ("tool_recoveries", o.tool_recoveries),
            ] {
                if let Some(value) = value {
                    *s.means.entry(name.into()).or_default() += value as f64;
                    *s.observations.entry(name.into()).or_default() += 1;
                }
            }
        }
        for (name, mean) in &mut s.means {
            *mean /= s.observations[name] as f64;
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn budgets_fail_observed_excess_and_make_unknown_policy_explicit() {
        let mut budget = Budgets {
            max_steps: Some(2),
            max_tool_errors: Some(0),
            max_tool_error_rate: None,
            unknown_coverage: UnknownPolicy::Unknown,
        };
        assert_eq!(budget.score(None), BudgetStatus::Unknown);
        let mut o = Observation {
            source: Source::CodexTurns,
            steps: Some(2),
            tool_errors: Some(0),
            coverage: Coverage::Partial,
            ..Observation::default()
        };
        assert_eq!(budget.score(Some(&o)), BudgetStatus::Unknown);
        budget.unknown_coverage = UnknownPolicy::Fail;
        assert_eq!(budget.score(Some(&o)), BudgetStatus::Fail);
        budget.unknown_coverage = UnknownPolicy::ObservedOnly;
        assert_eq!(budget.score(Some(&o)), BudgetStatus::Pass);
        assert_eq!(budget.score(None), BudgetStatus::Unknown);
        o.tool_errors = Some(1);
        assert_eq!(budget.score(Some(&o)), BudgetStatus::Fail);
        budget.unknown_coverage = UnknownPolicy::Unknown;
        assert_eq!(budget.score(Some(&o)), BudgetStatus::Fail);
        o.tool_errors = Some(0);
        o.coverage = Coverage::CompleteObservedStream;
        assert_eq!(budget.score(Some(&o)), BudgetStatus::Pass);
    }

    fn complete(calls: u64, errors: u64) -> Observation {
        Observation {
            source: Source::CodexTurns,
            coverage: Coverage::CompleteObservedStream,
            steps: Some(2),
            tool_calls: Some(calls),
            completed_tool_calls: Some(calls),
            tool_errors: Some(errors),
            ..Default::default()
        }
    }

    #[test]
    fn rate_rejects_invalid_counts_and_keeps_legacy_and_partial_zero_unknown() {
        assert_eq!(complete(0, 0).tool_error_rate(), Some(0.0));
        assert_eq!(complete(4, 1).tool_error_rate(), Some(0.25));
        for (calls, completed, errors) in [(0, 0, 1), (1, 2, 1), (2, 1, 2), (2, 1, 0)] {
            let mut o = complete(calls, errors);
            o.completed_tool_calls = Some(completed);
            assert_eq!(o.tool_error_rate(), None);
        }
        let mut o = complete(0, 0);
        o.coverage = Coverage::Partial;
        assert_eq!(o.tool_error_rate(), None);
        let mut unsupported = complete(2, 1);
        unsupported.source = Source::Unsupported;
        assert_eq!(unsupported.tool_error_rate(), None);
        unsupported.source = Source::CodexTurns;
        unsupported.coverage = Coverage::None;
        assert_eq!(unsupported.tool_error_rate(), None);
        let mut legacy = serde_json::to_value(complete(2, 1)).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("completed_tool_calls");
        let legacy: Observation = serde_json::from_value(legacy).unwrap();
        assert_eq!(legacy.tool_error_rate(), None);
        for field in ["tool_errors", "tool_calls"] {
            let mut value = serde_json::to_value(complete(2, 1)).unwrap();
            value[field] = Value::Null;
            assert_eq!(
                serde_json::from_value::<Observation>(value)
                    .unwrap()
                    .tool_error_rate(),
                None
            );
        }
    }

    #[test]
    fn guards_tighten_case_limits_and_require_complete_evidence() {
        let case = Budgets {
            max_steps: Some(2),
            max_tool_errors: Some(1),
            max_tool_error_rate: Some(ErrorRate(0.25)),
            unknown_coverage: UnknownPolicy::ObservedOnly,
        };
        for (steps, rate, expected_steps, expected_rate) in [(8, 0.75, 2, 0.25), (1, 0.1, 1, 0.1)] {
            let guards = Guardrails {
                max_steps: Some(steps),
                max_tool_error_rate: Some(ErrorRate(rate)),
            };
            let effective = guards.effective(Some(&case)).unwrap();
            assert_eq!(effective.max_steps, Some(expected_steps));
            assert_eq!(
                effective.max_tool_error_rate,
                Some(ErrorRate(expected_rate))
            );
            assert_eq!(effective.max_tool_errors, Some(1));
            assert!(matches!(effective.unknown_coverage, UnknownPolicy::Unknown));
        }
        assert!(Guardrails::default().effective(Some(&case)).is_none());
        let guards = Guardrails {
            max_steps: Some(8),
            max_tool_error_rate: Some(ErrorRate(0.5)),
        };
        let effective = guards.effective(Some(&case)).unwrap();
        assert_eq!(effective.score(Some(&complete(4, 1))), BudgetStatus::Pass);
        assert_eq!(effective.score(Some(&complete(2, 1))), BudgetStatus::Fail);
        assert_eq!(effective.score(None), BudgetStatus::Unknown);
        let mut partial = complete(4, 1);
        partial.coverage = Coverage::Partial;
        assert_eq!(case.score(Some(&partial)), BudgetStatus::Pass);
        assert_eq!(effective.score(Some(&partial)), BudgetStatus::Unknown);
        // An observed rate above the maximum may fall as pending calls finish.
        partial.completed_tool_calls = Some(1);
        assert_eq!(effective.score(Some(&partial)), BudgetStatus::Unknown);
        assert_eq!(case.score(Some(&partial)), BudgetStatus::Fail);
        partial.steps = Some(9);
        assert_eq!(effective.score(Some(&partial)), BudgetStatus::Fail);
        for loss in 0..4 {
            let mut incomplete = complete(4, 1);
            match loss {
                0 => incomplete.stream_incomplete = true,
                1 => incomplete.tracking_limited = true,
                2 => incomplete.malformed_events = 1,
                _ => incomplete.unclassified_events = 1,
            }
            assert_eq!(effective.score(Some(&incomplete)), BudgetStatus::Unknown);
        }
    }

    #[test]
    fn rates_validate_through_all_input_formats() {
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.1, 1.1] {
            assert!(ErrorRate::try_from(invalid).is_err());
        }
        for invalid in ["null", "-0.1", "1.1", "\"0.5\""] {
            assert!(serde_json::from_str::<ErrorRate>(invalid).is_err());
        }
        for valid in [0.0, 0.5, 1.0] {
            let rate = ErrorRate::try_from(valid).unwrap();
            assert_eq!(serde_json::to_value(rate).unwrap(), json!(valid));
        }
    }
}

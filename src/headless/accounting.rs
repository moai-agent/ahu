//! Terminal-span observations, not a billing ledger or additive call counters.
//! Only fixed labels and bounded numeric fields cross this projection.
use super::Events;
use crate::eval::trajectory::Coverage;
#[cfg(test)]
use crate::eval::trajectory::Source;
use crate::telemetry::SpanGuard;

#[derive(Debug, PartialEq)]
enum Attribute {
    Label(String),
    Count(u64),
    Amount(f64),
    Flag(bool),
}

#[derive(Debug, Default)]
pub(super) struct Projection {
    attributes: Vec<(&'static str, Attribute)>,
    numeric_range_limited: bool,
}

impl Projection {
    pub(super) fn from_events(events: &Events) -> Self {
        let mut p = Self::default();
        // A resumed stream may repeat prior session totals; native helpers or
        // registered children may overlap the parent's reported observations.
        p.label("ahu.accounting.scope", "attempt_stream_observations");
        p.label("ahu.accounting.cross_attempt_additivity", "not_established");
        p.label("ahu.accounting.parent_child_additivity", "not_established");

        let o = &events.trajectory.observation;
        p.label("ahu.events.scope", "recognized_native_stdout_events");
        p.label("ahu.events.source", o.source.as_str());
        let complete = o.complete() && events.terminal && events.unknown_events == 0;
        p.label(
            "ahu.events.coverage",
            if complete {
                "complete_observed_stream"
            } else if o.coverage == Coverage::None {
                "unavailable"
            } else {
                "partial"
            },
        );
        p.flag("ahu.events.complete_observed_stream", complete);
        p.flag("ahu.events.terminal_observed", events.terminal);
        p.flag("ahu.events.stream_incomplete", o.stream_incomplete);
        p.flag("ahu.events.tracking_limited", o.tracking_limited);
        p.count("ahu.events.malformed", Some(o.malformed_events));
        p.count("ahu.events.unclassified", Some(o.unclassified_events));
        p.count("ahu.events.unknown", Some(events.unknown_events));

        p.label("ahu.tokens.source", "native_event_usage_fields");
        p.label("ahu.tokens.aggregation", "maximum_reported_per_field");
        p.label("ahu.tokens.scope", "attempt_stream_observations");
        let mut tokens_observed = false;
        for (key, value) in events.usage.normalized_fields() {
            tokens_observed |= p.count(key, value);
        }
        // Even complete event capture cannot establish token accounting coverage.
        p.label("ahu.tokens.coverage", coverage(tokens_observed));

        p.label(
            "ahu.cost.scope",
            "native_reported_amounts_observed_in_attempt",
        );
        p.label("ahu.cost.unit", "USD");
        p.label("ahu.cost.basis", "harness_reported_not_provider_billing");
        p.flag("ahu.cost.tracking_limited", events.cost_tracking_limited);
        let source = match events.cost.source.as_deref() {
            Some("native_cumulative_usd") => {
                Some(("native_cumulative_usd", "maximum_reported_result_total"))
            }
            Some("native_step_sum_usd") => {
                Some(("native_step_sum_usd", "sum_first_valid_cost_per_part_id"))
            }
            _ => None,
        };
        let cost_observed =
            source.is_some() && p.amount("ahu.cost.harness_reported_usd", events.cost.usd);
        p.label("ahu.cost.coverage", coverage(cost_observed));
        if cost_observed && let Some((source, aggregation)) = source {
            p.label("ahu.cost.source", source);
            p.label("ahu.cost.aggregation", aggregation);
        }

        // Metering remains independently partial; units never imply tokens or USD.
        if o.metering.is_some() {
            p.label("ahu.metering.source", o.source.as_str());
            p.label(
                "ahu.metering.scope",
                "accepted_current_attempt_observations",
            );
            p.label("ahu.metering.credits.unit", "credit");
            p.label(
                "ahu.metering.credits.aggregation",
                "sum_accepted_deduplicated_prompt_completions",
            );
            p.label("ahu.metering.elapsed.unit", "ms");
            p.label(
                "ahu.metering.elapsed.aggregation",
                "sum_accepted_deduplicated_prompt_completions",
            );
            p.label("ahu.metering.context.unit", "percent");
            p.label(
                "ahu.metering.context.aggregation",
                "last_and_maximum_observed_window_usage",
            );
            let m = o.metering.as_ref().filter(|m| m.valid());
            let credits = p.amount(
                "ahu.metering.credits.observed",
                m.and_then(|m| m.observed_credits),
            );
            let elapsed = p.amount(
                "ahu.metering.elapsed.observed_ms",
                m.and_then(|m| m.observed_elapsed_ms),
            );
            let context = p.amount(
                "ahu.metering.context.last_percent",
                m.and_then(|m| m.context_usage_percent),
            );
            p.amount(
                "ahu.metering.context.peak_percent",
                m.and_then(|m| m.peak_context_usage_percent),
            );
            let completions = p.count(
                "ahu.metering.prompt_completions",
                m.and_then(|m| m.prompt_completions),
            );
            p.label(
                "ahu.metering.coverage",
                coverage(completions || credits || elapsed || context),
            );
            p.label(
                "ahu.metering.prompt_completions.aggregation",
                "count_accepted_deduplicated_completions",
            );
            p.label("ahu.metering.credits.coverage", coverage(credits));
            p.label("ahu.metering.elapsed.coverage", coverage(elapsed));
            p.label("ahu.metering.context.coverage", coverage(context));
            if let Some(m) = m {
                for (key, value) in [
                    ("ahu.metering.credits.observations", m.credit_observations),
                    ("ahu.metering.elapsed.observations", m.elapsed_observations),
                    (
                        "ahu.metering.duplicate_completions",
                        m.duplicate_completions,
                    ),
                    ("ahu.metering.rejected_updates", m.rejected_updates),
                    (
                        "ahu.metering.unattributed_completions",
                        m.unattributed_completions,
                    ),
                ] {
                    p.count(key, Some(value));
                }
            }
        }
        p.flag(
            "ahu.accounting.numeric_range_limited",
            p.numeric_range_limited,
        );
        p
    }

    fn label(&mut self, key: &'static str, value: &str) {
        self.attributes.push((key, Attribute::Label(value.into())));
    }

    fn flag(&mut self, key: &'static str, value: bool) {
        self.attributes.push((key, Attribute::Flag(value)));
    }

    fn count(&mut self, key: &'static str, value: Option<u64>) -> bool {
        let Some(value) = value else { return false };
        // SpanGuard uses OTel's signed integer representation. Never wrap a
        // native unsigned observation into a negative exported count.
        if value > i64::MAX as u64 {
            self.numeric_range_limited = true;
            return false;
        }
        self.attributes.push((key, Attribute::Count(value)));
        true
    }

    fn amount(&mut self, key: &'static str, value: Option<f64>) -> bool {
        let Some(value) = value.filter(|v| v.is_finite() && *v >= 0.0) else {
            return false;
        };
        self.attributes.push((key, Attribute::Amount(value)));
        true
    }

    pub(super) fn export(self, span: &mut SpanGuard) {
        for (key, value) in self.attributes {
            match value {
                Attribute::Label(value) => span.set_string(key, &value),
                Attribute::Count(value) => span.set_u64(key, value),
                Attribute::Amount(value) => span.set_f64(key, value),
                Attribute::Flag(value) => span.set_bool(key, value),
            }
        }
    }
}

fn coverage(observed: bool) -> &'static str {
    if observed { "partial" } else { "unavailable" }
}

#[cfg(test)]
mod tests {
    use super::*;
    impl Projection {
        fn get(&self, key: &str) -> Option<&Attribute> {
            self.attributes
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v)
        }
    }
    #[test]
    fn normalized_usage_preserves_unknown_zero_and_signed_export_bounds() {
        let mut events = Events::default();
        let absent = Projection::from_events(&events);
        assert_eq!(absent.get("ahu.tokens.input"), None);
        events.usage.input = Some(0);
        assert_eq!(
            Projection::from_events(&events).get("ahu.tokens.input"),
            Some(&Attribute::Count(0))
        );
        events.usage.input = Some(u64::MAX);
        events.usage.output = Some(i64::MAX as u64);
        let projection = Projection::from_events(&events);
        assert_eq!(projection.get("ahu.tokens.input"), None);
        assert_eq!(
            projection.get("ahu.tokens.output"),
            Some(&Attribute::Count(i64::MAX as u64))
        );
        assert_eq!(
            projection.get("ahu.accounting.numeric_range_limited"),
            Some(&Attribute::Flag(true))
        );
    }
    #[test]
    fn normalized_complete_stream_does_not_claim_complete_token_accounting() {
        let mut events = Events {
            terminal: true,
            ..Default::default()
        };
        events.trajectory.observation.source =
            Source::try_from("fixture_turns".to_owned()).unwrap();
        events.trajectory.observation.coverage = Coverage::CompleteObservedStream;
        events.usage.input = Some(10);
        let projection = Projection::from_events(&events);
        assert_eq!(
            projection.get("ahu.events.complete_observed_stream"),
            Some(&Attribute::Flag(true))
        );
        assert_eq!(
            projection.get("ahu.tokens.coverage"),
            Some(&Attribute::Label("partial".into()))
        );
        events.trajectory.incomplete();
        assert_eq!(
            Projection::from_events(&events).get("ahu.events.complete_observed_stream"),
            Some(&Attribute::Flag(false))
        );
    }
}

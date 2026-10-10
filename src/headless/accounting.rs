//! Terminal-span observations, not a billing ledger or additive call counters.
//! Only fixed labels and bounded numeric fields cross this projection.
use super::Events;
use crate::eval::trajectory::{Coverage, Source};
use crate::telemetry::SpanGuard;

#[derive(Debug, PartialEq)]
enum Attribute {
    Label(&'static str),
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
        p.label(
            "ahu.events.source",
            match o.source {
                Source::Unsupported => "unsupported",
                Source::CodexTurns => "codex_turns",
                Source::ClaudeAssistantMessages => "claude_assistant_messages",
                Source::OpencodeSteps => "opencode_steps",
                Source::KiroSessionUpdates => "kiro_session_updates",
            },
        );
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
            Some("claude_code_result_total") => {
                Some(("claude_code_result_total", "maximum_reported_result_total"))
            }
            Some("opencode_step_finish_sum") => Some((
                "opencode_step_finish_sum",
                "sum_first_valid_cost_per_part_id",
            )),
            _ => None,
        };
        let cost_observed =
            source.is_some() && p.amount("ahu.cost.harness_reported_usd", events.cost.usd);
        p.label("ahu.cost.coverage", coverage(cost_observed));
        if cost_observed && let Some((source, aggregation)) = source {
            p.label("ahu.cost.source", source);
            p.label("ahu.cost.aggregation", aggregation);
        }

        // Kiro's accepted completions remain partial even on a successful run.
        // Replay, conflicting completions and helper evidence are already excluded
        // by the shared trajectory observer; never reparse raw events here.
        if o.source == Source::KiroSessionUpdates {
            p.label("ahu.kiro.source", "kiro_session_info_updates");
            p.label("ahu.kiro.scope", "accepted_current_attempt_observations");
            p.label("ahu.kiro.credits.unit", "credit");
            p.label(
                "ahu.kiro.credits.aggregation",
                "sum_accepted_deduplicated_prompt_completions",
            );
            p.label("ahu.kiro.elapsed.unit", "ms");
            p.label(
                "ahu.kiro.elapsed.aggregation",
                "sum_accepted_deduplicated_prompt_completions",
            );
            p.label("ahu.kiro.context.unit", "percent");
            p.label(
                "ahu.kiro.context.aggregation",
                "last_and_maximum_observed_window_usage",
            );
            let m = o.kiro_metering.as_ref().filter(|m| m.valid());
            let credits = p.amount(
                "ahu.kiro.credits.observed",
                m.and_then(|m| m.observed_credits),
            );
            let elapsed = p.amount(
                "ahu.kiro.elapsed.observed_ms",
                m.and_then(|m| m.observed_elapsed_ms),
            );
            let context = p.amount(
                "ahu.kiro.context.last_percent",
                m.and_then(|m| m.context_usage_percent),
            );
            p.amount(
                "ahu.kiro.context.peak_percent",
                m.and_then(|m| m.peak_context_usage_percent),
            );
            let completions = p.count(
                "ahu.kiro.prompt_completions",
                m.and_then(|m| m.prompt_completions),
            );
            p.label(
                "ahu.kiro.coverage",
                coverage(completions || credits || elapsed || context),
            );
            p.label(
                "ahu.kiro.prompt_completions.aggregation",
                "count_accepted_deduplicated_completions",
            );
            p.label("ahu.kiro.credits.coverage", coverage(credits));
            p.label("ahu.kiro.elapsed.coverage", coverage(elapsed));
            p.label("ahu.kiro.context.coverage", coverage(context));
            if let Some(m) = m {
                for (key, value) in [
                    ("ahu.kiro.credits.observations", m.credit_observations),
                    ("ahu.kiro.elapsed.observations", m.elapsed_observations),
                    ("ahu.kiro.duplicate_completions", m.duplicate_completions),
                    ("ahu.kiro.rejected_updates", m.rejected_updates),
                    (
                        "ahu.kiro.unattributed_completions",
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

    fn label(&mut self, key: &'static str, value: &'static str) {
        self.attributes.push((key, Attribute::Label(value)));
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
                Attribute::Label(value) => span.set_string(key, value),
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
    use serde_json::{Value, json};

    impl Projection {
        fn get(&self, key: &str) -> Option<&Attribute> {
            self.attributes
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v)
        }
    }

    fn observe(events: &mut Events, harness: &str, event: Value) {
        events.observe(harness, &serde_json::to_vec(&event).unwrap());
    }

    #[test]
    fn claude_cumulative_results_are_idempotent_and_keep_maximum_valid_observation() {
        let mut events = Events::default();
        observe(
            &mut events,
            "claude-code",
            json!({"type":"result","total_cost_usd":0}),
        );
        assert_eq!(
            Projection::from_events(&events).get("ahu.cost.harness_reported_usd"),
            Some(&Attribute::Amount(0.0))
        );
        for amount in [
            json!(0.125),
            json!(0.125),
            json!(0.0625),
            json!(-1),
            Value::Null,
            json!("0.5"),
            json!(0.25),
        ] {
            observe(
                &mut events,
                "claude-code",
                json!({"type":"result","total_cost_usd":amount}),
            );
        }
        observe(
            &mut events,
            "claude-code",
            json!({"type":"assistant","total_cost_usd":99}),
        );
        assert_eq!(events.cost.usd, Some(0.25));
        let p = Projection::from_events(&events);
        assert_eq!(
            p.get("ahu.cost.harness_reported_usd"),
            Some(&Attribute::Amount(0.25))
        );
        assert_eq!(
            p.get("ahu.cost.source"),
            Some(&Attribute::Label("claude_code_result_total"))
        );
        assert_eq!(
            p.get("ahu.cost.aggregation"),
            Some(&Attribute::Label("maximum_reported_result_total"))
        );
        assert_eq!(
            p.get("ahu.cost.coverage"),
            Some(&Attribute::Label("partial"))
        );
        assert_eq!(
            p.get("ahu.accounting.cross_attempt_additivity"),
            Some(&Attribute::Label("not_established"))
        );
        assert_eq!(
            p.get("ahu.accounting.parent_child_additivity"),
            Some(&Attribute::Label("not_established"))
        );
    }

    #[test]
    fn opencode_accepts_late_valid_cost_and_deduplicates_only_accepted_parts() {
        let mut events = Events::default();
        for amount in [
            Value::Null,
            json!(-1),
            json!("private-payload"),
            json!(0.25),
            json!(0.25),
            json!(99),
        ] {
            observe(
                &mut events,
                "opencode",
                json!({"type":"step_finish","part":{"id":"private-part","cost":amount}}),
            );
        }
        observe(
            &mut events,
            "opencode",
            json!({"type":"step_finish","part":{"id":"second","cost":0.125}}),
        );
        assert_eq!(events.cost.usd, Some(0.375));
        assert_eq!(events.cost_step_ids.len(), 2);
        let p = Projection::from_events(&events);
        assert_eq!(
            p.get("ahu.cost.aggregation"),
            Some(&Attribute::Label("sum_first_valid_cost_per_part_id"))
        );
        assert!(!format!("{p:?}").contains("private-"));
    }

    #[test]
    fn opencode_cost_tracking_and_arithmetic_are_bounded() {
        let mut events = Events::default();
        for id in [String::new(), "x".repeat(1025)] {
            observe(
                &mut events,
                "opencode",
                json!({"type":"step_finish","part":{"id":id,"cost":1}}),
            );
        }
        assert!(events.cost_step_ids.is_empty());
        assert!(events.cost_tracking_limited);
        assert_eq!(events.cost.usd, None);
        for i in 0..4097 {
            observe(
                &mut events,
                "opencode",
                json!({"type":"step_finish","part":{"id":format!("part-{i}"),"cost":0}}),
            );
        }
        assert_eq!(events.cost_step_ids.len(), 4096);
        assert_eq!(events.cost.usd, Some(0.0));
        assert_eq!(
            Projection::from_events(&events).get("ahu.cost.tracking_limited"),
            Some(&Attribute::Flag(true))
        );

        let mut overflow = Events::default();
        for id in ["first", "second"] {
            observe(
                &mut overflow,
                "opencode",
                json!({"type":"step_finish","part":{"id":id,"cost":1e308}}),
            );
        }
        assert_eq!(overflow.cost.usd, Some(1e308));
        assert_eq!(overflow.cost_step_ids.len(), 1);
        assert!(overflow.cost_tracking_limited);
        observe(
            &mut overflow,
            "opencode",
            json!({"type":"step_finish","part":{"id":"second","cost":0}}),
        );
        assert_eq!(overflow.cost_step_ids.len(), 2);
    }

    #[test]
    fn tokens_remain_per_field_maxima_and_missing_is_not_zero() {
        let mut events = Events::default();
        for usage in [
            json!({"input_tokens":12,"output_tokens":4,"cached_tokens":0}),
            json!({"input_tokens":3,"output_tokens":7}),
        ] {
            observe(
                &mut events,
                "codex",
                json!({"type":"turn.completed","usage":usage}),
            );
        }
        let p = Projection::from_events(&events);
        assert_eq!(p.get("ahu.tokens.input"), Some(&Attribute::Count(12)));
        assert_eq!(p.get("ahu.tokens.output"), Some(&Attribute::Count(7)));
        assert_eq!(p.get("ahu.tokens.cached"), Some(&Attribute::Count(0)));
        assert_eq!(p.get("ahu.tokens.total"), None);
        assert_eq!(
            p.get("ahu.tokens.aggregation"),
            Some(&Attribute::Label("maximum_reported_per_field"))
        );
        assert_eq!(
            p.get("ahu.tokens.coverage"),
            Some(&Attribute::Label("partial"))
        );
        assert_eq!(p.get("ahu.cost.harness_reported_usd"), None);
        assert_eq!(
            p.get("ahu.cost.coverage"),
            Some(&Attribute::Label("unavailable"))
        );
        assert!(
            !p.attributes
                .iter()
                .any(|(key, _)| key.starts_with("gen_ai."))
        );

        let empty = Projection::from_events(&Events::default());
        assert_eq!(empty.get("ahu.tokens.input"), None);
        assert_eq!(
            empty.get("ahu.tokens.coverage"),
            Some(&Attribute::Label("unavailable"))
        );
    }

    #[test]
    fn complete_event_stream_does_not_certify_accounting_coverage() {
        let mut events = Events::default();
        observe(
            &mut events,
            "codex",
            json!({"type":"thread.started","thread_id":"synthetic"}),
        );
        observe(
            &mut events,
            "codex",
            json!({"type":"turn.completed","usage":{"input_tokens":0},"total_cost_usd":99}),
        );
        let p = Projection::from_events(&events);
        assert_eq!(
            p.get("ahu.events.complete_observed_stream"),
            Some(&Attribute::Flag(true))
        );
        assert_eq!(
            p.get("ahu.tokens.coverage"),
            Some(&Attribute::Label("partial"))
        );
        assert_eq!(p.get("ahu.cost.harness_reported_usd"), None);
        events.trajectory.incomplete();
        let p = Projection::from_events(&events);
        assert_eq!(
            p.get("ahu.events.complete_observed_stream"),
            Some(&Attribute::Flag(false))
        );
        assert_eq!(
            p.get("ahu.events.stream_incomplete"),
            Some(&Attribute::Flag(true))
        );
        assert_eq!(p.get("ahu.tokens.input"), Some(&Attribute::Count(0)));
    }

    fn kiro_start() -> Events {
        let mut events = Events::default();
        observe(
            &mut events,
            "kiro",
            json!({"type":"runStarted","data":{"engine":"v3","payloadSchema":"acp"}}),
        );
        events
    }

    fn kiro_info(events: &mut Events, meta: Value) {
        observe(
            events,
            "kiro",
            json!({"type":"sessionUpdate","data":{"sessionId":"sess_synthetic",
            "update":{"sessionUpdate":"session_info_update","_meta":{"kiro":meta}}}}),
        );
    }

    fn kiro_prompt(events: &mut Events) {
        kiro_info(
            events,
            json!({"kind":"user_message_id_assigned","userMessageId":"private-prompt"}),
        );
        kiro_info(
            events,
            json!({"kind":"turn_start","messageId":"private-turn"}),
        );
    }

    #[test]
    fn kiro_projects_only_accepted_deduplicated_metering_in_native_units() {
        let mut events = kiro_start();
        kiro_prompt(&mut events);
        let completion = json!({"kind":"turn_completion","status":"success","elapsedTime":0,
            "requestIds":["private-request"],"promptTurnSummaries":[{"unit":"credit","usage":0,"usedTools":["private-tool"]}]});
        kiro_info(&mut events, completion.clone());
        kiro_info(&mut events, completion);
        kiro_info(
            &mut events,
            json!({"kind":"turn_completion","status":"success","promptTurnSummaries":[{"unit":"USD","usage":999}]}),
        );
        kiro_info(
            &mut events,
            json!({"kind":"context_usage","usagePercentage":12.5,"breakdown":{"private-file":999}}),
        );
        kiro_info(
            &mut events,
            json!({"kind":"context_usage","usagePercentage":0}),
        );
        let p = Projection::from_events(&events);
        assert_eq!(
            p.get("ahu.kiro.credits.observed"),
            Some(&Attribute::Amount(0.0))
        );
        assert_eq!(
            p.get("ahu.kiro.elapsed.observed_ms"),
            Some(&Attribute::Amount(0.0))
        );
        assert_eq!(
            p.get("ahu.kiro.credits.observations"),
            Some(&Attribute::Count(1))
        );
        assert_eq!(
            p.get("ahu.kiro.prompt_completions"),
            Some(&Attribute::Count(1))
        );
        assert_eq!(
            p.get("ahu.kiro.duplicate_completions"),
            Some(&Attribute::Count(1))
        );
        assert_eq!(
            p.get("ahu.kiro.rejected_updates"),
            Some(&Attribute::Count(1))
        );
        assert_eq!(
            p.get("ahu.kiro.context.last_percent"),
            Some(&Attribute::Amount(0.0))
        );
        assert_eq!(
            p.get("ahu.kiro.context.peak_percent"),
            Some(&Attribute::Amount(12.5))
        );
        assert_eq!(
            p.get("ahu.kiro.credits.unit"),
            Some(&Attribute::Label("credit"))
        );
        assert_eq!(
            p.get("ahu.kiro.credits.coverage"),
            Some(&Attribute::Label("partial"))
        );
        assert_eq!(p.get("ahu.tokens.input"), None);
        assert_eq!(p.get("ahu.cost.harness_reported_usd"), None);
        assert!(!format!("{p:?}").contains("private-"));
        assert!(p.attributes.len() < 80);
    }

    #[test]
    fn kiro_absent_credit_and_replayed_history_are_never_reported_as_zero() {
        let mut events = kiro_start();
        kiro_info(
            &mut events,
            json!({"kind":"turn_completion","replay":true,"status":"success",
            "promptTurnSummaries":[{"unit":"credit","usage":3}]}),
        );
        let p = Projection::from_events(&events);
        assert_eq!(p.get("ahu.kiro.credits.observed"), None);
        assert_eq!(p.get("ahu.kiro.credits.observations"), None);
        assert_eq!(
            p.get("ahu.kiro.credits.coverage"),
            Some(&Attribute::Label("unavailable"))
        );
        kiro_prompt(&mut events);
        kiro_info(
            &mut events,
            json!({"kind":"turn_completion","status":"aborted"}),
        );
        let p = Projection::from_events(&events);
        assert_eq!(
            p.get("ahu.kiro.prompt_completions"),
            Some(&Attribute::Count(1))
        );
        assert_eq!(
            p.get("ahu.kiro.credits.observations"),
            Some(&Attribute::Count(0))
        );
        assert_eq!(p.get("ahu.kiro.credits.observed"), None);
        assert_eq!(p.get("ahu.kiro.elapsed.observed_ms"), None);
    }

    #[test]
    fn unsigned_counts_outside_otel_range_are_omitted_and_flagged() {
        let mut events = Events::default();
        observe(
            &mut events,
            "codex",
            json!({"type":"turn.completed","usage":{"input_tokens":u64::MAX,"output_tokens":i64::MAX}}),
        );
        let p = Projection::from_events(&events);
        assert_eq!(p.get("ahu.tokens.input"), None);
        assert_eq!(
            p.get("ahu.tokens.output"),
            Some(&Attribute::Count(i64::MAX as u64))
        );
        assert_eq!(
            p.get("ahu.accounting.numeric_range_limited"),
            Some(&Attribute::Flag(true))
        );
    }
}

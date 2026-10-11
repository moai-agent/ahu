//! Bounded native model evidence. A first value cannot establish session identity.
use serde_json::Value;

#[derive(Debug, Default)]
pub(super) struct ModelEvidence {
    first: Option<String>,
    conflicting: bool,
    redacted: bool,
    missing: bool,
}

impl ModelEvidence {
    pub(super) fn observe<'a>(&mut self, values: impl Iterator<Item = &'a Value>) {
        let mut found = false;
        for value in values {
            found = true;
            let Some(value) = value
                .as_str()
                .filter(|v| crate::eval::provenance::safe_identity(v))
            else {
                self.redacted = true;
                continue;
            };
            if let Some(first) = &self.first {
                self.conflicting |= first != value;
            } else {
                self.first = Some(value.to_owned());
            }
        }
        self.missing |= !found;
    }

    fn attributes(&self, complete_stream: bool) -> Vec<(&'static str, String)> {
        let coverage = if self.conflicting {
            "conflicting"
        } else if self.redacted {
            "redacted"
        } else if self.missing || !complete_stream {
            "partial"
        } else {
            "complete"
        };
        let mut attributes = vec![("ahu.harness.model.reported.coverage", coverage.into())];
        if !self.conflicting
            && !self.redacted
            && let Some(model) = &self.first
        {
            attributes.push(("ahu.harness.model.reported", model.clone()));
        }
        attributes
    }
}

impl super::Events {
    pub(crate) fn reported_model_attributes(&self) -> Vec<(&'static str, String)> {
        self.model_evidence.attributes(
            self.terminal && self.unknown_events == 0 && self.trajectory.observation.complete(),
        )
    }
}

/// These are launch requests, not reported provider or model observations.
pub(crate) fn requested_attributes(harness: &str, model: &str) -> [(&'static str, String); 2] {
    [
        ("ahu.harness.backend", harness.into()),
        ("ahu.harness.model.requested", model.into()),
    ]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    fn coverage(events: &super::super::Events) -> String {
        events
            .reported_model_attributes()
            .into_iter()
            .find(|(key, _)| *key == "ahu.harness.model.reported.coverage")
            .unwrap()
            .1
    }

    #[test]
    fn gaps_invalid_aliases_and_truncated_streams_cannot_restore_single_identity() {
        let mut events = super::super::Events::default();
        events.observe("opencode", br#"{"type":"step_start","model":"fixture"}"#);
        assert_eq!(coverage(&events), "partial"); // No terminal observation.
        events.observe("opencode", b"{");
        events.observe(
            "opencode",
            br#"{"type":"step_finish","model":"fixture","part":{"id":"end","reason":"stop"}}"#,
        );
        assert_eq!(coverage(&events), "partial"); // Later success cannot erase loss.

        for (alias, expected) in [(json!("different"), "conflicting"), (json!(42), "redacted")] {
            let mut events = super::super::Events::default();
            events.observe(
                "opencode",
                &serde_json::to_vec(&json!({
                    "type":"step_finish", "model":"fixture", "model_name":alias,
                    "part":{"id":"end","reason":"stop"}
                }))
                .unwrap(),
            );
            assert_eq!(coverage(&events), expected);
            assert!(
                events
                    .reported_model_attributes()
                    .iter()
                    .all(|(k, _)| *k != "ahu.harness.model.reported")
            );
        }
    }

    #[test]
    fn kiro_model_configuration_is_not_reported_inference_identity() {
        let mut events = super::super::Events::default();
        events.observe("kiro", br#"{"type":"runStarted","data":{"engine":"v3","payloadSchema":"acp","acpProtocolVersion":1}}"#);
        events.observe(
            "kiro",
            &serde_json::to_vec(&json!({
                "type":"sessionUpdate", "data":{"sessionId":"sess_fixture","update":{"sessionUpdate":"config_option_update",
                    "configOptions":[{"id":"model","currentValue":"fixture"}]}}
            }))
            .unwrap(),
        );
        assert_eq!(events.model.as_deref(), Some("fixture"));
        assert!(
            events
                .reported_model_attributes()
                .iter()
                .all(|(k, _)| *k != "ahu.harness.model.reported")
        );
        assert_eq!(coverage(&events), "partial");
    }
}

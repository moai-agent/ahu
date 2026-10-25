//! Generic normalized model labels. Provider evidence is decoded externally.
impl super::Events {
    pub(crate) fn reported_model_attributes(&self) -> Vec<(&'static str, String)> {
        let mut result = Vec::new();
        for key in [
            "ahu.harness.model.reported.coverage",
            "ahu.harness.model.reported",
        ] {
            if let Some(value) = self.model_attributes.get(key) {
                result.push((key, value.clone()));
            }
        }
        result
    }
}
/// These are launch requests, not reported provider or model observations.
pub(crate) fn requested_attributes(harness: &str, model: &str) -> [(&'static str, String); 2] {
    [
        ("ahu.harness.backend", harness.into()),
        ("ahu.harness.model.requested", model.into()),
    ]
}

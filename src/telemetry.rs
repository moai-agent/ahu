//! Opt-in OpenTelemetry setup for ahu-launched work.
//!
//! Configuration is read from the repository policy and applied only to
//! processes ahu starts. The invoking shell and unrelated harness sessions
//! never receive these variables.

use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use opentelemetry::global;
use opentelemetry::trace::{Span, Tracer};
use opentelemetry::{KeyValue, Value};
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig};
use opentelemetry_sdk::{Resource, trace::SdkTracerProvider};

use crate::config::TelemetryConfig;
use crate::util::{Error, Result};

static PROVIDER: OnceLock<Mutex<Option<SdkTracerProvider>>> = OnceLock::new();

fn provider_slot() -> &'static Mutex<Option<SdkTracerProvider>> {
    PROVIDER.get_or_init(|| Mutex::new(None))
}

pub fn validate_config(config: &TelemetryConfig, path: &Path) -> Result<()> {
    let endpoint = config.endpoint.parse::<url::Url>().map_err(|error| {
        Error::new(format!(
            "{}: telemetry.endpoint is not a valid URL: {error}",
            path.display()
        ))
    })?;
    if endpoint.scheme() != "http" {
        return Err(Error::new(format!(
            "{}: telemetry.endpoint must use http:// for the local collector",
            path.display()
        )));
    }
    let local = matches!(
        endpoint.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    if !local {
        return Err(Error::new(format!(
            "{}: telemetry.endpoint must point to localhost, 127.0.0.1, or ::1",
            path.display()
        )));
    }
    if endpoint.port_or_known_default() != Some(4318) {
        return Err(Error::new(format!(
            "{}: telemetry.endpoint must use the OTLP/HTTP port 4318",
            path.display()
        )));
    }
    Ok(())
}

/// Enable ahu's own spans when project telemetry is enabled.
pub fn initialize(config: &TelemetryConfig) -> Result<()> {
    if !config.enabled
        || provider_slot()
            .lock()
            .expect("telemetry mutex poisoned")
            .is_some()
    {
        return Ok(());
    }
    let exporter = SpanExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(format!(
            "{}/v1/traces",
            config.endpoint.trim_end_matches('/')
        ))
        .with_timeout(Duration::from_millis(500))
        .build();
    let exporter = match exporter {
        Ok(exporter) => exporter,
        Err(_) => {
            // Exporter diagnostics can contain environment-derived credentials.
            // Telemetry setup must never prevent the assignment from running.
            eprintln!("ahu: OTLP exporter unavailable; continuing without export");
            return Ok(());
        }
    };
    let resource = Resource::builder()
        .with_service_name("ahu")
        .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
        .build();
    let provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(exporter)
        .build();
    global::set_tracer_provider(provider.clone());
    *provider_slot().lock().expect("telemetry mutex poisoned") = Some(provider);
    Ok(())
}

/// Apply project telemetry only to a child process ahu is about to start.
#[allow(clippy::too_many_arguments)]
pub fn configure_child(
    command: &mut std::process::Command,
    config: &TelemetryConfig,
    agent: &str,
    harness: &str,
    model: &str,
    agent_version: Option<&str>,
    harness_version: Option<&str>,
    task_id: Option<&str>,
) {
    if !config.enabled {
        return;
    }
    command
        .env_remove("OTEL_EXPORTER_OTLP_HEADERS")
        .env_remove("OTEL_EXPORTER_OTLP_TRACES_HEADERS")
        .env_remove("OTEL_EXPORTER_OTLP_METRICS_HEADERS")
        .env_remove("OTEL_EXPORTER_OTLP_LOGS_HEADERS")
        .env_remove("OTEL_EXPORTER_OTLP_CERTIFICATE")
        .env_remove("OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE")
        .env_remove("OTEL_EXPORTER_OTLP_CLIENT_KEY")
        .env_remove("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
        .env_remove("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT")
        .env_remove("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT")
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", &config.endpoint)
        .env("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf")
        .env("OTEL_EXPORTER_OTLP_TIMEOUT", "500")
        .env("OTEL_TRACES_EXPORTER", "otlp")
        .env("OTEL_METRICS_EXPORTER", "none")
        .env("OTEL_LOGS_EXPORTER", "none")
        .env("OTEL_SERVICE_NAME", "ahu-agent")
        .env(
            "OTEL_RESOURCE_ATTRIBUTES",
            resource_attributes(
                agent,
                harness,
                model,
                agent_version,
                harness_version,
                task_id,
                std::env::var("OTEL_RESOURCE_ATTRIBUTES").ok().as_deref(),
            ),
        );
}

fn resource_attributes(
    agent: &str,
    harness: &str,
    model: &str,
    agent_version: Option<&str>,
    harness_version: Option<&str>,
    task_id: Option<&str>,
    inherited: Option<&str>,
) -> String {
    let mut values = vec![
        format!("ahu.agent.name={}", escape(agent)),
        format!("ahu.harness={}", escape(harness)),
        format!("ahu.model={}", escape(model)),
        format!("ahu.version={}", escape(env!("CARGO_PKG_VERSION"))),
    ];
    if let Some(version) = agent_version {
        values.push(format!("ahu.agent.version={}", escape(version)));
    }
    if let Some(version) = harness_version {
        values.push(format!("ahu.harness.version={}", escape(version)));
    }
    if let Some(task_id) = task_id {
        values.push(format!("ahu.task.id={}", escape(task_id)));
    }
    if let Some(inherited) = inherited.filter(|value| !value.is_empty()) {
        values.push(inherited.to_string());
    }
    values.join(",")
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(',', "\\,")
        .replace('=', "\\=")
}

/// Local numeric projection, never passed to an OTEL exporter or child env.
/// Correlation uses the containing result's existing task ID and attempt.
/// No task record, prompt, identity string, or tracker reference is accepted.
#[derive(Debug, serde::Serialize)]
pub(crate) struct LocalMetrics {
    schema_version: u32,
    token_aggregation: &'static str,
    values: std::collections::BTreeMap<&'static str, Measurement>,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum Measurement {
    Observed(u64),
    Unavailable,
    // Estimates need an explicit method and provenance before being produced.
    // Schema v1 emits no estimates and never derives a missing total.
}

pub(crate) fn local_metrics(
    config: &TelemetryConfig,
    usage: &crate::headless::TokenUsage,
) -> Option<LocalMetrics> {
    config.local_metrics.then(|| LocalMetrics {
        schema_version: 1,
        // The normalizer retains maxima across reports. These are observations,
        // not additive task totals or provider billing measurements.
        token_aggregation: "maximum-reported-per-field",
        values: usage
            .normalized_fields()
            .into_iter()
            .map(|(key, value)| {
                (
                    key,
                    value.map_or(Measurement::Unavailable, Measurement::Observed),
                )
            })
            .collect(),
    })
}

pub struct SpanGuard {
    span: Option<global::BoxedSpan>,
    started: Instant,
}

impl SpanGuard {
    pub fn set_string(&mut self, key: &'static str, value: impl Into<String>) {
        if let Some(span) = self.span.as_mut() {
            span.set_attribute(KeyValue::new(key, Value::from(value.into())));
        }
    }

    pub fn set_u64(&mut self, key: &'static str, value: u64) {
        if let Some(span) = self.span.as_mut() {
            span.set_attribute(KeyValue::new(key, value as i64));
        }
    }
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        if let Some(span) = self.span.as_mut() {
            span.set_attribute(KeyValue::new(
                "ahu.duration_ms",
                self.started.elapsed().as_millis() as i64,
            ));
            span.end();
        }
    }
}

pub fn span(
    name: &'static str,
    attributes: impl IntoIterator<Item = (&'static str, String)>,
) -> SpanGuard {
    let tracer = global::tracer("ahu");
    let mut span = tracer.start(name);
    for (key, value) in attributes {
        span.set_attribute(KeyValue::new(key, Value::from(value)));
    }
    SpanGuard {
        span: Some(span),
        started: Instant::now(),
    }
}

pub fn shutdown() {
    if let Some(provider) = provider_slot()
        .lock()
        .expect("telemetry mutex poisoned")
        .take()
    {
        let _ = provider.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::{configure_child, validate_config};
    use crate::config::TelemetryConfig;
    use std::path::Path;

    #[test]
    fn local_metrics_are_opt_in_numeric_and_do_not_infer_totals() {
        let mut events = crate::headless::Events::default();
        events.observe("codex", br#"{"type":"turn.completed","usage":{"input_tokens":0,"output_tokens":7,"cached_tokens":"private-marker","total_tokens":-1,"tracker_url":"private-marker"},"prompt":"private-marker","model":"private-marker"}"#);
        let mut config = TelemetryConfig::default();
        assert!(super::local_metrics(&config, &events.usage).is_none());
        config.local_metrics = true;
        let value = serde_json::to_value(super::local_metrics(&config, &events.usage)).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "schema_version": 1,
                "token_aggregation": "maximum-reported-per-field",
                "values": {
                    "ahu.tokens.input": {"kind":"observed", "value":0},
                    "ahu.tokens.output": {"kind":"observed", "value":7},
                    "ahu.tokens.cached": {"kind":"unavailable"},
                    "ahu.tokens.cache_write": {"kind":"unavailable"},
                    "ahu.tokens.reasoning": {"kind":"unavailable"},
                    "ahu.tokens.total": {"kind":"unavailable"}
                }
            })
        );
        let mut child = std::process::Command::new("true");
        configure_child(
            &mut child, &config, "agent", "codex", "model", None, None, None,
        );
        assert_eq!(child.get_envs().count(), 0);
    }

    #[test]
    fn local_endpoint_is_accepted_and_remote_is_rejected() {
        let config = TelemetryConfig::default();
        validate_config(&config, Path::new("config.toml")).unwrap();
        let remote = TelemetryConfig {
            enabled: true,
            endpoint: "https://collector.example.test:443".to_string(),
            ..TelemetryConfig::default()
        };
        assert!(validate_config(&remote, Path::new("config.toml")).is_err());
    }

    #[test]
    fn child_environment_is_only_changed_when_enabled() {
        let mut disabled = std::process::Command::new("true");
        configure_child(
            &mut disabled,
            &TelemetryConfig::default(),
            "agent",
            "codex",
            "model",
            None,
            None,
            Some("task"),
        );
        assert!(
            disabled
                .get_envs()
                .all(|(key, _)| key != "OTEL_SERVICE_NAME")
        );

        let enabled = TelemetryConfig {
            enabled: true,
            ..TelemetryConfig::default()
        };
        let mut command = std::process::Command::new("true");
        configure_child(
            &mut command,
            &enabled,
            "agent",
            "codex",
            "model",
            None,
            None,
            Some("task"),
        );
        let env: std::collections::BTreeMap<_, _> = command
            .get_envs()
            .filter_map(|(key, value)| Some((key.to_str()?, value?.to_str()?)))
            .collect();
        assert_eq!(env["OTEL_EXPORTER_OTLP_ENDPOINT"], "http://127.0.0.1:4318");
        assert!(env["OTEL_RESOURCE_ATTRIBUTES"].contains("ahu.harness=codex"));
    }
}

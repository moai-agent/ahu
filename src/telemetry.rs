//! Opt-in OpenTelemetry setup for ahu-launched work.
//!
//! Configuration is read from the repository policy and applied only to
//! processes ahu starts. The invoking shell and unrelated harness sessions
//! never receive these variables.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use opentelemetry::global;
use opentelemetry::trace::{Span, Tracer};
use opentelemetry::{KeyValue, Value};
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig};
use opentelemetry_sdk::{
    Resource,
    trace::{BatchConfigBuilder, BatchSpanProcessor, SdkTracerProvider},
};

use crate::config::TelemetryConfig;
use crate::util::{Error, Result};

pub mod private;
pub mod private_store;

static PROVIDER: OnceLock<Mutex<Option<SdkTracerProvider>>> = OnceLock::new();
static MCP_REQUESTS: AtomicU64 = AtomicU64::new(0);
static MCP_TOOL_CALLS: AtomicU64 = AtomicU64::new(0);
static MCP_DECISION_CALLS: AtomicU64 = AtomicU64::new(0);
static MCP_TOOL_ERRORS: AtomicU64 = AtomicU64::new(0);
static MCP_LIST_REQUESTS: AtomicU64 = AtomicU64::new(0);
static MCP_TRANSPORT_ERRORS: AtomicU64 = AtomicU64::new(0);

const SPAN_QUEUE_CAPACITY: usize = 128;
const SPAN_EXPORT_BATCH_SIZE: usize = 32;
const SPAN_SCHEDULED_DELAY: Duration = Duration::from_millis(500);

fn provider_slot() -> &'static Mutex<Option<SdkTracerProvider>> {
    PROVIDER.get_or_init(|| Mutex::new(None))
}

fn bounded_span_processor(
    exporter: impl opentelemetry_sdk::trace::SpanExporter + 'static,
) -> BatchSpanProcessor {
    BatchSpanProcessor::builder(exporter)
        .with_batch_config(
            BatchConfigBuilder::default()
                .with_max_queue_size(SPAN_QUEUE_CAPACITY)
                .with_max_export_batch_size(SPAN_EXPORT_BATCH_SIZE)
                .with_scheduled_delay(SPAN_SCHEDULED_DELAY)
                .build(),
        )
        .build()
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
    initialize_named(config, "ahu")
}

/// Initialize the independently launched stdio MCP server as its own service.
pub(crate) fn initialize_mcp(config: &TelemetryConfig) -> Result<()> {
    initialize_named(config, "ahu-mcp")
}

fn initialize_named(config: &TelemetryConfig, service_name: &'static str) -> Result<()> {
    let endpoint = eval_endpoint_override().unwrap_or_else(|| config.endpoint.clone());
    let enabled = config.enabled || std::env::var_os("AHU_EVAL_OTEL_ENDPOINT").is_some();
    if !enabled
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
        .with_endpoint(format!("{}/v1/traces", endpoint.trim_end_matches('/')))
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
    let mut resource = Resource::builder()
        .with_service_name(service_name)
        .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")));
    // Some harnesses remove OTEL_* from MCP child environments. Carry only
    // ahu's bounded identity fields through a separate, non-secret channel.
    let attributes = if service_name == "ahu-mcp" {
        std::env::var("AHU_MCP_RESOURCE_ATTRIBUTES")
            .ok()
            .map(|raw| parse_ahu_resource_attributes(&raw))
            .unwrap_or_else(ahu_resource_attributes)
    } else {
        ahu_resource_attributes()
    };
    for (key, value) in attributes {
        resource = resource.with_attribute(KeyValue::new(key, value));
    }
    let resource = resource.build();
    // Keep export asynchronous and bounded, and override OTEL_BSP_* values so
    // an inherited environment cannot turn a local ahu process into an
    // unbounded queue or stall task execution. Full queues drop telemetry.
    let processor = bounded_span_processor(exporter);
    let provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_span_processor(processor)
        .build();
    global::set_tracer_provider(provider.clone());
    *provider_slot().lock().expect("telemetry mutex poisoned") = Some(provider);
    Ok(())
}

/// Read only ahu's bounded identity attributes from the launch environment.
/// Other inherited OTEL attributes may contain private values and are excluded.
fn ahu_resource_attributes() -> Vec<(&'static str, String)> {
    std::env::var("OTEL_RESOURCE_ATTRIBUTES")
        .ok()
        .map(|raw| parse_ahu_resource_attributes(&raw))
        .unwrap_or_default()
}

fn parse_ahu_resource_attributes(raw: &str) -> Vec<(&'static str, String)> {
    const ALLOWED: &[&str] = &[
        "ahu.agent.name",
        "ahu.agent.version",
        "ahu.eval.case_id",
        "ahu.eval.corpus_version",
        "ahu.eval.run_id",
        "ahu.eval.stage",
        "ahu.harness",
        "ahu.harness.version",
        "ahu.model",
        "ahu.task.id",
        "ahu.task.attempt",
        "ahu.version",
    ];
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut pair = String::new();
    let mut escaped = false;
    let mut pairs = Vec::new();
    for ch in raw.chars() {
        if escaped {
            pair.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == ',' {
            pairs.push(std::mem::take(&mut pair));
        } else {
            pair.push(ch);
        }
    }
    if escaped {
        pair.push('\\');
    }
    pairs.push(pair);
    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        if ALLOWED.contains(&key) && safe_resource_value(key, value) && seen.insert(key.to_string())
        {
            out.push((
                ALLOWED
                    .iter()
                    .copied()
                    .find(|allowed| *allowed == key)
                    .unwrap(),
                value.to_string(),
            ));
        }
    }
    out
}

fn safe_resource_value(key: &str, value: &str) -> bool {
    if value.is_empty() || value.len() > 128 || !value.is_ascii() {
        return false;
    }
    if key == "ahu.task.attempt" {
        return value.bytes().all(|byte| byte.is_ascii_digit());
    }
    if key.starts_with("ahu.eval.") {
        return safe_eval_identifier(value);
    }
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"._:/-".contains(&byte))
}

fn safe_eval_identifier(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
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
    attempt: Option<u32>,
) {
    let Some(endpoint) =
        eval_endpoint_override().or_else(|| config.enabled.then(|| config.endpoint.clone()))
    else {
        return;
    };
    let attributes = resource_attributes(
        agent,
        harness,
        model,
        agent_version,
        harness_version,
        task_id,
        attempt,
        std::env::var("OTEL_RESOURCE_ATTRIBUTES").ok().as_deref(),
    );
    let mcp_attributes = parse_ahu_resource_attributes(&attributes)
        .into_iter()
        .map(|(key, value)| format!("{key}={}", escape(&value)))
        .collect::<Vec<_>>()
        .join(",");
    command
        .env("AHU_MCP_RESOURCE_ATTRIBUTES", mcp_attributes)
        .env_remove("AHU_EVAL_LOCAL_METRICS")
        .env_remove("OTEL_EXPORTER_OTLP_HEADERS")
        .env_remove("OTEL_EXPORTER_OTLP_TRACES_HEADERS")
        .env_remove("OTEL_EXPORTER_OTLP_METRICS_HEADERS")
        .env_remove("OTEL_EXPORTER_OTLP_LOGS_HEADERS")
        .env_remove("OTEL_EXPORTER_OTLP_CERTIFICATE")
        .env_remove("OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE")
        .env_remove("OTEL_EXPORTER_OTLP_CLIENT_KEY")
        .env_remove("OTEL_EXPORTER_OTLP_COMPRESSION")
        .env_remove("OTEL_EXPORTER_OTLP_TRACES_COMPRESSION")
        .env_remove("OTEL_EXPORTER_OTLP_METRICS_COMPRESSION")
        .env_remove("OTEL_EXPORTER_OTLP_LOGS_COMPRESSION")
        .env_remove("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
        .env_remove("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT")
        .env_remove("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT")
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint)
        .env("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf")
        .env("OTEL_EXPORTER_OTLP_TIMEOUT", "500")
        .env("OTEL_TRACES_EXPORTER", "otlp")
        .env("OTEL_METRICS_EXPORTER", "none")
        .env("OTEL_LOGS_EXPORTER", "none")
        .env("OTEL_SERVICE_NAME", "ahu-agent")
        .env("OTEL_RESOURCE_ATTRIBUTES", attributes);
}

/// Evaluation capture is an internal, loopback-only override. It lets a
/// single eval run receive OTLP directly, without requiring a collector
/// backend or changing the project's checked-in telemetry policy.
pub(crate) fn eval_endpoint_override() -> Option<String> {
    let raw = std::env::var("AHU_EVAL_OTEL_ENDPOINT").ok()?;
    let endpoint = raw.parse::<url::Url>().ok()?;
    let local = matches!(endpoint.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if endpoint.scheme() != "http"
        || !local
        || endpoint.port().is_none()
        || endpoint.username() != ""
        || endpoint.password().is_some()
        || !matches!(endpoint.path(), "" | "/")
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return None;
    }
    Some(format!(
        "http://{}:{}",
        endpoint.host_str()?,
        endpoint.port()?
    ))
}

#[allow(clippy::too_many_arguments)]
fn resource_attributes(
    agent: &str,
    harness: &str,
    model: &str,
    agent_version: Option<&str>,
    harness_version: Option<&str>,
    task_id: Option<&str>,
    attempt: Option<u32>,
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
    if let Some(attempt) = attempt {
        values.push(format!("ahu.task.attempt={attempt}"));
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

/// Evidence about skill events, never a claim that a skill executed correctly.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillEvidence {
    Observed,
    #[default]
    Unavailable,
    Unverified,
}

impl SkillEvidence {
    pub(crate) fn unverified() -> Self {
        Self::Unverified
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "observed",
            Self::Unavailable => "unavailable",
            Self::Unverified => "unverified",
        }
    }
}

/// Local numeric projection, never passed to an OTEL exporter or child env.
/// Correlation uses the containing result's existing task ID and attempt.
/// No task record, prompt, identity string, or tracker reference is accepted.
#[derive(Debug, serde::Serialize)]
pub(crate) struct LocalMetrics {
    schema_version: u32,
    token_aggregation: &'static str,
    elapsed_ms: Measurement,
    values: std::collections::BTreeMap<&'static str, Measurement>,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum Measurement {
    Observed(u64),
    ObservedFloat(f64),
    Unavailable,
    // A harness-reported amount remains labeled as such; ahu never derives a
    // missing cost from token counts or a pricing table.
}

pub(crate) fn local_metrics(
    config: &TelemetryConfig,
    usage: &crate::headless::TokenUsage,
    cost: &crate::headless::ReportedCost,
    elapsed_ms: Option<u64>,
) -> Option<LocalMetrics> {
    (config.local_metrics || std::env::var("AHU_EVAL_LOCAL_METRICS").as_deref() == Ok("1")).then(
        || LocalMetrics {
            schema_version: 2,
            // The normalizer retains maxima across reports. These are observations,
            // not additive task totals or provider billing measurements.
            token_aggregation: "maximum-reported-per-field",
            elapsed_ms: elapsed_ms.map_or(Measurement::Unavailable, Measurement::Observed),
            values: usage
                .normalized_fields()
                .into_iter()
                .map(|(key, value)| {
                    (
                        key,
                        value.map_or(Measurement::Unavailable, Measurement::Observed),
                    )
                })
                .chain(std::iter::once((
                    "ahu.cost.harness_reported_usd",
                    cost.usd
                        .filter(|amount| amount.is_finite() && *amount >= 0.0)
                        .map_or(Measurement::Unavailable, Measurement::ObservedFloat),
                )))
                .collect(),
        },
    )
}

pub struct SpanGuard {
    span: Option<global::BoxedSpan>,
    started: Instant,
}

impl SpanGuard {
    fn set_attribute(&mut self, attribute: KeyValue) {
        if let Some(span) = self.span.as_mut() {
            span.set_attribute(attribute);
        }
    }

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

    pub fn set_f64(&mut self, key: &'static str, value: f64) {
        if value.is_finite()
            && let Some(span) = self.span.as_mut()
        {
            span.set_attribute(KeyValue::new(key, value));
        }
    }

    pub fn set_bool(&mut self, key: &'static str, value: bool) {
        if let Some(span) = self.span.as_mut() {
            span.set_attribute(KeyValue::new(key, value));
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

/// Begin a payload-free observation for every parsed MCP request/notification.
pub(crate) fn mcp_request_span(request: &serde_json::Value) -> SpanGuard {
    let method = request
        .get("method")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(if request.is_null() {
            "invalid_json"
        } else {
            "unknown"
        });
    let operation = match method {
        "initialize" => "ahu.mcp.initialize",
        "ping" => "ahu.mcp.ping",
        "server/discover" => "ahu.mcp.server.discover",
        "tools/list" => "ahu.mcp.tools.list",
        "tools/call" => "ahu.mcp.tool.call",
        "tasks/get" | "tasks/update" | "tasks/cancel" => "ahu.mcp.tasks.operation",
        "subscriptions/listen" => "ahu.mcp.subscriptions.listen",
        "notifications/initialized"
        | "notifications/cancelled"
        | "notifications/progress"
        | "notifications/roots/list_changed" => "ahu.mcp.notification",
        "invalid_json" => "ahu.mcp.invalid_request",
        _ => "ahu.mcp.request",
    };
    let mut span = span(operation, []);
    let method = match method {
        "initialize"
        | "ping"
        | "server/discover"
        | "tools/list"
        | "tools/call"
        | "tasks/get"
        | "tasks/update"
        | "tasks/cancel"
        | "subscriptions/listen"
        | "notifications/initialized"
        | "notifications/cancelled"
        | "notifications/progress"
        | "notifications/roots/list_changed"
        | "invalid_json" => method,
        _ => "unknown",
    };
    span.set_string("ahu.mcp.method", method);
    if method == "tools/call" {
        let params = request.get("params").unwrap_or(&serde_json::Value::Null);
        let name = params
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        let arguments = params.get("arguments").unwrap_or(&serde_json::Value::Null);
        for attribute in mcp_tool_attributes(name, arguments) {
            span.set_attribute(attribute);
        }
    }
    span
}

fn mcp_tool_attributes(name: &str, arguments: &serde_json::Value) -> Vec<KeyValue> {
    let name = if crate::mcp::TOOL_NAMES.contains(&name) {
        name
    } else {
        "unknown"
    };
    let mut attributes = vec![KeyValue::new("ahu.mcp.tool.name", name.to_string())];
    if name == "ahu_typed_decide" {
        if let Ok(bytes) = serde_json::to_vec(arguments) {
            attributes.push(KeyValue::new(
                "ahu.mcp.decision.arguments.bytes",
                bytes.len() as i64,
            ));
        }
        let items_form = arguments.get("items").is_some()
            && arguments.get("question").is_some()
            && arguments.get("state").is_none()
            && arguments.get("questions").is_none();
        if items_form {
            attributes.push(KeyValue::new("ahu.mcp.decision.request.format", "items"));
            if let Some(items) = arguments
                .get("items")
                .and_then(serde_json::Value::as_object)
            {
                attributes.push(KeyValue::new(
                    "ahu.mcp.decision.questions.count",
                    items.len() as i64,
                ));
            }
            if let Some(kind) = arguments
                .get("question")
                .and_then(|question| question.get("type"))
                .and_then(serde_json::Value::as_str)
                .filter(|kind| ["choice", "score", "probability"].contains(kind))
            {
                attributes.push(KeyValue::new(
                    "ahu.mcp.decision.questions.types",
                    kind.to_string(),
                ));
            }
        } else if arguments.get("state").is_some()
            && arguments.get("questions").is_some()
            && arguments.get("items").is_none()
            && arguments.get("question").is_none()
        {
            attributes.push(KeyValue::new(
                "ahu.mcp.decision.request.format",
                "questions",
            ));
        }
        let questions = arguments
            .get("questions")
            .and_then(serde_json::Value::as_object);
        if let Some(questions) = questions {
            attributes.push(KeyValue::new(
                "ahu.mcp.decision.questions.count",
                questions.len() as i64,
            ));
            let mut types = questions
                .values()
                .filter_map(|question| question.get("type").and_then(serde_json::Value::as_str))
                .filter(|kind| ["choice", "score", "probability"].contains(kind))
                .collect::<Vec<_>>();
            types.sort_unstable();
            types.dedup();
            if !types.is_empty() {
                attributes.push(KeyValue::new(
                    "ahu.mcp.decision.questions.types",
                    types.join(","),
                ));
            }
            let mut keys = questions
                .values()
                .filter_map(|question| {
                    question
                        .get("telemetry_key")
                        .and_then(serde_json::Value::as_str)
                })
                .filter(|key| safe_eval_identifier(key))
                .collect::<Vec<_>>();
            keys.sort_unstable();
            keys.dedup();
            if !keys.is_empty() {
                attributes.push(KeyValue::new(
                    "ahu.mcp.decision.telemetry_keys",
                    keys.join(","),
                ));
            }
        }
    }
    attributes
}

/// Add bounded protocol metadata without copying content into telemetry.
pub(crate) fn finish_mcp_request_span(
    span: &mut SpanGuard,
    request: &serde_json::Value,
    response: Option<&serde_json::Value>,
) {
    let method = request
        .get("method")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(if request.is_null() {
            "invalid_json"
        } else {
            "unknown"
        });
    let name = request
        .pointer("/params/name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let Some(response) = response else {
        MCP_REQUESTS.fetch_add(1, Ordering::Relaxed);
        span.set_string(
            "ahu.mcp.outcome",
            if method.starts_with("notifications/") {
                "notification"
            } else {
                "no_response"
            },
        );
        return;
    };
    MCP_REQUESTS.fetch_add(1, Ordering::Relaxed);
    if method == "tools/list" {
        MCP_LIST_REQUESTS.fetch_add(1, Ordering::Relaxed);
    }
    if method == "tools/call" {
        MCP_TOOL_CALLS.fetch_add(1, Ordering::Relaxed);
        if name == "ahu_typed_decide" {
            MCP_DECISION_CALLS.fetch_add(1, Ordering::Relaxed);
        }
        if response.get("error").is_some()
            || response.pointer("/result/isError") == Some(&serde_json::Value::Bool(true))
        {
            MCP_TOOL_ERRORS.fetch_add(1, Ordering::Relaxed);
        }
    }
    for attribute in mcp_result_attributes(method, name, response) {
        span.set_attribute(attribute);
    }
}

/// Emit bounded per-process coverage so an evaluator can distinguish no MCP
/// traffic from a set of successfully exported per-request spans.
pub(crate) fn mcp_session_summary(telemetry_configured: bool) {
    let mut span = span("ahu.mcp.session", []);
    span.set_u64(
        "ahu.mcp.requests.count",
        MCP_REQUESTS.load(Ordering::Relaxed),
    );
    span.set_u64(
        "ahu.mcp.tool.calls.count",
        MCP_TOOL_CALLS.load(Ordering::Relaxed),
    );
    span.set_u64(
        "ahu.mcp.decision.calls.count",
        MCP_DECISION_CALLS.load(Ordering::Relaxed),
    );
    span.set_u64(
        "ahu.mcp.tool.errors.count",
        MCP_TOOL_ERRORS.load(Ordering::Relaxed),
    );
    span.set_u64(
        "ahu.mcp.transport.errors.count",
        MCP_TRANSPORT_ERRORS.load(Ordering::Relaxed),
    );
    span.set_u64(
        "ahu.mcp.tools.list.count",
        MCP_LIST_REQUESTS.load(Ordering::Relaxed),
    );
    span.set_bool("ahu.mcp.telemetry.configured", telemetry_configured);
    span.set_bool("ahu.mcp.telemetry.exporter_ready", exporter_ready());
}

pub(crate) fn mcp_transport_error() {
    MCP_TRANSPORT_ERRORS.fetch_add(1, Ordering::Relaxed);
    let mut span = span("ahu.mcp.transport.error", []);
    span.set_string("ahu.mcp.outcome", "error");
    span.set_string("ahu.mcp.error.category", "transport_error");
}

fn exporter_ready() -> bool {
    provider_slot()
        .lock()
        .expect("telemetry mutex poisoned")
        .is_some()
}

fn mcp_result_attributes(method: &str, name: &str, response: &serde_json::Value) -> Vec<KeyValue> {
    let failed = response.get("error").is_some()
        || response.pointer("/result/isError") == Some(&serde_json::Value::Bool(true));
    let mut attributes = vec![KeyValue::new(
        "ahu.mcp.outcome",
        if failed { "error" } else { "success" },
    )];
    if failed {
        attributes.push(KeyValue::new(
            "ahu.mcp.error.category",
            mcp_error_category(method, name, response),
        ));
    }
    if method == "tools/list"
        && let Some(tools) = response
            .pointer("/result/tools")
            .and_then(serde_json::Value::as_array)
    {
        attributes.push(KeyValue::new("ahu.mcp.tools.count", tools.len() as i64));
        let mut names = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(serde_json::Value::as_str))
            .filter(|tool| crate::mcp::TOOL_NAMES.contains(tool))
            .collect::<Vec<_>>();
        names.sort_unstable();
        names.dedup();
        if !names.is_empty() {
            attributes.push(KeyValue::new("ahu.mcp.tools.available", names.join(",")));
        }
    }
    if method == "initialize"
        && let Some(version) = response
            .pointer("/result/protocolVersion")
            .and_then(serde_json::Value::as_str)
        && version.len() <= 32
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.')
    {
        attributes.push(KeyValue::new(
            "ahu.mcp.protocol.version",
            version.to_string(),
        ));
    }
    if response.pointer("/result/isError") == Some(&serde_json::Value::Bool(true))
        || response.get("error").is_some()
    {
        return attributes;
    }
    if method == "tools/call" && name == "ahu_skills_suggest" {
        if let Some(value) = response.pointer("/result/structuredContent") {
            for (field, key, allowed) in [
                (
                    "mode",
                    "ahu.selection.mode",
                    &["none", "lexical", "decision"][..],
                ),
                (
                    "status",
                    "ahu.selection.status",
                    &["disabled", "suggested", "abstained", "fallback"][..],
                ),
            ] {
                if let Some(v) = value.get(field).and_then(serde_json::Value::as_str)
                    && allowed.contains(&v)
                {
                    attributes.push(KeyValue::new(key, v.to_owned()));
                }
            }
            for (pointer, key) in [
                ("/candidate_count", "ahu.selection.candidates"),
                ("/policy_version", "ahu.selection.policy_version"),
                ("/service/prompt_tokens", "ahu.selection.tokens.input"),
                ("/service/generated_tokens", "ahu.selection.tokens.output"),
            ] {
                if let Some(v) = value.pointer(pointer).and_then(serde_json::Value::as_u64) {
                    attributes.push(KeyValue::new(key, v.min(i64::MAX as u64) as i64));
                }
            }
            for (field, key) in [
                (
                    "prompt_tokens_complete",
                    "ahu.selection.tokens.input.complete",
                ),
                (
                    "generated_tokens_complete",
                    "ahu.selection.tokens.output.complete",
                ),
            ] {
                let complete = value["service"][field].as_bool() == Some(true);
                attributes.push(KeyValue::new(key, i64::from(complete)));
            }
            if let Some(rows) = value.get("selected").and_then(serde_json::Value::as_array) {
                attributes.push(KeyValue::new("ahu.selection.selected", rows.len() as i64));
            }
            if let Some(v) = value.get("elapsed_ms").and_then(serde_json::Value::as_f64)
                && v.is_finite()
                && v >= 0.0
            {
                attributes.push(KeyValue::new("ahu.selection.duration_ms", v));
            }
        }
        return attributes;
    }
    if method != "tools/call" || name != "ahu_typed_decide" {
        if method == "tools/call" {
            let pointer = match name {
                "ahu_agents_list" => "/result/structuredContent/agents",
                "ahu_tasks_list" => "/result/structuredContent/tasks",
                _ => "",
            };
            if !pointer.is_empty()
                && let Some(rows) = response
                    .pointer(pointer)
                    .and_then(serde_json::Value::as_array)
            {
                attributes.push(KeyValue::new(
                    "ahu.mcp.tool.result_count",
                    rows.len() as i64,
                ));
            }
            if name == "ahu_task_get"
                && let Some(state) = response
                    .pointer("/result/structuredContent/session_state")
                    .and_then(serde_json::Value::as_str)
                && ["queued", "running", "completed", "failed", "cancelled"].contains(&state)
            {
                attributes.push(KeyValue::new("ahu.mcp.task.state", state.to_string()));
            }
        }
        return attributes;
    }
    let Some(service) = response
        .pointer("/result/structuredContent/service")
        .and_then(serde_json::Value::as_object)
    else {
        return attributes;
    };
    for (input, output) in [
        ("backend", "ahu.mcp.decision.backend"),
        ("model", "ahu.mcp.decision.model"),
    ] {
        if let Some(value) = service.get(input).and_then(serde_json::Value::as_str)
            && !value.is_empty()
            && value.len() <= 128
            && safe_resource_value(output, value)
        {
            attributes.push(KeyValue::new(output, value.to_string()));
        }
    }
    for (input, output) in [
        ("prompt_tokens", "ahu.mcp.decision.tokens.input"),
        ("generated_tokens", "ahu.mcp.decision.tokens.output"),
    ] {
        if let Some(value) = service.get(input).and_then(serde_json::Value::as_u64) {
            attributes.push(KeyValue::new(output, value.min(i64::MAX as u64) as i64));
        }
    }
    for (input, output) in [
        ("duration_ms", "ahu.mcp.decision.duration_ms"),
        ("load_ms", "ahu.mcp.decision.load_ms"),
        ("prompt_eval_ms", "ahu.mcp.decision.prompt_eval_ms"),
        ("generation_ms", "ahu.mcp.decision.generation_ms"),
    ] {
        if let Some(value) = service.get(input).and_then(serde_json::Value::as_f64)
            && value >= 0.0
        {
            attributes.push(KeyValue::new(output, value));
        }
    }
    attributes
}

fn mcp_error_category(method: &str, name: &str, response: &serde_json::Value) -> &'static str {
    if let Some(code) = response
        .pointer("/error/code")
        .and_then(serde_json::Value::as_i64)
    {
        return match code {
            -32700 => "parse_error",
            -32600 => "invalid_request",
            -32601 => "method_not_found",
            -32602 => "invalid_arguments",
            _ => "protocol_error",
        };
    }
    if method == "tools/call" && name == "ahu_typed_decide" {
        let message = response
            .pointer("/result/content/0/text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if message.starts_with("ahu_typed_decide requires")
            || message.starts_with("ahu_typed_decide needs")
            || message.starts_with(".env")
            || message.starts_with("cannot safely read .env")
            || message.starts_with("cannot inspect .env")
            || message.starts_with("cannot parse .env")
            || message.starts_with("TypeSafe API key")
            || message.starts_with("invalid AHU_DECISION_URL")
            || message.starts_with("AHU_DECISION_URL must")
            || message.starts_with("cannot create decision client")
            || message.starts_with("cannot create TypeSafe decision client")
        {
            "decision_configuration"
        } else if message.starts_with("decision service request failed")
            || message.starts_with("TypeSafe decision request failed")
        {
            "decision_service_unavailable"
        } else if message.starts_with("decision service returned HTTP")
            || message.starts_with("TypeSafe decision API returned HTTP")
        {
            "decision_service_http_error"
        } else if message.starts_with("decision service returned invalid JSON")
            || message.starts_with("decision service response")
            || message.starts_with("decision service answers")
            || message.starts_with("TypeSafe decision response")
            || message.starts_with("TypeSafe response")
            || message.starts_with("TypeSafe answers")
            || message.starts_with("TypeSafe answer")
            || message.starts_with("TypeSafe choice")
            || message.starts_with("TypeSafe score")
            || message.starts_with("TypeSafe noul")
            || message.starts_with("TypeSafe confidence")
            || message.starts_with("answer ")
        {
            "decision_service_invalid_response"
        } else {
            "tool_error"
        }
    } else if method == "tools/call" {
        "tool_error"
    } else {
        "protocol_error"
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
    use super::{
        configure_child, mcp_result_attributes, mcp_tool_attributes, parse_ahu_resource_attributes,
        validate_config,
    };
    use crate::config::TelemetryConfig;
    use opentelemetry::{KeyValue, Value};
    use std::path::Path;

    #[test]
    fn local_metrics_are_opt_in_numeric_and_do_not_infer_totals() {
        let mut events = crate::headless::Events::default();
        events.observe("codex", br#"{"type":"turn.completed","usage":{"input_tokens":0,"output_tokens":7,"cached_tokens":"private-marker","total_tokens":-1,"tracker_url":"private-marker"},"prompt":"private-marker","model":"private-marker"}"#);
        let mut config = TelemetryConfig::default();
        assert!(super::local_metrics(&config, &events.usage, &events.cost, Some(12)).is_none());
        config.local_metrics = true;
        let value = serde_json::to_value(super::local_metrics(
            &config,
            &events.usage,
            &events.cost,
            Some(12),
        ))
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "schema_version": 2,
                "token_aggregation": "maximum-reported-per-field",
                "elapsed_ms": {"kind":"observed", "value":12},
                "values": {
                    "ahu.tokens.input": {"kind":"observed", "value":0},
                    "ahu.tokens.output": {"kind":"observed", "value":7},
                    "ahu.tokens.cached": {"kind":"unavailable"},
                    "ahu.tokens.cache_write": {"kind":"unavailable"},
                    "ahu.tokens.reasoning": {"kind":"unavailable"},
                    "ahu.tokens.total": {"kind":"unavailable"},
                    "ahu.cost.harness_reported_usd": {"kind":"unavailable"}
                }
            })
        );
        let mut child = std::process::Command::new("true");
        configure_child(
            &mut child, &config, "agent", "codex", "model", None, None, None, None,
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
            Some(2),
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
        let compression_keys = [
            "OTEL_EXPORTER_OTLP_COMPRESSION",
            "OTEL_EXPORTER_OTLP_TRACES_COMPRESSION",
            "OTEL_EXPORTER_OTLP_METRICS_COMPRESSION",
            "OTEL_EXPORTER_OTLP_LOGS_COMPRESSION",
        ];
        for key in compression_keys {
            command.env(key, "gzip");
        }
        configure_child(
            &mut command,
            &enabled,
            "agent",
            "codex",
            "model",
            None,
            None,
            Some("task"),
            Some(2),
        );
        let env: std::collections::BTreeMap<_, _> = command
            .get_envs()
            .filter_map(|(key, value)| Some((key.to_str()?, value?.to_str()?)))
            .collect();
        assert_eq!(env["OTEL_EXPORTER_OTLP_ENDPOINT"], "http://127.0.0.1:4318");
        assert!(env["OTEL_RESOURCE_ATTRIBUTES"].contains("ahu.harness=codex"));
        assert!(env["OTEL_RESOURCE_ATTRIBUTES"].contains("ahu.task.attempt=2"));
        assert!(env["AHU_MCP_RESOURCE_ATTRIBUTES"].contains("ahu.task.attempt=2"));
        assert!(env["AHU_MCP_RESOURCE_ATTRIBUTES"].contains("ahu.task.id=task"));
        // No compression is represented by absence. The Rust OTLP SDK rejects
        // the string "none", preventing native exporter initialization.
        for key in compression_keys {
            assert!(
                command
                    .get_envs()
                    .any(|(name, value)| name == key && value.is_none())
            );
        }
    }

    #[test]
    fn mcp_batch_attributes_are_payload_free() {
        let input = serde_json::json!({"items":{"private-id":"private-evidence","another-id":[]},"question":{"type":"choice","instructions":"private-policy","options":{"private-option":"private-description"},"telemetry_key":"unwanted_dimension"}});
        let attributes = mcp_tool_attributes("ahu_typed_decide", &input);
        assert_eq!(
            string_attr(&attributes, "ahu.mcp.decision.request.format"),
            Some("items".into())
        );
        assert_eq!(
            integer_attr(&attributes, "ahu.mcp.decision.arguments.bytes"),
            Some(serde_json::to_vec(&input).unwrap().len() as i64)
        );
        assert_eq!(
            integer_attr(&attributes, "ahu.mcp.decision.questions.count"),
            Some(2)
        );
        assert_eq!(
            string_attr(&attributes, "ahu.mcp.decision.questions.types"),
            Some("choice".into())
        );
        let text = format!("{attributes:?}");
        for private in ["private-", "another-id", "unwanted_dimension"] {
            assert!(!text.contains(private));
        }
        let legacy =
            serde_json::json!({"state":{},"questions":{"private-id":{"type":"probability"}}});
        assert_eq!(
            string_attr(
                &mcp_tool_attributes("ahu_typed_decide", &legacy),
                "ahu.mcp.decision.request.format"
            ),
            Some("questions".into())
        );
    }

    #[test]
    fn mcp_decision_attributes_are_payload_free_and_capture_use_and_usage() {
        let arguments = serde_json::json!({
            "state":{"body":"private decision payload"},
            "questions":{
                "route":{"type":"choice","instructions":"private instruction","telemetry_key":"department"},
                "risk":{"type":"probability","instructions":"private proposition","telemetry_key":"refund_requested"}
            }
        });
        let attributes = mcp_tool_attributes("ahu_typed_decide", &arguments);
        assert_eq!(
            string_attr(&attributes, "ahu.mcp.tool.name"),
            Some("ahu_typed_decide".into())
        );
        assert_eq!(
            integer_attr(&attributes, "ahu.mcp.decision.questions.count"),
            Some(2)
        );
        assert_eq!(
            string_attr(&attributes, "ahu.mcp.decision.questions.types"),
            Some("choice,probability".into())
        );
        assert_eq!(
            string_attr(&attributes, "ahu.mcp.decision.telemetry_keys"),
            Some("department,refund_requested".into())
        );
        let serialized = format!("{attributes:?}");
        assert!(!serialized.contains("private decision payload"));
        assert!(!serialized.contains("private instruction"));

        let result = serde_json::json!({"result":{"structuredContent":{
            "answers":{"route":{"value":"billing"}},
            "service":{"backend":"ollama","model":"qwen-local", "prompt_tokens":31,
                "generated_tokens":9,"duration_ms":123.5,"private":"private response payload"}
        }}});
        let result_attributes = mcp_result_attributes("tools/call", "ahu_typed_decide", &result);
        assert_eq!(
            string_attr(&result_attributes, "ahu.mcp.outcome"),
            Some("success".into())
        );
        assert_eq!(
            string_attr(&result_attributes, "ahu.mcp.decision.backend"),
            Some("ollama".into())
        );
        assert_eq!(
            string_attr(&result_attributes, "ahu.mcp.decision.model"),
            Some("qwen-local".into())
        );
        assert_eq!(
            integer_attr(&result_attributes, "ahu.mcp.decision.tokens.input"),
            Some(31)
        );
        assert_eq!(
            integer_attr(&result_attributes, "ahu.mcp.decision.tokens.output"),
            Some(9)
        );
        assert_eq!(
            float_attr(&result_attributes, "ahu.mcp.decision.duration_ms"),
            Some(123.5)
        );
        assert!(!format!("{result_attributes:?}").contains("private response payload"));
        assert_eq!(
            mcp_result_attributes("tools/call", "ahu_agents_list", &result).len(),
            1
        );
        let without_service = mcp_result_attributes(
            "tools/call",
            "ahu_typed_decide",
            &serde_json::json!({"result":{"structuredContent":{"answers":{}}}}),
        );
        assert_eq!(
            string_attr(&without_service, "ahu.mcp.outcome"),
            Some("success".into())
        );

        let error = mcp_result_attributes(
            "tools/call",
            "ahu_typed_decide",
            &serde_json::json!({"result":{"isError":true}}),
        );
        assert_eq!(string_attr(&error, "ahu.mcp.outcome"), Some("error".into()));
        assert_eq!(error.len(), 2);
        assert_eq!(
            string_attr(&error, "ahu.mcp.error.category"),
            Some("tool_error".into())
        );

        let unavailable = mcp_result_attributes(
            "tools/call",
            "ahu_typed_decide",
            &serde_json::json!({"result":{"isError":true,"content":[{"text":"decision service request failed: private-marker"}]}}),
        );
        assert_eq!(
            string_attr(&unavailable, "ahu.mcp.error.category"),
            Some("decision_service_unavailable".into())
        );
        assert!(!format!("{unavailable:?}").contains("private-marker"));
        for (message, category) in [
            (
                "invalid AHU_DECISION_URL: private-marker",
                "decision_configuration",
            ),
            (
                "TypeSafe API key is empty or malformed",
                "decision_configuration",
            ),
            (".env permissions are too broad", "decision_configuration"),
            (
                "TypeSafe decision request failed",
                "decision_service_unavailable",
            ),
            (
                "TypeSafe decision API returned HTTP 401",
                "decision_service_http_error",
            ),
            (
                "TypeSafe decision response is invalid JSON",
                "decision_service_invalid_response",
            ),
            (
                "decision service returned HTTP 503",
                "decision_service_http_error",
            ),
            (
                "decision service returned invalid JSON",
                "decision_service_invalid_response",
            ),
        ] {
            let attributes = mcp_result_attributes(
                "tools/call",
                "ahu_typed_decide",
                &serde_json::json!({"result":{"isError":true,"content":[{"text":message}]}}),
            );
            assert_eq!(
                string_attr(&attributes, "ahu.mcp.error.category"),
                Some(category.into())
            );
        }
        let invalid = mcp_result_attributes(
            "tools/call",
            "ahu_typed_decide",
            &serde_json::json!({"error":{"code":-32602,"message":"private-marker"}}),
        );
        assert_eq!(
            string_attr(&invalid, "ahu.mcp.error.category"),
            Some("invalid_arguments".into())
        );
    }

    #[test]
    fn mcp_tool_names_are_bounded_and_resource_identity_is_allowlisted() {
        let unknown = mcp_tool_attributes(
            "attacker-supplied-private-tool-name",
            &serde_json::json!({}),
        );
        assert_eq!(
            string_attr(&unknown, "ahu.mcp.tool.name"),
            Some("unknown".into())
        );
        let attributes = parse_ahu_resource_attributes(
            "ahu.harness=opencode,ahu.model=ollama/qwen,ahu.eval.run_id=run-001,ahu.eval.case_id=case-001,ahu.eval.stage=candidate,private.marker=secret,ahu.task.id=task-123",
        );
        assert_eq!(attributes.len(), 6);
        assert!(!format!("{attributes:?}").contains("secret"));
        let escaped = parse_ahu_resource_attributes(
            r"ahu.agent.name=agent\,with\,commas,private.marker=secret",
        );
        assert!(escaped.is_empty());
        let duplicate =
            parse_ahu_resource_attributes("ahu.model=qwen-local,ahu.model=private-marker");
        assert_eq!(duplicate, vec![("ahu.model", "qwen-local".to_string())]);
    }

    #[test]
    fn mcp_protocol_spans_include_offered_tools_and_protocol_outcomes() {
        let tools = mcp_result_attributes(
            "tools/list",
            "unknown",
            &serde_json::json!({"result":{"tools":[
                {"name":"ahu_agents_list"},{"name":"ahu_typed_decide"},
                {"name":"private-tool-name"}
            ]}}),
        );
        assert_eq!(integer_attr(&tools, "ahu.mcp.tools.count"), Some(3));
        assert_eq!(
            string_attr(&tools, "ahu.mcp.tools.available"),
            Some("ahu_agents_list,ahu_typed_decide".into())
        );
        assert!(!format!("{tools:?}").contains("private-tool-name"));
        let initialized = mcp_result_attributes(
            "initialize",
            "unknown",
            &serde_json::json!({"result":{"protocolVersion":"2026-07-28"}}),
        );
        assert_eq!(
            string_attr(&initialized, "ahu.mcp.protocol.version"),
            Some("2026-07-28".into())
        );

        let listed = mcp_result_attributes(
            "tools/call",
            "ahu_agents_list",
            &serde_json::json!({"result":{"structuredContent":{"agents":[
                {"name":"private-agent-name"}
            ]}}}),
        );
        assert_eq!(integer_attr(&listed, "ahu.mcp.tool.result_count"), Some(1));
        assert!(!format!("{listed:?}").contains("private-agent-name"));
        let inspected = mcp_result_attributes(
            "tools/call",
            "ahu_task_get",
            &serde_json::json!({"result":{"structuredContent":{
                "task_id":"private-task-id", "session_state":"completed"
            }}}),
        );
        assert_eq!(
            string_attr(&inspected, "ahu.mcp.task.state"),
            Some("completed".into())
        );
        assert!(!format!("{inspected:?}").contains("private-task-id"));
    }

    #[test]
    fn a_full_span_queue_drops_telemetry_without_blocking_span_completion() {
        use opentelemetry::trace::{Span as _, Tracer as _, TracerProvider as _};
        use opentelemetry_sdk::error::OTelSdkResult;
        use opentelemetry_sdk::trace::{SpanData, SpanExporter as SdkSpanExporter};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Condvar, Mutex};
        use std::time::{Duration, Instant};

        #[derive(Debug)]
        struct BlockingExporter {
            gate: Arc<(Mutex<(bool, bool)>, Condvar)>,
            exported: Arc<AtomicUsize>,
        }

        impl SdkSpanExporter for BlockingExporter {
            async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
                let (state, ready) = &*self.gate;
                let mut state = state.lock().unwrap();
                state.0 = true;
                ready.notify_all();
                while !state.1 {
                    state = ready.wait(state).unwrap();
                }
                self.exported.fetch_add(batch.len(), Ordering::Relaxed);
                Ok(())
            }
        }

        let gate = Arc::new((Mutex::new((false, false)), Condvar::new()));
        let exported = Arc::new(AtomicUsize::new(0));
        let processor = super::bounded_span_processor(BlockingExporter {
            gate: gate.clone(),
            exported: exported.clone(),
        });
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
            .with_span_processor(processor)
            .build();
        let tracer = provider.tracer("ahu-backpressure-test");
        for _ in 0..super::SPAN_EXPORT_BATCH_SIZE {
            tracer.start("initial").end();
        }

        let (state, ready) = &*gate;
        let state = state.lock().unwrap();
        let (state, _) = ready
            .wait_timeout_while(state, Duration::from_secs(2), |state| !state.0)
            .unwrap();
        let export_started = state.0;
        drop(state);

        let (completed_tx, completed_rx) = std::sync::mpsc::channel();
        let producer_tracer = tracer.clone();
        let producer = std::thread::spawn(move || {
            let started = Instant::now();
            for _ in 0..1024 {
                producer_tracer.start("saturated").end();
            }
            let _ = completed_tx.send(started.elapsed());
        });
        let producer_duration = completed_rx.recv_timeout(Duration::from_secs(1));

        let (state, ready) = &*gate;
        state.lock().unwrap().1 = true;
        ready.notify_all();
        let _ = provider.shutdown();
        let _ = producer.join();

        assert!(export_started, "exporter never started to occupy the queue");
        let producer_duration =
            producer_duration.expect("span completion blocked while the OTLP queue was full");
        assert!(producer_duration < Duration::from_secs(1));
        assert!(exported.load(Ordering::Relaxed) < 32 + 1024);
    }

    fn string_attr(attributes: &[KeyValue], name: &str) -> Option<String> {
        attributes
            .iter()
            .find(|attr| attr.key.as_str() == name)
            .and_then(|attr| match &attr.value {
                Value::String(value) => Some(value.to_string()),
                _ => None,
            })
    }

    fn integer_attr(attributes: &[KeyValue], name: &str) -> Option<i64> {
        attributes
            .iter()
            .find(|attr| attr.key.as_str() == name)
            .and_then(|attr| match attr.value {
                Value::I64(value) => Some(value),
                _ => None,
            })
    }

    fn float_attr(attributes: &[KeyValue], name: &str) -> Option<f64> {
        attributes
            .iter()
            .find(|attr| attr.key.as_str() == name)
            .and_then(|attr| match attr.value {
                Value::F64(value) => Some(value),
                _ => None,
            })
    }
}

/// Export one bounded prelaunch selection observation to this eval's collector.
/// Uses a private provider rather than changing global telemetry or process env.
pub(crate) fn export_eval_selection(
    endpoint: &str,
    run_id: &str,
    selection_id: &str,
    selection: &crate::skill_selection::Selection,
) -> bool {
    use opentelemetry_proto::tonic::{
        collector::trace::v1::ExportTraceServiceRequest,
        common::v1::{AnyValue, KeyValue as ProtoKeyValue, any_value},
        resource::v1::Resource as ProtoResource,
        trace::v1::{ResourceSpans, ScopeSpans, Span as ProtoSpan},
    };
    use opentelemetry_sdk::trace::{IdGenerator, RandomIdGenerator};
    use prost::Message;
    if selection.mode == crate::skill_selection::Mode::None {
        return false;
    }
    let Ok(url) = url::Url::parse(endpoint) else {
        return false;
    };
    if url.scheme() != "http"
        || !url.host_str().is_some_and(|h| {
            h.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        })
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let attr = |key: &str, value: any_value::Value| ProtoKeyValue {
        key: key.into(),
        value: Some(AnyValue { value: Some(value) }),
        ..Default::default()
    };
    let string = |key: &str, value: &str| attr(key, any_value::Value::StringValue(value.into()));
    let integer = |key: &str, value: u64| {
        attr(
            key,
            any_value::Value::IntValue(value.min(i64::MAX as u64) as i64),
        )
    };
    let mut attributes = vec![
        string("ahu.selection.purpose", "skill_relevance"),
        string("ahu.selection.mode", selection.mode.as_str()),
        integer(
            "ahu.selection.policy_version",
            selection.policy_version as u64,
        ),
        integer("ahu.selection.candidates", selection.candidate_count as u64),
        integer("ahu.selection.selected", selection.selected.len() as u64),
        string("ahu.selection.status", &selection.status),
        attr(
            "ahu.selection.duration_ms",
            any_value::Value::DoubleValue(selection.elapsed_ms),
        ),
        string("ahu.selection.catalog_digest", &selection.catalog_digest),
    ];
    if let Some(service) = &selection.service {
        for (field, key) in [
            ("prompt_tokens", "ahu.selection.tokens.input"),
            ("generated_tokens", "ahu.selection.tokens.output"),
        ] {
            if let Some(value) = service.get(field).and_then(serde_json::Value::as_u64) {
                attributes.push(integer(key, value));
            }
            attributes.push(integer(
                &format!("{key}.complete"),
                u64::from(
                    service
                        .get(format!("{field}_complete"))
                        .and_then(serde_json::Value::as_bool)
                        == Some(true),
                ),
            ));
        }
    }
    let ids = RandomIdGenerator::default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos().min(u64::MAX as u128) as u64);
    // Construct the wire resource explicitly: SDK Resource::builder and the
    // default OTLP exporter both import unrelated inherited environment data.
    let message = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(ProtoResource {
                attributes: vec![
                    string("service.name", "ahu-eval"),
                    string("ahu.eval.run_id", run_id),
                    string("ahu.eval.stage", "skill_selection"),
                    string("ahu.task.id", selection_id),
                    string("ahu.task.attempt", "1"),
                ],
                ..Default::default()
            }),
            scope_spans: vec![ScopeSpans {
                spans: vec![ProtoSpan {
                    trace_id: ids.new_trace_id().to_bytes().to_vec(),
                    span_id: ids.new_span_id().to_bytes().to_vec(),
                    name: "ahu.skills.selection".into(),
                    start_time_unix_nano: now,
                    end_time_unix_nano: now,
                    attributes,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    // Never consult inherited proxy/OTLP headers; never follow redirects.
    let Ok(client) = reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(500))
        .build()
    else {
        return false;
    };
    client
        .post(format!("{}/v1/traces", endpoint.trim_end_matches('/')))
        .header(reqwest::header::CONTENT_TYPE, "application/x-protobuf")
        .body(message.encode_to_vec())
        .send()
        .is_ok_and(|response| response.status().is_success())
}

#[cfg(test)]
mod selection_tests {

    #[test]
    fn selection_export_environment_child() {
        let Ok(endpoint) = std::env::var("AHU_TEST_SELECTION_ENDPOINT") else {
            return;
        };
        if std::env::var("AHU_TEST_DECISION_EXPORT").as_deref() == Ok("true") {
            let mut observation = crate::eval::decision::Observation::new("typed_decision");
            observation.status = "scored".into();
            observation.calls_attempted = 1;
            observation.elapsed_ms = Some(2.0);
            observation.service = Some(
                serde_json::json!({"prompt_tokens":1,"model":"synthetic-private-model", "evidence":"synthetic-evidence-secret"}),
            );
            observation.telemetry = Some(serde_json::json!({"evidence":"synthetic-trace-secret"}));
            assert!(super::export_eval_decision(
                &endpoint,
                "wire-run",
                "wire-evaluator",
                &observation
            ));
            return;
        }
        let selection = serde_json::from_value(serde_json::json!({
            "mode":"lexical","policy_version":1,"catalog_digest":"a".repeat(64),
            "candidate_count":1,"selected":[],"status":"abstained","elapsed_ms":1.0,
            "service":null,"error_code":null
        }))
        .unwrap();
        assert!(super::export_eval_selection(
            &endpoint,
            "wire-run",
            "wire-selection",
            &selection
        ));
    }

    #[test]
    fn selection_wire_excludes_inherited_headers_resources_and_proxy() {
        check_export_wire(false);
    }

    #[test]
    fn decision_evaluator_wire_excludes_inherited_headers_resources_evidence_and_proxy() {
        check_export_wire(true);
    }

    fn check_export_wire(decision: bool) {
        use std::io::{BufRead, BufReader, Read, Write};
        use std::net::TcpListener;
        use std::time::{Duration, Instant};
        let collector = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", collector.local_addr().unwrap());
        collector.set_nonblocking(true).unwrap();
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        proxy.set_nonblocking(true).unwrap();
        let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
        let capture = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(4);
            let (mut stream, _) = loop {
                match collector.accept() {
                    Ok(pair) => break pair,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => return None,
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                let deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    match reader.read_line(&mut line) {
                        Ok(_) => break,
                        Err(error)
                            if matches!(
                                error.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) && Instant::now() < deadline =>
                        {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("cannot read local OTLP request headers: {error}"),
                    }
                }
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                headers.push_str(&line);
            }
            let length = headers
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            Some((headers, body))
        });
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .env_clear()
            .env("AHU_TEST_SELECTION_ENDPOINT", endpoint)
            .env("AHU_TEST_DECISION_EXPORT", decision.to_string())
            .env("HTTP_PROXY", &proxy_url)
            .env("ALL_PROXY", &proxy_url)
            .env("http_proxy", &proxy_url)
            .env("NO_PROXY", "")
            .env("no_proxy", "")
            .env(
                "OTEL_EXPORTER_OTLP_HEADERS",
                "authorization=synthetic-header-secret",
            )
            .env(
                "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
                "x-private=synthetic-trace-secret",
            )
            .env(
                "OTEL_RESOURCE_ATTRIBUTES",
                "private.attr=synthetic-resource-secret",
            )
            .args([
                "--exact",
                "telemetry::selection_tests::selection_export_environment_child",
            ])
            .output()
            .unwrap();
        let wire = capture.join().unwrap();
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        let (headers, body) = wire.expect("local collector received the request directly");
        assert!(!headers.contains("synthetic-"));
        for sentinel in [
            b"synthetic-resource-secret".as_slice(),
            b"private.attr".as_slice(),
            b"synthetic-".as_slice(),
        ] {
            assert!(!body.windows(sentinel.len()).any(|w| w == sentinel));
        }
        assert!(matches!(proxy.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
    }
    #[test]
    fn selection_export_reaches_only_its_eval_receiver_without_payload() {
        let receiver = crate::eval_otel::Receiver::start_for(Some("selection-run"), None).unwrap();
        let selection: crate::skill_selection::Selection =
            serde_json::from_value(serde_json::json!({
                "mode":"decision","policy_version":1,"catalog_digest":"a".repeat(64),
                "candidate_count":4,"selected":[".agents/skills/private-marker/SKILL.md"],
                "status":"suggested","elapsed_ms":12.0,
                "service":{"prompt_tokens":100,"generated_tokens":4},
                "error_code":null
            }))
            .unwrap();
        assert!(super::export_eval_selection(
            receiver.endpoint(),
            "selection-run",
            "selector-1",
            &selection
        ));
        let observed = receiver.task("selector-1", 1).unwrap();
        assert_eq!(observed.selection_observations, 1);
        assert_eq!(observed.selection_candidate_count, Some(4));
        assert_eq!(observed.selection_selected_count, Some(1));
        assert_eq!(observed.selection_input_tokens, Some(100));
        assert_eq!(observed.selection_output_tokens, Some(4));
        assert_eq!(observed.selection_duration_ms, Some(12.0));
        assert_eq!(observed.tool_call_spans, 0);
        assert!(!observed.mcp_observed);
        assert!(
            !serde_json::to_string(&observed)
                .unwrap()
                .contains("private-marker")
        );
        for endpoint in [
            "https://127.0.0.1:4318",
            "http://example.invalid:4318",
            "http://user@127.0.0.1:4318",
            "http://127.0.0.1:4318/?secret=x",
        ] {
            assert!(!super::export_eval_selection(
                endpoint,
                "selection-run",
                "selector-1",
                &selection
            ));
        }
    }
}

/// Export a genuine typed grading attempt with only status, duration and usage.
pub(crate) fn export_eval_decision(
    endpoint: &str,
    run_id: &str,
    evaluator_id: &str,
    observation: &crate::eval::decision::Observation,
) -> bool {
    use opentelemetry_proto::tonic::{
        collector::trace::v1::ExportTraceServiceRequest,
        common::v1::{AnyValue, KeyValue as ProtoKeyValue, any_value},
        resource::v1::Resource as ProtoResource,
        trace::v1::{ResourceSpans, ScopeSpans, Span as ProtoSpan},
    };
    use opentelemetry_sdk::trace::{IdGenerator, RandomIdGenerator};
    use prost::Message;
    if observation.calls_attempted != 1 {
        return false;
    }
    let Ok(url) = url::Url::parse(endpoint) else {
        return false;
    };
    if url.scheme() != "http"
        || !url.host_str().is_some_and(|h| {
            h.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        })
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let attr = |key: &str, value: any_value::Value| ProtoKeyValue {
        key: key.into(),
        value: Some(AnyValue { value: Some(value) }),
        ..Default::default()
    };
    let string = |key: &str, value: &str| attr(key, any_value::Value::StringValue(value.into()));
    let integer = |key: &str, value: u64| {
        attr(
            key,
            any_value::Value::IntValue(value.min(i64::MAX as u64) as i64),
        )
    };
    let mut attributes = vec![
        string("ahu.eval.decision.status", &observation.status),
        integer("ahu.eval.decision.calls", observation.calls_attempted),
        attr(
            "ahu.eval.decision.duration_ms",
            any_value::Value::DoubleValue(observation.elapsed_ms.unwrap_or_default()),
        ),
    ];
    if let Some(service) = &observation.service {
        for (field, key) in [
            ("prompt_tokens", "ahu.eval.decision.tokens.input"),
            ("generated_tokens", "ahu.eval.decision.tokens.output"),
        ] {
            if let Some(value) = service[field].as_u64() {
                attributes.push(integer(key, value));
            }
        }
    }
    let ids = RandomIdGenerator::default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos().min(u64::MAX as u128) as u64);
    // Construct the wire resource explicitly: SDK Resource::builder and the
    // default OTLP exporter both import unrelated inherited environment data.
    let message = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(ProtoResource {
                attributes: vec![
                    string("service.name", "ahu-eval"),
                    string("ahu.eval.run_id", run_id),
                    string("ahu.eval.stage", "evaluator"),
                    string("ahu.task.id", evaluator_id),
                    string("ahu.task.attempt", "1"),
                ],
                ..Default::default()
            }),
            scope_spans: vec![ScopeSpans {
                spans: vec![ProtoSpan {
                    trace_id: ids.new_trace_id().to_bytes().to_vec(),
                    span_id: ids.new_span_id().to_bytes().to_vec(),
                    name: "ahu.eval.typed_decision".into(),
                    start_time_unix_nano: now.saturating_sub(
                        (observation.elapsed_ms.unwrap_or_default() * 1_000_000.0) as u64,
                    ),
                    end_time_unix_nano: now,
                    attributes,
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }],
    };
    // Never consult inherited proxy/OTLP headers; never follow redirects.
    let Ok(client) = reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_millis(500))
        .build()
    else {
        return false;
    };
    client
        .post(format!("{}/v1/traces", endpoint.trim_end_matches('/')))
        .header(reqwest::header::CONTENT_TYPE, "application/x-protobuf")
        .body(message.encode_to_vec())
        .send()
        .is_ok_and(|response| response.status().is_success())
}

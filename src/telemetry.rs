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
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig, WithHttpConfig};
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
const SPAN_EXPORT_TIMEOUT: Duration = Duration::from_millis(500);

// Expecting the mutex is correct for telemetry because poisoning only occurs
// after a panic in this process, and halting is the intended fail-stop behavior.
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

pub(crate) fn is_valid_local_endpoint(endpoint: &url::Url) -> bool {
    let local = matches!(
        endpoint.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    endpoint.scheme() == "http"
        && local
        && endpoint.username().is_empty()
        && endpoint.password().is_none()
        && endpoint.query().is_none()
        && endpoint.fragment().is_none()
}

pub fn validate_config(config: &TelemetryConfig, path: &Path) -> Result<()> {
    let endpoint = config.endpoint.parse::<url::Url>().map_err(|error| {
        Error::new(format!(
            "{}: telemetry.endpoint is not a valid URL: {error}",
            path.display()
        ))
    })?;
    if !crate::telemetry::is_valid_local_endpoint(&endpoint) {
        return Err(Error::new(format!(
            "{}: telemetry.endpoint must be a local http origin without credentials, query, or fragment",
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
    initialize_named(config, "ahu", None)
}

/// Initialize the independently launched stdio MCP server as its own service.
pub(crate) fn initialize_mcp(config: &TelemetryConfig) -> Result<()> {
    initialize_named(config, "ahu-mcp", None)
}

fn initialize_named(
    config: &TelemetryConfig,
    service_name: &'static str,
    identity: Option<Vec<(&'static str, String)>>,
) -> Result<()> {
    let eval_endpoint = eval_endpoint_override();
    let enabled = config.enabled || eval_endpoint.is_some();
    if !enabled
        || provider_slot()
            .lock()
            .expect("telemetry mutex poisoned")
            .is_some()
    {
        return Ok(());
    }
    // Config loading validates this too, but direct library callers must not
    // bypass the same local collector boundary. An invalid eval override alone
    // must not enable export to the default collector.
    if eval_endpoint.is_none() && validate_config(config, Path::new("telemetry")).is_err() {
        eprintln!("ahu: invalid telemetry endpoint; continuing without export");
        return Ok(());
    }
    let endpoint = eval_endpoint.unwrap_or_else(|| config.endpoint.clone());
    // Pin routing and deadlines independently of the inherited environment.
    // Keep the SDK's documented header/resource inheritance at the collector.
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(SPAN_EXPORT_TIMEOUT)
        .timeout(SPAN_EXPORT_TIMEOUT)
        .build();
    let Ok(client) = client else {
        eprintln!("ahu: OTLP transport unavailable; continuing without export");
        return Ok(());
    };
    let exporter = SpanExporter::builder()
        .with_http()
        .with_http_client(client)
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(format!("{}/v1/traces", endpoint.trim_end_matches('/')))
        .with_timeout(SPAN_EXPORT_TIMEOUT)
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
    // SDK detectors retain non-ahu resource configuration. Rebuild ahu identity
    // from one bounded source so an absent frozen field cannot fall through to
    // an ambient value (including on independently launched MCP servers).
    let headless_attempt = service_name == "ahu"
        && identity.as_ref().is_some_and(|attributes| {
            attributes
                .iter()
                .any(|(key, value)| *key == "ahu.execution.mode" && value == "headless")
        });
    let attributes = identity.unwrap_or_else(|| {
        if service_name == "ahu-mcp" {
            std::env::var("AHU_MCP_RESOURCE_ATTRIBUTES")
                .ok()
                .map(|raw| parse_ahu_resource_attributes(&raw))
                .unwrap_or_else(ahu_resource_attributes)
        } else {
            ahu_resource_attributes()
        }
    });
    let resource = if headless_attempt {
        // Headless invocation evidence has no ambient resource payload. Keep
        // paths, arbitrary labels and credentials out of Ahu-owned exports.
        Resource::builder_empty()
            .with_service_name(service_name)
            .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
            .with_attributes(
                attributes
                    .into_iter()
                    .map(|(key, value)| KeyValue::new(key, value)),
            )
            .build()
    } else {
        identity_resource(service_name, attributes)
    };
    // Keep export asynchronous and bounded, and override OTEL_BSP_* values so
    // an inherited environment cannot turn a local ahu process into an
    // unbounded queue or stall task execution. Full queues drop telemetry.
    let processor = bounded_span_processor(exporter);
    let mut builder = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_span_processor(processor);
    if headless_attempt {
        // Every owned attempt needs a terminal observation, even if an ambient
        // SDK sampler or attribute limit would otherwise erase that evidence.
        builder = builder
            .with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOn)
            .with_span_limits(opentelemetry_sdk::trace::SpanLimits::default());
    }
    let provider = builder.build();
    global::set_tracer_provider(provider.clone());
    *provider_slot().lock().expect("telemetry mutex poisoned") = Some(provider);
    Ok(())
}

fn identity_resource(
    service_name: &'static str,
    attributes: Vec<(&'static str, String)>,
) -> Resource {
    let inherited = Resource::builder().build();
    Resource::builder_empty()
        .with_attributes(
            inherited
                .iter()
                .filter(|(key, _)| !key.as_str().starts_with("ahu."))
                .map(|(key, value)| KeyValue::new(key.clone(), value.clone())),
        )
        .with_service_name(service_name)
        .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
        .with_attributes(
            attributes
                .into_iter()
                .map(|(key, value)| KeyValue::new(key, value)),
        )
        .build()
}

/// Freeze the operator's release label before launching a task. An explicitly
/// supplied invalid or ambiguous label must not silently lose attribution.
pub(crate) fn release_id_for_launch() -> Result<Option<String>> {
    match std::env::var("OTEL_RESOURCE_ATTRIBUTES") {
        Ok(raw) => release_id_from_attributes(&raw),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(Error::new(
            "OTEL_RESOURCE_ATTRIBUTES must be UTF-8 for task launch",
        )),
    }
}

fn release_id_from_attributes(raw: &str) -> Result<Option<String>> {
    let (attributes, release_keys) = parse_attributes_with_release_count(raw);
    let release = attributes
        .into_iter()
        .find_map(|(key, value)| (key == "ahu.release.id").then_some(value));
    if release_keys > 0 && release.is_none() {
        return Err(Error::new(
            "invalid or repeated ahu.release.id in OTEL_RESOURCE_ATTRIBUTES; supply one opaque release label of 1-64 ASCII letters, digits, dots, underscores or hyphens before launching",
        ));
    }
    Ok(release)
}

pub(crate) fn deserialize_release_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    use serde::Deserialize;
    Ok(Option::<String>::deserialize(deserializer)?
        .filter(|value| safe_resource_value("ahu.release.id", value)))
}

fn task_resource_attributes(
    record: &crate::task::TaskRecord,
    spec: Option<&crate::headless::Spec>,
    ambient: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut attributes = ambient
        .map(parse_ahu_resource_attributes)
        .unwrap_or_default();
    // Only eval correlation survives from the ambient environment. All task
    // identity, including absence of release/parent on old tasks, is frozen.
    attributes.retain(|(key, _)| key.starts_with("ahu.eval."));
    attributes.extend(parse_ahu_resource_attributes(&resource_attributes(
        &record.identity.agent,
        &record.identity.harness,
        &record.identity.model,
        record.identity.agent_version.as_deref(),
        spec.map(|spec| spec.harness_version.as_str())
            .or(record.enforcement.harness_version.as_deref()),
        Some(&record.task_id),
        Some(spec.map_or(1, |spec| spec.attempt)),
        None,
    )));
    for (key, value) in [
        ("ahu.release.id", record.identity.release_id.as_deref()),
        (
            "ahu.task.root_id",
            spec.and_then(|spec| spec.root_task.as_deref()).or_else(|| {
                // Legacy children without a root record have unknown root identity.
                spec.filter(|spec| spec.parent_task.is_some())
                    .is_none()
                    .then_some(record.task_id.as_str())
            }),
        ),
        (
            "ahu.task.parent_id",
            spec.and_then(|spec| spec.parent_task.as_deref()),
        ),
        (
            "ahu.execution.mode",
            Some(if spec.is_some() {
                "headless"
            } else {
                "interactive"
            }),
        ),
    ] {
        if let Some(value) = value.filter(|value| safe_resource_value(key, value)) {
            attributes.push((key, value.to_string()));
        }
    }
    attributes
}

pub(crate) fn initialize_task(
    config: &TelemetryConfig,
    record: &crate::task::TaskRecord,
    spec: Option<&crate::headless::Spec>,
) -> Result<()> {
    initialize_named(
        config,
        "ahu",
        Some(task_resource_attributes(
            record,
            spec,
            std::env::var("OTEL_RESOURCE_ATTRIBUTES").ok().as_deref(),
        )),
    )
}

/// Explicitly carry recorded identity across process boundaries, even when
/// export is off. This does not opt a harness into native instrumentation.
pub(crate) fn configure_task_identity(
    command: &mut std::process::Command,
    record: &crate::task::TaskRecord,
    spec: Option<&crate::headless::Spec>,
) {
    let attributes = task_resource_attributes(
        record,
        spec,
        std::env::var("OTEL_RESOURCE_ATTRIBUTES").ok().as_deref(),
    );
    let encoded = attributes
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",");
    command
        .env("OTEL_RESOURCE_ATTRIBUTES", &encoded)
        .env("AHU_MCP_RESOURCE_ATTRIBUTES", encoded);
}

pub(crate) fn task_span_attributes(
    span: &mut SpanGuard,
    record: &crate::task::TaskRecord,
    spec: Option<&crate::headless::Spec>,
) {
    for (key, value) in task_resource_attributes(record, spec, None) {
        if key == "ahu.task.attempt" {
            span.set_u64(key, u64::from(spec.map_or(1, |spec| spec.attempt)));
        } else {
            span.set_string(key, value);
        }
    }
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
    parse_attributes_with_release_count(raw).0
}

fn parse_attributes_with_release_count(raw: &str) -> (Vec<(&'static str, String)>, usize) {
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
        "ahu.task.root_id",
        "ahu.task.parent_id",
        "ahu.execution.mode",
        "ahu.release.id",
        "ahu.task.attempt",
        "ahu.version",
    ];
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut release_keys = 0;
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
            if pair.trim() == "ahu.release.id" {
                release_keys += 1;
            }
            continue;
        };
        // Match the SDK's surrounding-whitespace handling for standard
        // OTEL_RESOURCE_ATTRIBUTES, including duplicate release detection.
        let key = key.trim();
        let value = value.trim();
        if key == "ahu.release.id" {
            release_keys += 1;
        }
        if let Some(&allowed_key) = ALLOWED.iter().find(|&&allowed| allowed == key)
            && safe_resource_value(allowed_key, value)
            && seen.insert(allowed_key.to_string())
        {
            out.push((allowed_key, value.to_string()));
        }
    }
    if release_keys > 1 {
        out.retain(|(key, _)| *key != "ahu.release.id");
    }
    (out, release_keys)
}

fn safe_resource_value(key: &str, value: &str) -> bool {
    if value.is_empty() || value.len() > 128 || !value.is_ascii() {
        return false;
    }
    if key == "ahu.task.attempt" {
        return value.bytes().all(|byte| byte.is_ascii_digit());
    }
    if key == "ahu.execution.mode" {
        return matches!(value, "headless" | "interactive");
    }
    if key == "ahu.release.id" || key.starts_with("ahu.eval.") {
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
/// Trust boundary: attributes emitted by child processes are redacted only at the
/// local collector; the child environment is NOT a trust boundary for attribute content.
/// See docs/reference.md ~:906-914 and docs/knowledge/telemetry-evidence.md ~:74-76.
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
    command
        .env_remove("AHU_EVAL_LOCAL_METRICS")
        // Never forward a previous task's parent context or MCP identity.
        .env_remove("AHU_MCP_RESOURCE_ATTRIBUTES")
        .env_remove("AHU_BROKER_PARENT_ATTRIBUTES")
        .env_remove("HTTP_PROXY")
        .env_remove("http_proxy")
        .env_remove("HTTPS_PROXY")
        .env_remove("https_proxy")
        .env_remove("ALL_PROXY")
        .env_remove("all_proxy")
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
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
        .env_remove("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT");

    let eval_endpoint = eval_endpoint_override();
    let eval_mode = eval_endpoint.is_some();
    let Some(endpoint_str) =
        eval_endpoint.or_else(|| config.enabled.then(|| config.endpoint.clone()))
    else {
        return;
    };
    let Ok(endpoint_url) = endpoint_str.parse::<url::Url>() else {
        eprintln!("ahu: invalid telemetry endpoint (not a URL); continuing without export");
        return;
    };
    if !crate::telemetry::is_valid_local_endpoint(&endpoint_url) {
        eprintln!("ahu: invalid telemetry endpoint; continuing without export");
        return;
    }

    let ambient = std::env::var("OTEL_RESOURCE_ATTRIBUTES").ok();
    let safe_ambient = ambient.as_deref().and_then(|raw| {
        let mut parsed = parse_ahu_resource_attributes(raw);
        parsed.retain(|(key, _)| key.starts_with("ahu.eval.") || *key == "ahu.release.id");
        if parsed.is_empty() && !raw.trim().is_empty() {
            eprintln!("ahu: ambient OTEL_RESOURCE_ATTRIBUTES malformed or not allowed, dropping");
            None
        } else if parsed.is_empty() {
            None
        } else {
            Some(
                parsed
                    .into_iter()
                    .map(|(k, v)| format!("{k}={}", escape(&v)))
                    .collect::<Vec<_>>()
                    .join(","),
            )
        }
    });

    let attributes = resource_attributes(
        agent,
        harness,
        model,
        agent_version,
        harness_version,
        task_id,
        attempt,
        safe_ambient.as_deref(),
    );
    let mcp_attributes = parse_ahu_resource_attributes(&attributes)
        .into_iter()
        .map(|(key, value)| format!("{key}={}", escape(&value)))
        .collect::<Vec<_>>()
        .join(",");
    command
        .env("AHU_MCP_RESOURCE_ATTRIBUTES", mcp_attributes)
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint_str)
        .env("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf")
        .env("OTEL_EXPORTER_OTLP_TIMEOUT", "500")
        .env("OTEL_TRACES_EXPORTER", "otlp")
        // The eval receiver implements only ExportTraceService. Ordinary
        // collectors retain all supported signals. These settings do not
        // enable native instrumentation in a harness that does not have it.
        .env(
            "OTEL_METRICS_EXPORTER",
            if eval_mode { "none" } else { "otlp" },
        )
        .env(
            "OTEL_LOGS_EXPORTER",
            if eval_mode { "none" } else { "otlp" },
        )
        .env("OTEL_SERVICE_NAME", "ahu-agent")
        .env("OTEL_RESOURCE_ATTRIBUTES", attributes);
    if eval_mode && harness == "kiro" {
        // Kiro V3 emits native OTLP JSON traces/metrics even when told to use
        // protobuf and disable metrics. The eval receiver projects ahu's
        // protobuf spans only. Keep its endpoint in AHU_EVAL_OTEL_ENDPOINT
        // for ahu/MCP, without opting Kiro into incompatible native exports.
        command
            .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
            .env("OTEL_TRACES_EXPORTER", "none");
    }
}

/// A director has no frozen task record. Carry its bounded release label for
/// future task creation and apply the same export policy on either cmux path.
pub(crate) fn configure_coordinator(
    command: &mut std::process::Command,
    config: &TelemetryConfig,
    harness: &str,
    model: &str,
    harness_version: Option<&str>,
) {
    configure_child(
        command,
        config,
        "director",
        harness,
        model,
        None,
        harness_version,
        None,
        None,
    );
    if let Some(endpoint) = eval_endpoint_override() {
        command.env("AHU_EVAL_OTEL_ENDPOINT", endpoint);
    } else {
        // A relocated workspace must not pick up a stale cmux-server receiver.
        command.env_remove("AHU_EVAL_OTEL_ENDPOINT");
    }
    let mut inherited = ahu_resource_attributes();
    inherited.retain(|(key, _)| key.starts_with("ahu.eval.") || *key == "ahu.release.id");
    let inherited = inherited
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut identity = parse_ahu_resource_attributes(&resource_attributes(
        "director",
        harness,
        model,
        None,
        harness_version,
        None,
        None,
        Some(&inherited),
    ));
    identity.push(("ahu.execution.mode", "interactive".to_string()));
    let encoded = identity
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(",");
    command
        .env("OTEL_RESOURCE_ATTRIBUTES", &encoded)
        .env("AHU_MCP_RESOURCE_ATTRIBUTES", encoded);
}

/// Evaluation capture is an internal, loopback-only override. It lets a
/// single eval run receive OTLP directly, without requiring a collector
/// backend or changing the project's checked-in telemetry policy.
pub(crate) fn eval_endpoint_override() -> Option<String> {
    let raw = std::env::var("AHU_EVAL_OTEL_ENDPOINT").ok()?;
    let endpoint = raw.parse::<url::Url>().ok()?;
    if !crate::telemetry::is_valid_local_endpoint(&endpoint) || endpoint.port().is_none() {
        return None;
    }
    Some(endpoint.to_string())
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

/// A bounded metadata carrier already forwarded by ahu's MCP configuration.
/// The reserved field is deliberately absent from the resource allowlist: it
/// is consumed as parent context, never exported as a resource attribute.
/// Matching task identity prevents accidental cross-task reuse, not forgery by
/// same-user code. This is correlation metadata, never authorization evidence.
mod causal_context {
    use std::collections::HashMap;
    use std::process::Command;

    use opentelemetry::Context;
    use opentelemetry::propagation::TextMapPropagator;
    use opentelemetry::trace::{SpanContext, TraceContextExt};
    use opentelemetry_sdk::propagation::TraceContextPropagator;

    const ENV: &str = "AHU_MCP_RESOURCE_ATTRIBUTES";
    const PARENT: &str = "ahu.internal.traceparent";
    const MAX_CARRIER_BYTES: usize = 4096;

    fn fields(raw: &str) -> Option<HashMap<&str, &str>> {
        if raw.len() > MAX_CARRIER_BYTES || !raw.is_ascii() || raw.contains('\\') {
            return None;
        }
        let mut fields = HashMap::new();
        for pair in raw.split(',') {
            let (key, value) = pair.split_once('=')?;
            if (key != PARENT && !super::safe_resource_value(key, value))
                || fields.insert(key, value).is_some()
            {
                return None;
            }
        }
        let task = *fields.get("ahu.task.id")?;
        if !super::safe_resource_value("ahu.task.id", task) {
            return None;
        }
        let attempt = fields.get("ahu.task.attempt")?;
        if attempt.parse::<u32>().ok()? == 0 {
            return None;
        }
        Some(fields)
    }

    pub(super) fn extract(raw: Option<&str>, task: Option<&str>, attempt: Option<u32>) -> Context {
        let root = Context::new();
        let Some(fields) = raw.and_then(fields) else {
            return root;
        };
        if fields.get("ahu.task.id").copied() != task
            || fields
                .get("ahu.task.attempt")
                .and_then(|s| s.parse::<u32>().ok())
                != attempt
            || attempt.is_none_or(|n| n == 0)
        {
            return root;
        }
        let Some(parent) = fields.get(PARENT).copied() else {
            return root;
        };
        // Accept only the exact version-00 form ahu emits, with no extensions,
        // whitespace, uppercase, extra flags, tracestate, or baggage.
        if parent.len() != 55
            || !parent.starts_with("00-")
            || !matches!(&parent[53..], "00" | "01")
            || parent.bytes().enumerate().any(|(index, byte)| {
                if matches!(index, 2 | 35 | 52) {
                    byte != b'-'
                } else {
                    !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte)
                }
            })
        {
            return root;
        }
        let carrier = HashMap::from([("traceparent".to_string(), parent.to_string())]);
        TraceContextPropagator::new().extract_with_context(&root, &carrier)
    }

    pub(super) fn from_environment() -> Context {
        let raw = std::env::var(ENV).ok();
        let task = std::env::var("AHU_TASK_ID").ok();
        let frozen = std::env::var("OTEL_RESOURCE_ATTRIBUTES").ok();
        let attempt = frozen.as_deref().and_then(fields).and_then(|f| {
            (f.get("ahu.task.id").copied() == task.as_deref())
                .then(|| f.get("ahu.task.attempt").and_then(|s| s.parse().ok()))
                .flatten()
        });
        extract(raw.as_deref(), task.as_deref(), attempt)
    }

    pub(super) fn inject(command: &mut Command, span: &SpanContext) {
        if !span.is_valid() {
            return;
        }
        // Read only configure_child's explicit command env, not inherited env.
        let Some(raw) = command
            .get_envs()
            .find_map(|(key, value)| (key == ENV).then_some(value).flatten())
            .and_then(|value| value.to_str())
        else {
            return;
        };
        if fields(raw).is_none() {
            return;
        }
        let mut carrier = HashMap::new();
        TraceContextPropagator::new().inject_context(
            &Context::new().with_remote_span_context(span.clone()),
            &mut carrier,
        );
        let Some(parent) = carrier.get("traceparent") else {
            return;
        };
        // Rebuild from the resource allowlist so repeated calls replace the
        // reserved field and cannot carry arbitrary attributes into MCP.
        let attributes = super::parse_ahu_resource_attributes(raw)
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(",");
        let attributes = format!("{attributes},{PARENT}={parent}");
        if attributes.len() <= MAX_CARRIER_BYTES {
            command.env(ENV, attributes);
        }
    }
}

impl SpanGuard {
    pub(crate) fn task_parent_carrier(&self, task: &str, attempt: u32) -> Option<String> {
        if !safe_resource_value("ahu.task.id", task) || attempt == 0 {
            return None;
        }
        let mut command = std::process::Command::new("unused");
        command.env(
            "AHU_MCP_RESOURCE_ATTRIBUTES",
            format!("ahu.task.id={task},ahu.task.attempt={attempt}"),
        );
        self.configure_child_context(&mut command);
        command
            .get_envs()
            .find_map(|(k, v)| (k == "AHU_MCP_RESOURCE_ATTRIBUTES").then_some(v).flatten())
            .and_then(|v| v.to_str())
            .filter(|s| s.contains("ahu.internal.traceparent="))
            .map(str::to_owned)
    }
    /// Propagate this span explicitly after configure_child. No context is
    /// attached to a thread or held across a future's suspension points.
    pub(crate) fn configure_child_context(&self, command: &mut std::process::Command) {
        if let Some(span) = &self.span {
            causal_context::inject(command, span.span_context());
        }
    }

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
    span_with_context(name, attributes, &causal_context::from_environment())
}

fn span_with_context(
    name: &'static str,
    attributes: impl IntoIterator<Item = (&'static str, String)>,
    context: &opentelemetry::Context,
) -> SpanGuard {
    let tracer = global::tracer("ahu");
    let mut span = tracer.start_with_context(name, context);
    for (key, value) in attributes {
        span.set_attribute(KeyValue::new(key, Value::from(value)));
    }
    SpanGuard {
        span: Some(span),
        started: Instant::now(),
    }
}

/// Child parentage is bound to the frozen supervisor spec, not harness input.
pub(crate) fn task_span(
    name: &'static str,
    attributes: impl IntoIterator<Item = (&'static str, String)>,
    spec: &crate::headless::Spec,
) -> SpanGuard {
    let carrier = std::env::var("AHU_BROKER_PARENT_ATTRIBUTES").ok();
    let context = causal_context::extract(
        carrier.as_deref(),
        spec.parent_task.as_deref(),
        spec.parent_attempt,
    );
    span_with_context(name, attributes, &context)
}

pub(crate) fn configure_supervisor_parent(
    command: &mut std::process::Command,
    spec: &crate::headless::Spec,
) {
    use opentelemetry::trace::TraceContextExt;
    command.env_remove("AHU_BROKER_PARENT_ATTRIBUTES");
    let carrier = std::env::var("AHU_BROKER_PARENT_ATTRIBUTES").ok();
    let context = causal_context::extract(
        carrier.as_deref(),
        spec.parent_task.as_deref(),
        spec.parent_attempt,
    );
    if context.span().span_context().is_valid() {
        // Only the minimal broker-owned carrier is propagated; never baggage.
        let span = context.span();
        let sc = span.span_context();
        command.env(
            "AHU_BROKER_PARENT_ATTRIBUTES",
            format!(
                "ahu.task.id={},ahu.task.attempt={},ahu.internal.traceparent=00-{}-{}-{:02x}",
                spec.parent_task.as_deref().unwrap(),
                spec.parent_attempt.unwrap(),
                sc.trace_id(),
                sc.span_id(),
                sc.trace_flags().to_u8()
            ),
        );
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
        | "skills/list"
        | "skills/get"
        | "resources/list"
        | "resources/read"
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
    if name == "ahu_request_approval"
        && let Some(operation) = arguments
            .get("operation")
            .and_then(serde_json::Value::as_str)
            .filter(|operation| {
                ["external-write", "network", "destructive", "other"].contains(operation)
            })
    {
        attributes.push(KeyValue::new(
            "ahu.mcp.approval.operation",
            operation.to_owned(),
        ));
    }
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
    for (input, output) in [("model", "ahu.mcp.decision.model")] {
        if let Some(value) = service.get(input).and_then(serde_json::Value::as_str)
            && crate::eval::provenance::safe_identity(value)
        {
            attributes.push(KeyValue::new(output, value.to_string()));
        }
    }
    for (suffix, value) in decision_identity_fields(service) {
        attributes.push(KeyValue::new(
            format!("ahu.mcp.decision.{suffix}"),
            value.to_owned(),
        ));
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

fn decision_identity_fields(
    service: &serde_json::Map<String, serde_json::Value>,
) -> Vec<(&'static str, &str)> {
    let mut fields = Vec::new();
    for (input, suffix) in [
        ("backend", "backend"),
        ("requested_model", "model.requested"),
    ] {
        if let Some(value) = service.get(input).and_then(serde_json::Value::as_str)
            && crate::eval::provenance::safe_identity(value)
        {
            fields.push((suffix, value));
        }
    }
    if service
        .get("model_reported")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
        && let Some(value) = service.get("model").and_then(serde_json::Value::as_str)
        && crate::eval::provenance::safe_identity(value)
    {
        fields.push(("model.reported", value));
    }
    fields
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
            || message.starts_with("AHU_DECISION_MODEL must")
            || message.starts_with("invalid AHU_OLLAMA_URL")
            || message.starts_with("AHU_OLLAMA_URL ")
            || message.starts_with("AHU_OLLAMA_MODEL ")
            || message.starts_with("AHU_OLLAMA_MODEL/AHU_OLLAMA_URL ")
            || message.starts_with("cannot create Ollama decision client")
            || message.starts_with("cannot create decision client")
            || message.starts_with("cannot create TypeSafe decision client")
        {
            "decision_configuration"
        } else if message.starts_with("decision service request failed")
            || message.starts_with("TypeSafe decision request failed")
            || message.starts_with("Ollama decision request failed")
        {
            "decision_service_unavailable"
        } else if message.starts_with("decision service returned HTTP")
            || message.starts_with("TypeSafe decision API returned HTTP")
            || message.starts_with("Ollama decision API returned HTTP")
        {
            "decision_service_http_error"
        } else if message.starts_with("decision service returned invalid JSON")
            || message.starts_with("decision service response")
            || message.starts_with("decision service answers")
            || message.starts_with("TypeSafe decision response")
            || message.starts_with("Ollama decision response")
            || message.starts_with("cannot read Ollama decision response")
            || message.starts_with("cannot read TypeSafe decision response")
            || message.starts_with("cannot read decision response")
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

/// Run ordinary-collector assertions without inheriting eval capture or mutating
/// the environment shared by parallel tests. Return true in the parent process.
#[cfg(test)]
pub(crate) fn test_without_eval_endpoint(test_name: &str) -> bool {
    const CHILD: &str = "AHU_TEST_WITHOUT_EVAL_ENDPOINT";
    if std::env::var(CHILD).as_deref() == Ok(test_name) {
        assert!(std::env::var_os("AHU_EVAL_OTEL_ENDPOINT").is_none());
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .env_clear()
        .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|value| ("LLVM_PROFILE_FILE", value)))
        .env(CHILD, test_name)
        .args(["--exact", test_name, "--nocapture"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("test result: ok. 1 passed;"),
        "{test_name}: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    true
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
        if super::test_without_eval_endpoint(
            "telemetry::tests::local_metrics_are_opt_in_numeric_and_do_not_infer_totals",
        ) {
            return;
        }
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
        // Stripping is fail-closed: sensitive and proxy variables are removed
        // even when telemetry is disabled, and nothing is set.
        let envs: std::collections::HashMap<_, _> = child.get_envs().collect();
        for key in [
            "AHU_EVAL_LOCAL_METRICS",
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
            "no_proxy",
            "OTEL_EXPORTER_OTLP_HEADERS",
            "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
            "OTEL_EXPORTER_OTLP_METRICS_HEADERS",
            "OTEL_EXPORTER_OTLP_LOGS_HEADERS",
            "OTEL_EXPORTER_OTLP_CERTIFICATE",
            "OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE",
            "OTEL_EXPORTER_OTLP_CLIENT_KEY",
            "OTEL_EXPORTER_OTLP_COMPRESSION",
            "OTEL_EXPORTER_OTLP_TRACES_COMPRESSION",
            "OTEL_EXPORTER_OTLP_METRICS_COMPRESSION",
            "OTEL_EXPORTER_OTLP_LOGS_COMPRESSION",
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
            "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
        ] {
            assert!(
                envs.get(std::ffi::OsStr::new(key))
                    .map(|v| v.is_none())
                    .unwrap_or(false),
                "{key} should be stripped even when telemetry is disabled"
            );
        }
        assert!(
            envs.iter().all(|(_, value)| value.is_none()),
            "no environment variables should be set when telemetry is disabled"
        );
    }

    #[test]
    fn local_endpoint_is_accepted_and_remote_is_rejected() {
        for endpoint in [
            "http://localhost:4318",
            "http://127.0.0.1:4318/",
            "http://[::1]:4318",
            "http://127.0.0.1:4318/v1/traces",
        ] {
            let config = TelemetryConfig {
                endpoint: endpoint.into(),
                ..TelemetryConfig::default()
            };
            validate_config(&config, Path::new("config.toml")).unwrap();
        }
        for endpoint in [
            "https://collector.example.test:443",
            "http://collector.example.test:4318",
            "http://[::2]:4318",
            "http://127.0.0.1:4319",
            "http://127.0.0.1",
            "http://synthetic-user:synthetic-password@127.0.0.1:4318",
            "http://synthetic-user@127.0.0.1:4318",
            "http://:synthetic-password@[::1]:4318",
            "http://127.0.0.1:4318/?synthetic-query",
            "http://127.0.0.1:4318/#synthetic-fragment",
        ] {
            let config = TelemetryConfig {
                endpoint: endpoint.into(),
                ..TelemetryConfig::default()
            };
            let error = validate_config(&config, Path::new("config.toml"))
                .expect_err("only a local collector origin is valid");
            assert!(!error.to_string().contains("synthetic-"));
        }
    }

    #[test]
    fn child_environment_proxy_vars_are_absent() {
        let mut command = std::process::Command::new("true");
        for key in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
            "no_proxy",
        ] {
            command.env(key, "synthetic-proxy");
        }
        let enabled = TelemetryConfig {
            enabled: true,
            endpoint: "http://127.0.0.1:4318".into(),
            ..TelemetryConfig::default()
        };
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
        let envs: std::collections::HashMap<_, _> = command.get_envs().collect();
        for key in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
            "no_proxy",
        ] {
            assert!(
                envs.get(std::ffi::OsStr::new(key))
                    .map(|v| v.is_none())
                    .unwrap_or(true),
                "{} should be stripped",
                key
            );
        }
    }

    #[test]
    fn child_environment_invalid_endpoint_no_export() {
        if super::test_without_eval_endpoint(
            "telemetry::tests::child_environment_invalid_endpoint_no_export",
        ) {
            return;
        }
        for endpoint in [
            "https://127.0.0.1:4318",
            "http://example.com:4318",
            "http://user@127.0.0.1:4318",
            "http://127.0.0.1:4318/?query=1",
            "http://127.0.0.1:4318/#frag",
        ] {
            let mut command = std::process::Command::new("true");
            let enabled = TelemetryConfig {
                enabled: true,
                endpoint: endpoint.into(),
                ..TelemetryConfig::default()
            };
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
            assert!(
                command
                    .get_envs()
                    .all(|(key, _)| key != "OTEL_EXPORTER_OTLP_ENDPOINT")
            );
        }
        // Test token path endpoint is accepted
        let mut command = std::process::Command::new("true");
        let enabled = TelemetryConfig {
            enabled: true,
            endpoint: "http://127.0.0.1:4318/token123/v1/traces".into(),
            ..TelemetryConfig::default()
        };
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
        let envs: std::collections::HashMap<_, _> = command.get_envs().collect();
        assert!(envs.contains_key(std::ffi::OsStr::new("OTEL_EXPORTER_OTLP_ENDPOINT")));
    }

    #[test]
    fn parse_ahu_resource_attributes_does_not_panic_on_valid_inputs() {
        let parsed =
            super::parse_ahu_resource_attributes("ahu.task.id=123,ahu.version=1.0,unknown=value");
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn child_environment_is_only_changed_when_enabled() {
        if super::test_without_eval_endpoint(
            "telemetry::tests::child_environment_is_only_changed_when_enabled",
        ) {
            return;
        }
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
        assert_eq!(env["OTEL_TRACES_EXPORTER"], "otlp");
        assert_eq!(env["OTEL_METRICS_EXPORTER"], "otlp");
        assert_eq!(env["OTEL_LOGS_EXPORTER"], "otlp");
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
    fn child_environment_enables_otlp_for_all_signals() {
        if super::test_without_eval_endpoint(
            "telemetry::tests::child_environment_enables_otlp_for_all_signals",
        ) {
            return;
        }
        let enabled = TelemetryConfig {
            enabled: true,
            ..TelemetryConfig::default()
        };
        let mut command = std::process::Command::new("true");
        command.env("OTEL_METRICS_EXPORTER", "none");
        command.env("OTEL_LOGS_EXPORTER", "none");
        configure_child(
            &mut command,
            &enabled,
            "agent",
            "codex",
            "model",
            None,
            None,
            None,
            None,
        );
        let env: std::collections::BTreeMap<_, _> = command
            .get_envs()
            .filter_map(|(key, value)| Some((key.to_str()?, value?.to_str()?)))
            .collect();
        assert_eq!(env["OTEL_TRACES_EXPORTER"], "otlp");
        assert_eq!(env["OTEL_METRICS_EXPORTER"], "otlp");
        assert_eq!(env["OTEL_LOGS_EXPORTER"], "otlp");
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
    fn approval_telemetry_records_only_the_bounded_operation_category() {
        let input = serde_json::json!({
            "operation":"network",
            "summary":"private approval summary",
            "target":"https://private.invalid/path"
        });
        let attributes = mcp_tool_attributes("ahu_request_approval", &input);
        assert_eq!(
            string_attr(&attributes, "ahu.mcp.approval.operation"),
            Some("network".into())
        );
        let encoded = format!("{attributes:?}");
        assert!(!encoded.contains("private approval summary"));
        assert!(!encoded.contains("private.invalid"));
        let invalid = mcp_tool_attributes(
            "ahu_request_approval",
            &serde_json::json!({"operation":"private-operation","summary":"secret"}),
        );
        assert_eq!(string_attr(&invalid, "ahu.mcp.approval.operation"), None);
    }

    #[test]
    fn decision_identity_export_distinguishes_requested_from_reported_and_rejects_payloads() {
        for reported in [None, Some(false), Some(true)] {
            let mut service = serde_json::json!({"backend":"ollama","requested_model":"fixture","model":"actual"});
            if let Some(reported) = reported {
                service["model_reported"] = reported.into();
            }
            let response = serde_json::json!({"result":{"structuredContent":{"service":service}}});
            let attrs = mcp_result_attributes("tools/call", "ahu_typed_decide", &response);
            assert_eq!(
                string_attr(&attrs, "ahu.mcp.decision.model.requested"),
                Some("fixture".into())
            );
            assert_eq!(
                string_attr(&attrs, "ahu.mcp.decision.model.reported"),
                if reported == Some(true) {
                    Some("actual".into())
                } else {
                    None
                }
            );
            assert_eq!(
                attrs
                    .iter()
                    .filter(|a| a.key.as_str() == "ahu.mcp.decision.backend")
                    .count(),
                1
            );
        }
        let service = serde_json::json!({"backend":"https://private.invalid","requested_model":"/private/model","model":"sk-secret","model_reported":true});
        assert!(super::decision_identity_fields(service.as_object().unwrap()).is_empty());
    }

    #[test]
    fn auth_budget_telemetry_records_tool_use_without_profile_or_capacity_values() {
        let tool = mcp_tool_attributes("ahu_auth_budget", &serde_json::json!({}));
        assert_eq!(
            string_attr(&tool, "ahu.mcp.tool.name"),
            Some("ahu_auth_budget".into())
        );
        let result = mcp_result_attributes(
            "tools/call",
            "ahu_auth_budget",
            &serde_json::json!({"result":{"structuredContent":{
                "profile":"private-profile",
                "providers":[{"harness":"codex","status":"available","windows":[{"remaining_percent":88.0,"resets_at_unix":1800000000}]}]
            }}}),
        );
        assert_eq!(
            string_attr(&result, "ahu.mcp.outcome"),
            Some("success".into())
        );
        assert_eq!(result.len(), 1);
        assert!(!format!("{tool:?}{result:?}").contains("private-profile"));
        assert!(!format!("{tool:?}{result:?}").contains("remaining_percent"));
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
                "AHU_DECISION_MODEL must be valid: private-marker",
                "decision_configuration",
            ),
            (
                "invalid AHU_OLLAMA_URL: private-marker",
                "decision_configuration",
            ),
            (
                "AHU_OLLAMA_URL requires AHU_OLLAMA_MODEL",
                "decision_configuration",
            ),
            (
                "AHU_OLLAMA_URL path must be /v1/systemone",
                "decision_configuration",
            ),
            (
                "AHU_OLLAMA_MODEL cannot be a cloud model",
                "decision_configuration",
            ),
            (
                "AHU_OLLAMA_MODEL/AHU_OLLAMA_URL cannot be combined with AHU_DECISION_MODEL/AHU_DECISION_URL",
                "decision_configuration",
            ),
            (
                "cannot create Ollama decision client",
                "decision_configuration",
            ),
            (
                "Ollama decision request failed: private-marker",
                "decision_service_unavailable",
            ),
            (
                "Ollama decision API returned HTTP 500",
                "decision_service_http_error",
            ),
            (
                "Ollama decision response exceeds 1 MiB",
                "decision_service_invalid_response",
            ),
            (
                "Ollama decision response model does not match requested model",
                "decision_service_invalid_response",
            ),
            (
                "cannot read Ollama decision response",
                "decision_service_invalid_response",
            ),
            (
                "cannot read TypeSafe decision response",
                "decision_service_invalid_response",
            ),
            (
                "cannot read decision response",
                "decision_service_invalid_response",
            ),
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
            assert!(!format!("{attributes:?}").contains("private-marker"));
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
    fn launch_rejects_explicit_invalid_release_attribution_without_echoing_it() {
        assert_eq!(super::release_id_from_attributes("").unwrap(), None);
        assert_eq!(
            super::release_id_from_attributes("service.name=fixture,ahu.release.id=0.10.0")
                .unwrap()
                .as_deref(),
            Some("0.10.0")
        );
        for raw in [
            "ahu.release.id",
            "ahu.release.id=",
            "ahu.release.id=private-marker/invalid",
            "ahu.release.id=a, ahu.release.id=b",
            "ahu.release.id=a, ahu.release.id=a",
        ] {
            let error = super::release_id_from_attributes(raw).unwrap_err();
            assert!(!error.to_string().contains("private-marker"));
        }
        assert_eq!(
            super::release_id_from_attributes(r"unknown=escaped\,ahu.release.id=not-a-key")
                .unwrap(),
            None
        );
    }

    #[test]
    fn release_resource_values_are_bounded_and_ambiguous_labels_are_dropped() {
        assert_eq!(
            parse_ahu_resource_attributes("service.name=fixture, ahu.release.id = candidate-a "),
            vec![("ahu.release.id", "candidate-a".to_string())]
        );
        assert!(parse_ahu_resource_attributes("ahu.release.id=a, ahu.release.id = b ").is_empty());
        for value in ["release-01", "r_2.0", &"a".repeat(64)] {
            assert_eq!(
                parse_ahu_resource_attributes(&format!("ahu.release.id={value}")),
                vec![("ahu.release.id", value.to_string())]
            );
        }
        for value in [
            "",
            "two words",
            "synthetic\nprivate",
            "https://example.invalid/path",
            "id=private",
            "rélease",
            &"a".repeat(65),
        ] {
            assert!(parse_ahu_resource_attributes(&format!("ahu.release.id={value}")).is_empty());
        }
        for raw in [
            "ahu.release.id=r1,ahu.release.id=r2",
            "ahu.release.id=bad/value,ahu.release.id=r2",
        ] {
            assert!(parse_ahu_resource_attributes(raw).is_empty());
        }
        let attrs = parse_ahu_resource_attributes(
            "ahu.execution.mode=headless,ahu.task.root_id=root,ahu.task.parent_id=parent",
        );
        assert_eq!(attrs.len(), 3);
        assert!(parse_ahu_resource_attributes("ahu.execution.mode=private-payload").is_empty());
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
    fn skill_retrieval_telemetry_excludes_content_uris_metadata_and_digests() {
        let marker = "synthetic-sensitive-skill";
        let digest = crate::util::digest_bytes(marker.as_bytes());
        let skill = serde_json::json!({
            "uri": format!("ahu://skills/{marker}/SKILL.md"),
            "frontmatter": {"name": marker, "description": marker},
            "resources": [{"uri": marker, "digest": format!("sha256:{digest}"), "size": 42}],
            "_meta": {"origin": marker},
        });
        for method in [
            "skills/list",
            "skills/get",
            "resources/list",
            "resources/read",
        ] {
            for response in [
                serde_json::json!({"result": {
                    "skills": [skill.clone()], "skill": skill,
                    "resources": [{"uri": marker, "digest": digest}],
                    "contents": [{"uri": marker, "text": marker}],
                    "_meta": {"origin": marker},
                }}),
                serde_json::json!({"error": {"code": -32602, "message": marker, "data": skill}}),
            ] {
                let attributes = mcp_result_attributes(method, marker, &response);
                assert!(attributes.iter().all(|attribute| {
                    [
                        "ahu.mcp.outcome",
                        "ahu.mcp.error.code",
                        "ahu.mcp.error.category",
                    ]
                    .contains(&attribute.key.as_str())
                }));
                assert!(string_attr(&attributes, "ahu.mcp.outcome").is_some());
                let rendered = format!("{attributes:?}");
                assert!(!rendered.contains(marker));
                assert!(!rendered.contains(&digest));
                assert!(!rendered.contains("ahu://"));
                assert!(!rendered.contains("execution"));
            }
        }
    }

    #[test]
    fn skill_tool_telemetry_retains_only_names_and_outcomes() {
        let marker = "synthetic-private-skill";
        let payload = serde_json::json!({
            "uri": marker, "cursor": marker, "contents": [{"text":marker}],
            "skill": {"frontmatter": {"description":marker}, "_meta": {"origin":marker}},
        });
        for name in ["ahu_skills_list", "ahu_skills_get"] {
            let arguments = mcp_tool_attributes(name, &payload);
            assert_eq!(arguments.len(), 1);
            assert_eq!(
                string_attr(&arguments, "ahu.mcp.tool.name"),
                Some(name.into())
            );
            for response in [
                serde_json::json!({"result":{"structuredContent":payload, "content":[{"type":"text","text":marker}]}}),
                serde_json::json!({"result":{"isError":true,"content":[{"type":"text","text":marker}]}}),
                serde_json::json!({"error":{"code":-32602,"message":marker}}),
            ] {
                let attributes = mcp_result_attributes("tools/call", name, &response);
                assert!(attributes.iter().all(|attribute| {
                    [
                        "ahu.mcp.outcome",
                        "ahu.mcp.error.code",
                        "ahu.mcp.error.category",
                    ]
                    .contains(&attribute.key.as_str())
                }));
                assert!(!format!("{attributes:?}").contains(marker));
            }
        }
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

#[cfg(test)]
mod causal_context_tests {
    use super::{SpanGuard, causal_context, configure_child};
    use crate::config::TelemetryConfig;
    use opentelemetry::Context;
    use opentelemetry::trace::{Span, TraceContextExt, Tracer, TracerProvider};
    use opentelemetry_sdk::trace::{SdkTracerProvider, SpanData, SpanExporter};
    use std::process::Command;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    const PARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    fn carrier(parent: &str) -> String {
        format!("ahu.task.id=synthetic-task,ahu.task.attempt=1,ahu.internal.traceparent={parent}")
    }

    #[derive(Clone, Debug, Default)]
    struct Capture(Arc<Mutex<Vec<SpanData>>>);

    impl SpanExporter for Capture {
        async fn export(&self, batch: Vec<SpanData>) -> opentelemetry_sdk::error::OTelSdkResult {
            self.0.lock().unwrap().extend(batch);
            Ok(())
        }
    }

    pub(super) fn parent(provider: &SdkTracerProvider) -> SpanGuard {
        let tracer = opentelemetry::global::BoxedTracer::new(Box::new(provider.tracer("test")));
        SpanGuard {
            span: Some(tracer.start_with_context("synthetic.parent", &Context::new())),
            started: Instant::now(),
        }
    }

    #[test]
    fn explicit_parent_survives_child_carrier_and_thread_without_attachment() {
        let capture = Capture::default();
        let provider = SdkTracerProvider::builder()
            .with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOn)
            .with_simple_exporter(capture.clone())
            .build();
        let parent = parent(&provider);
        let expected = parent.span.as_ref().unwrap().span_context().clone();
        let ambient = Context::current().span().span_context().clone();
        let mut command = Command::new("synthetic-unused");
        let config = TelemetryConfig {
            enabled: true,
            ..TelemetryConfig::default()
        };
        configure_child(
            &mut command,
            &config,
            "agent",
            "codex",
            "model",
            None,
            None,
            Some("synthetic-task"),
            Some(1),
        );
        parent.configure_child_context(&mut command);
        // Idempotent reinjection must not introduce duplicate identity/context.
        parent.configure_child_context(&mut command);
        let raw = command
            .get_envs()
            .find(|(key, _)| *key == "AHU_MCP_RESOURCE_ATTRIBUTES")
            .unwrap()
            .1
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            !super::parse_ahu_resource_attributes(raw)
                .iter()
                .any(|(key, _)| key.contains("traceparent"))
        );
        let context = causal_context::extract(Some(raw), Some("synthetic-task"), Some(1));
        assert!(context.span().span_context().is_remote());
        assert_eq!(
            context.span().span_context().trace_id(),
            expected.trace_id()
        );
        let tracer = provider.tracer("child");
        std::thread::spawn(move || {
            assert!(!Context::current().span().span_context().is_valid());
            tracer.start_with_context("synthetic.child", &context).end();
            assert!(!Context::current().span().span_context().is_valid());
        })
        .join()
        .unwrap();
        drop(parent);
        assert_eq!(Context::current().span().span_context(), &ambient);
        provider.shutdown().unwrap();
        let spans = capture.0.lock().unwrap();
        let child = spans
            .iter()
            .find(|span| span.name == "synthetic.child")
            .unwrap();
        assert_eq!(child.span_context.trace_id(), expected.trace_id());
        assert_eq!(child.parent_span_id, expected.span_id());
        assert_ne!(child.span_context.span_id(), expected.span_id());
        assert!(child.parent_span_is_remote);
    }

    #[test]
    fn invalid_missing_or_mismatched_carriers_are_roots_even_with_ambient_context() {
        let valid = carrier(PARENT);
        let attached = causal_context::extract(Some(&valid), Some("synthetic-task"), Some(1));
        let _guard = attached.attach();
        let mut invalid = vec![
            String::new(),
            "x".repeat(4097),
            carrier(&PARENT.to_uppercase()),
            carrier(&format!("{PARENT}-extra")),
            carrier(&format!(" {PARENT}")),
            carrier(&format!("{PARENT}\n")),
            carrier(&PARENT.replacen("00-", "01-", 1)),
            carrier(&format!("{}03", &PARENT[..53])),
            carrier("00-00000000000000000000000000000000-00f067aa0ba902b7-01"),
            carrier("00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01"),
            carrier("秘密"),
            carrier(&"é".repeat(55)),
            valid.replace("attempt=1", "attempt=0"),
            valid.replace("attempt=1", "attempt=4294967296"),
            valid.replace("attempt=1", "attempt=-1"),
            valid.replace("ahu.task.attempt=1,", ""),
            valid.replace("ahu.task.id=synthetic-task,", ""),
            valid.replace("synthetic-task", "synthetic\\-task"),
            format!("{valid},ahu.task.id=synthetic-task"),
            format!("{valid},ahu.task.attempt=2"),
            format!("{valid},ahu.internal.traceparent={PARENT}"),
        ];
        invalid.push(valid.replace(&format!(",ahu.internal.traceparent={PARENT}"), ""));
        for raw in &invalid {
            assert!(
                !causal_context::extract(Some(raw), Some("synthetic-task"), Some(1))
                    .span()
                    .span_context()
                    .is_valid()
            );
        }
        for (raw, task) in [
            (None, Some("synthetic-task")),
            (Some(valid.as_str()), None),
            (Some(valid.as_str()), Some("other-task")),
        ] {
            assert!(
                !causal_context::extract(raw, task, Some(1))
                    .span()
                    .span_context()
                    .is_valid()
            );
        }
        for attempt in [None, Some(0), Some(2)] {
            assert!(
                !causal_context::extract(Some(&valid), Some("synthetic-task"), attempt)
                    .span()
                    .span_context()
                    .is_valid()
            );
        }
        let unsampled = carrier(&format!("{}00", &PARENT[..53]));
        let context = causal_context::extract(Some(&unsampled), Some("synthetic-task"), Some(1));
        assert!(context.span().span_context().is_valid());
        assert!(!context.span().span_context().is_sampled());
    }

    #[test]
    fn disabled_child_cannot_reuse_inherited_context() {
        if super::test_without_eval_endpoint(
            "telemetry::causal_context_tests::disabled_child_cannot_reuse_inherited_context",
        ) {
            return;
        }
        let provider = SdkTracerProvider::builder()
            .with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOn)
            .build();
        let parent = parent(&provider);
        let mut command = Command::new("synthetic-unused");
        command.env("AHU_MCP_RESOURCE_ATTRIBUTES", carrier(PARENT));
        configure_child(
            &mut command,
            &TelemetryConfig::default(),
            "agent",
            "codex",
            "model",
            None,
            None,
            Some("synthetic-task"),
            Some(1),
        );
        parent.configure_child_context(&mut command);
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == "AHU_MCP_RESOURCE_ATTRIBUTES" && value.is_none())
        );
    }
}

#[cfg(test)]
mod primary_export_tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    // A subprocess isolates both the SDK's global provider and inherited env.
    #[test]
    fn primary_export_environment_child() {
        let Ok(mode) = std::env::var("AHU_TEST_PRIMARY_MODE") else {
            return;
        };
        let mut config = crate::config::TelemetryConfig::default();
        if let Ok(endpoint) = std::env::var("AHU_TEST_CONFIG_ENDPOINT") {
            config.enabled = true;
            config.endpoint = endpoint;
        }
        if mode == "child-settings" {
            let mut command = Command::new("synthetic-unused");
            super::configure_child(
                &mut command,
                &config,
                "agent",
                &std::env::var("AHU_TEST_CHILD_HARNESS").unwrap_or_else(|_| "codex".into()),
                "model",
                None,
                None,
                Some("synthetic-task"),
                Some(1),
            );
            let env: std::collections::BTreeMap<_, _> = command
                .get_envs()
                .filter_map(|(key, value)| Some((key.to_str()?, value?.to_str()?)))
                .collect();
            let expected = std::env::var("AHU_TEST_EXPECT_SIGNALS").unwrap();
            if expected == "disabled" {
                assert!(!env.contains_key("OTEL_TRACES_EXPORTER"));
                assert!(!env.contains_key("AHU_MCP_RESOURCE_ATTRIBUTES"));
            } else if expected == "kiro-eval" {
                assert!(!env.contains_key("OTEL_EXPORTER_OTLP_ENDPOINT"));
                assert_eq!(env["OTEL_TRACES_EXPORTER"], "none");
                assert_eq!(env["OTEL_METRICS_EXPORTER"], "none");
                assert_eq!(env["OTEL_LOGS_EXPORTER"], "none");
                assert!(env["AHU_MCP_RESOURCE_ATTRIBUTES"].contains("ahu.task.id=synthetic-task"));
                assert!(super::eval_endpoint_override().is_some());
            } else {
                let expected_endpoint =
                    super::eval_endpoint_override().unwrap_or_else(|| config.endpoint.clone());
                assert_eq!(env["OTEL_EXPORTER_OTLP_ENDPOINT"], expected_endpoint);
                assert_eq!(env["OTEL_TRACES_EXPORTER"], "otlp");
                assert_eq!(env["OTEL_METRICS_EXPORTER"], expected);
                assert_eq!(env["OTEL_LOGS_EXPORTER"], expected);
                assert!(env["AHU_MCP_RESOURCE_ATTRIBUTES"].contains("ahu.task.id=synthetic-task"));
                assert!(env["AHU_MCP_RESOURCE_ATTRIBUTES"].contains("ahu.task.attempt=1"));
            }
            println!("synthetic task completed");
            return;
        }
        if mode == "ahu-mcp" {
            super::initialize_mcp(&config).unwrap();
        } else {
            super::initialize(&config).unwrap();
        }
        assert_eq!(super::exporter_ready(), mode != "rejected");
        let started = Instant::now();
        drop(super::span("ahu.synthetic.transport", []));
        assert!(started.elapsed() < Duration::from_millis(250));
        println!("synthetic task completed");
        super::shutdown();
    }

    fn listener() -> TcpListener {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        listener
    }

    fn endpoint(listener: &TcpListener) -> String {
        format!("http://{}", listener.local_addr().unwrap())
    }

    fn child_command(mode: &str, trap: &TcpListener) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .env_clear()
            .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|value| ("LLVM_PROFILE_FILE", value)))
            .env("AHU_TEST_PRIMARY_MODE", mode)
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint(trap))
            .env("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", endpoint(trap))
            .env("OTEL_EXPORTER_OTLP_TIMEOUT", "60000")
            .env("OTEL_EXPORTER_OTLP_TRACES_TIMEOUT", "60000")
            .env("OTEL_BSP_SCHEDULE_DELAY", "60000")
            .env("OTEL_BSP_MAX_QUEUE_SIZE", "65536")
            .env("OTEL_EXPORTER_OTLP_HEADERS", "x-synthetic=ambient-header")
            .env(
                "OTEL_RESOURCE_ATTRIBUTES",
                "synthetic.resource=ambient-resource",
            )
            .args([
                "--exact",
                "telemetry::primary_export_tests::primary_export_environment_child",
                "--nocapture",
            ]);
        command
    }

    fn run_child(mut command: Command) -> Duration {
        let scratch = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let mut child = command
            .current_dir(scratch.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if started.elapsed() > Duration::from_secs(4) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("telemetry blocked the synthetic task");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "child failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("synthetic task completed"));
        started.elapsed()
    }

    fn assert_unused(listener: &TcpListener) {
        assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
    }

    fn capture(
        collector: TcpListener,
        response: String,
        stall: bool,
    ) -> std::thread::JoinHandle<Option<(String, Vec<u8>)>> {
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            let (mut stream, _) = loop {
                match collector.accept() {
                    Ok(pair) => break pair,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            return None;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(e) => panic!("synthetic collector accept failed: {e}"),
                }
            };
            // Accepted sockets may inherit the listener's nonblocking mode.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                headers.push_str(&line);
                assert!(headers.len() < 16 * 1024);
            }
            let length = headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            assert!(length < 1024 * 1024);
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            if stall {
                // Longer than the exporter deadline; task shutdown must finish
                // while this otherwise healthy collector is still unresponsive.
                std::thread::sleep(Duration::from_secs(2));
            } else {
                stream.write_all(response.as_bytes()).unwrap();
            }
            Some((headers, body))
        })
    }

    fn assert_wire(wire: Option<(String, Vec<u8>)>, service: &str, header: &str) {
        use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
        use opentelemetry_proto::tonic::common::v1::any_value;
        use prost::Message;
        let (headers, body) = wire.expect("primary exporter reached its local collector");
        assert!(headers.starts_with("POST /v1/traces HTTP/1.1\r\n"));
        assert!(
            headers
                .to_ascii_lowercase()
                .contains("content-type: application/x-protobuf")
        );
        assert!(
            headers.contains(header),
            "documented ambient header was lost"
        );
        let message = ExportTraceServiceRequest::decode(body.as_slice()).unwrap();
        let resource = message.resource_spans[0].resource.as_ref().unwrap();
        for (key, value) in [
            ("service.name", service),
            ("synthetic.resource", "ambient-resource"),
        ] {
            assert!(resource.attributes.iter().any(|attr| {
                attr.key == key
                    && attr.value.as_ref().and_then(|v| v.value.as_ref())
                        == Some(&any_value::Value::StringValue(value.into()))
            }));
        }
        assert!(
            message
                .resource_spans
                .iter()
                .flat_map(|r| &r.scope_spans)
                .flat_map(|s| &s.spans)
                .any(|s| s.name == "ahu.synthetic.transport")
        );
    }

    #[test]
    fn primary_export_ignores_proxy_and_ambient_endpoints() {
        for service in ["ahu", "ahu-mcp"] {
            let collector = listener();
            let trap = listener();
            let mut command = child_command(service, &trap);
            command.env("AHU_EVAL_OTEL_ENDPOINT", endpoint(&collector));
            for key in [
                "HTTP_PROXY",
                "http_proxy",
                "HTTPS_PROXY",
                "https_proxy",
                "ALL_PROXY",
                "all_proxy",
            ] {
                command.env(key, endpoint(&trap));
            }
            command.env("NO_PROXY", "").env("no_proxy", "");
            let capture = capture(
                collector,
                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
                false,
            );
            run_child(command);
            assert_wire(
                capture.join().unwrap(),
                service,
                "x-synthetic: ambient-header",
            );
            assert_unused(&trap);
        }
    }

    #[test]
    fn mcp_otlp_resource_uses_carrier_identity_including_absent_release() {
        use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
        use opentelemetry_proto::tonic::common::v1::any_value;
        use prost::Message;
        for release in [None, Some("release-frozen")] {
            let collector = listener();
            let trap = listener();
            let mut command = child_command("ahu-mcp", &trap);
            let mut carrier = "ahu.task.id=frozen-task,ahu.task.root_id=frozen-root,ahu.task.attempt=2,ahu.execution.mode=headless".to_string();
            if let Some(release) = release {
                carrier.push_str(&format!(",ahu.release.id={release}"));
            }
            command.env("AHU_EVAL_OTEL_ENDPOINT", endpoint(&collector))
                .env("OTEL_RESOURCE_ATTRIBUTES", "ahu.release.id=ambient-release,ahu.task.id=ambient-task,ahu.task.parent_id=ambient-parent,ahu.private=private-value")
                .env("AHU_MCP_RESOURCE_ATTRIBUTES", carrier);
            let capture = capture(
                collector,
                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
                false,
            );
            run_child(command);
            let (_, bytes) = capture.join().unwrap().expect("MCP export");
            let decoded = ExportTraceServiceRequest::decode(bytes.as_slice()).unwrap();
            let attrs = &decoded.resource_spans[0]
                .resource
                .as_ref()
                .unwrap()
                .attributes;
            let get = |key: &str| {
                attrs.iter().find(|attr| attr.key == key).and_then(|attr| {
                    match attr.value.as_ref()?.value.as_ref()? {
                        any_value::Value::StringValue(value) => Some(value.as_str()),
                        _ => None,
                    }
                })
            };
            assert_eq!(get("ahu.release.id"), release);
            assert_eq!(get("ahu.task.id"), Some("frozen-task"));
            assert_eq!(get("ahu.task.root_id"), Some("frozen-root"));
            assert_eq!(get("ahu.task.attempt"), Some("2"));
            assert_eq!(get("ahu.execution.mode"), Some("headless"));
            assert_eq!(get("ahu.task.parent_id"), None);
            assert_eq!(get("ahu.private"), None);
            assert_unused(&trap);
        }
    }

    #[test]
    fn primary_export_does_not_follow_redirects() {
        for status in [307, 308] {
            let collector = listener();
            let trap = listener();
            let mut command = child_command("ahu", &trap);
            command
                .env("AHU_EVAL_OTEL_ENDPOINT", endpoint(&collector))
                .env(
                    "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
                    "x-synthetic-trace=ambient-trace-header",
                );
            let response = format!(
                "HTTP/1.1 {status} Redirect\r\nLocation: {}/escaped\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                endpoint(&trap)
            );
            let capture = capture(collector, response, false);
            run_child(command);
            assert_wire(
                capture.join().unwrap(),
                "ahu",
                "x-synthetic-trace: ambient-trace-header",
            );
            assert_unused(&trap);
        }
    }

    #[test]
    fn primary_export_failure_is_bounded_and_does_not_fail_task() {
        let collector = listener();
        let trap = listener();
        let mut command = child_command("ahu", &trap);
        command.env("AHU_EVAL_OTEL_ENDPOINT", endpoint(&collector));
        let capture = capture(collector, String::new(), true);
        let elapsed = run_child(command);
        assert_wire(
            capture.join().unwrap(),
            "ahu",
            "x-synthetic: ambient-header",
        );
        assert!(
            elapsed < Duration::from_millis(1500),
            "export timeout was not bounded: {elapsed:?}"
        );
        assert_unused(&trap);
    }

    #[test]
    fn primary_export_rejects_invalid_endpoints_before_transport() {
        let trap = listener();
        for endpoint in [
            "http://synthetic-user:synthetic-password@127.0.0.1:4318",
            "http://127.0.0.1:4318/?query",
            "http://127.0.0.1:4318/#fragment",
            "https://127.0.0.1:4318",
            "http://192.0.2.1:4318",
        ] {
            for key in ["AHU_TEST_CONFIG_ENDPOINT", "AHU_EVAL_OTEL_ENDPOINT"] {
                let mut command = child_command("rejected", &trap);
                command.env(key, endpoint);
                run_child(command);
                assert_unused(&trap);
            }
        }
    }

    #[test]
    fn eval_child_exports_only_traces_and_ordinary_collector_keeps_all_signals() {
        let trap = listener();
        for (eval, ordinary, expected) in [
            (Some("http://127.0.0.1:54321"), false, "none"),
            (Some("http://127.0.0.1:54321"), true, "none"),
            (None, true, "otlp"),
            (Some("https://example.invalid"), true, "otlp"),
            (Some("https://example.invalid"), false, "disabled"),
            (None, false, "disabled"),
        ] {
            let mut command = child_command("child-settings", &trap);
            command.env("AHU_TEST_EXPECT_SIGNALS", expected);
            if let Some(endpoint) = eval {
                command.env("AHU_EVAL_OTEL_ENDPOINT", endpoint);
            }
            if ordinary {
                command.env("AHU_TEST_CONFIG_ENDPOINT", "http://127.0.0.1:4318");
            }
            run_child(command);
        }
        for eval in [false, true] {
            let mut command = child_command("child-settings", &trap);
            command.env("AHU_TEST_CHILD_HARNESS", "kiro");
            command.env("AHU_TEST_CONFIG_ENDPOINT", "http://127.0.0.1:4318");
            command.env(
                "AHU_TEST_EXPECT_SIGNALS",
                if eval { "kiro-eval" } else { "otlp" },
            );
            if eval {
                command.env("AHU_EVAL_OTEL_ENDPOINT", "http://127.0.0.1:54321");
            }
            run_child(command);
        }
        assert_unused(&trap);
    }

    #[test]
    fn child_process_exports_real_parent_ids_and_rejects_stale_identity() {
        use opentelemetry::trace::Span;
        use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
        use prost::Message;

        for (service, matches_task) in [
            ("ahu", true),
            ("ahu-mcp", true),
            ("ahu", false),
            ("ahu-mcp", false),
        ] {
            let collector = listener();
            let trap = listener();
            let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
                .with_sampler(opentelemetry_sdk::trace::Sampler::AlwaysOn)
                .build();
            let parent = super::causal_context_tests::parent(&provider);
            let expected = parent.span.as_ref().unwrap().span_context().clone();
            let mut command = child_command(service, &trap);
            let config = crate::config::TelemetryConfig {
                enabled: true,
                endpoint: endpoint(&collector),
                ..Default::default()
            };
            super::configure_child(
                &mut command,
                &config,
                "agent",
                "codex",
                "model",
                None,
                None,
                Some("synthetic-task"),
                Some(1),
            );
            parent.configure_child_context(&mut command);
            command
                .env(
                    "AHU_TASK_ID",
                    if matches_task {
                        "synthetic-task"
                    } else {
                        "stale-task"
                    },
                )
                .env("AHU_EVAL_OTEL_ENDPOINT", endpoint(&collector))
                // Generic inherited context must not repair a rejected ahu carrier.
                .env(
                    "TRACEPARENT",
                    format!("00-{}-{}-01", expected.trace_id(), expected.span_id()),
                );
            let capture = capture(
                collector,
                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
                false,
            );
            run_child(command);
            let (_, body) = capture.join().unwrap().unwrap();
            let message = ExportTraceServiceRequest::decode(body.as_slice()).unwrap();
            let span = message
                .resource_spans
                .iter()
                .flat_map(|r| &r.scope_spans)
                .flat_map(|s| &s.spans)
                .find(|s| s.name == "ahu.synthetic.transport")
                .unwrap();
            if matches_task {
                assert_eq!(span.trace_id, expected.trace_id().to_bytes());
                assert_eq!(span.parent_span_id, expected.span_id().to_bytes());
            } else {
                assert_ne!(span.trace_id, expected.trace_id().to_bytes());
                assert!(span.parent_span_id.iter().all(|byte| *byte == 0));
            }
            assert!(
                message
                    .resource_spans
                    .iter()
                    .filter_map(|r| r.resource.as_ref())
                    .flat_map(|r| &r.attributes)
                    .all(|attr| attr.key != "ahu.internal.traceparent")
            );
            assert_unused(&trap);
        }
    }
}

/// Export one bounded prelaunch selection observation to this eval's collector.
/// Uses a private provider rather than changing global telemetry or process env.
pub(crate) fn export_eval_selection(
    endpoint: &str,
    run_id: &str,
    selection_id: &str,
    selection: &crate::skill_selection::Selection,
    release_id: Option<&str>,
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
    let mut resource_attributes = vec![
        string("service.name", "ahu-eval"),
        string("ahu.eval.run_id", run_id),
        string("ahu.eval.stage", "skill_selection"),
        string("ahu.task.id", selection_id),
        string("ahu.task.attempt", "1"),
    ];
    if let Some(release_id) = release_id {
        resource_attributes.push(string("ahu.release.id", release_id));
    }
    let message = ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(ProtoResource {
                attributes: resource_attributes,
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
            &selection,
            Some("release-fixture")
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
        let scratch = tempfile::tempdir().unwrap();
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
            .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|value| ("LLVM_PROFILE_FILE", value)))
            .current_dir(scratch.path())
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
        if !decision {
            assert!(
                body.windows(b"release-fixture".len())
                    .any(|w| w == b"release-fixture")
            );
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
            &selection,
            Some("release-fixture")
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
                &selection,
                None
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
        if let Some(service) = service.as_object() {
            for (suffix, value) in decision_identity_fields(service) {
                attributes.push(string(&format!("ahu.eval.decision.{suffix}"), value));
            }
        }
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

//! Small, loopback-only OTLP/HTTP receiver for `ahu eval run`.
//!
//! Evals need telemetry that can be joined to their runs without depending on
//! a vendor-specific collector query API. This receiver accepts OTLP protobuf
//! exports in memory, immediately projects them onto a fixed allowlist of
//! numeric counters and bounded identifiers, and retains no raw spans.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value as AnyValue;
use prost::Message;

const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_TASKS: usize = 1024;
const MAX_CONCURRENT_CONNECTIONS: usize = 16;
/// Span identities retained per task for duplicate-export detection.
///
/// An OTLP exporter may resend a batch it is unsure reached the collector, so a
/// counter that simply adds every export would report work that happened once
/// as having happened twice. Identities are bounded: beyond this, a span is
/// still projected, it just is not deduplicated.
const MAX_SPAN_IDS_PER_TASK: usize = 4096;

/// Receiver-side telemetry losses/errors for one eval interval.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReceiverStats {
    pub rejected_spans: u64,
    pub rejected_requests: u64,
    pub rejected_connections: u64,
    pub accept_errors: u64,
}

impl ReceiverStats {
    pub fn add(&mut self, other: Self) {
        self.rejected_spans = self.rejected_spans.saturating_add(other.rejected_spans);
        self.rejected_requests = self
            .rejected_requests
            .saturating_add(other.rejected_requests);
        self.rejected_connections = self
            .rejected_connections
            .saturating_add(other.rejected_connections);
        self.accept_errors = self.accept_errors.saturating_add(other.accept_errors);
    }

    pub fn since(self, earlier: Self) -> Self {
        Self {
            rejected_spans: self.rejected_spans.saturating_sub(earlier.rejected_spans),
            rejected_requests: self
                .rejected_requests
                .saturating_sub(earlier.rejected_requests),
            rejected_connections: self
                .rejected_connections
                .saturating_sub(earlier.rejected_connections),
            accept_errors: self.accept_errors.saturating_sub(earlier.accept_errors),
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TaskTelemetry {
    pub task_id: String,
    pub attempt: u32,
    pub trace_id: Option<String>,
    pub elapsed_ms: Option<u64>,
    pub mcp_requests: u64,
    pub tool_list_calls: u64,
    pub tool_calls: u64,
    pub tool_errors: u64,
    pub typed_decision_calls: u64,
    pub typed_decision_errors: u64,
    pub decision_input_tokens: Option<u64>,
    pub decision_output_tokens: Option<u64>,
    pub decision_duration_ms: Option<f64>,
    /// True once an `ahu.mcp.session` summary span arrived for this task.
    ///
    /// The summary is what carries the session's own totals, so without it the
    /// per-call spans below are a floor rather than a count.
    pub mcp_observed: bool,
    pub tool_calls_by_name: BTreeMap<String, u64>,
    pub tool_errors_by_name: BTreeMap<String, u64>,
    /// Every span accepted for this task, whatever its name.
    ///
    /// Distinguishes "nothing was observed" from "spans arrived but no session
    /// summary did", which are different claims about a run.
    pub spans_recorded: u64,
    /// `ahu.mcp.session` summary spans seen. More than one is an anomaly worth
    /// seeing rather than hiding behind the boolean above.
    pub session_summaries: u64,
    /// `ahu.mcp.tool.call` spans seen, before name filtering.
    pub tool_call_spans: u64,
    /// Spans dropped because their identity had already been projected.
    pub duplicate_spans: u64,
    /// Span identities already projected, so a resent export is not counted
    /// twice. Never serialized: it is bookkeeping, not an observation.
    #[serde(skip)]
    pub(crate) seen_span_ids: BTreeSet<Vec<u8>>,
}

impl TaskTelemetry {
    /// How much of the session this telemetry actually describes.
    pub fn coverage(&self) -> Coverage {
        if self.session_summaries > 0 {
            Coverage::CompleteSession
        } else if self.spans_recorded > 0 {
            Coverage::PartialSpans
        } else {
            Coverage::None
        }
    }

    /// Whether every tool call the session summary counted is attributable to a
    /// named tool.
    ///
    /// Only then can an absence be read as proof that a tool was not called:
    /// a call the projection could not name leaves the question open.
    pub fn tool_calls_fully_named(&self) -> bool {
        self.session_summaries > 0
            && self.tool_calls_by_name.values().sum::<u64>() == self.tool_calls
    }

    /// Whether the session summary attributed every tool error to a named tool.
    pub fn tool_errors_fully_named(&self) -> bool {
        self.session_summaries > 0
            && self.tool_errors_by_name.values().sum::<u64>() == self.tool_errors
    }
}

/// What a run's telemetry supports being read as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// No span reached the receiver for this task at all.
    None,
    /// Spans arrived, but no session summary: counts are a floor, not a total.
    PartialSpans,
    /// The session summary arrived, so its totals are the session's totals.
    CompleteSession,
}

impl Coverage {
    pub fn as_str(self) -> &'static str {
        match self {
            Coverage::None => "none",
            Coverage::PartialSpans => "partial_spans",
            Coverage::CompleteSession => "complete_session",
        }
    }
}

#[derive(Default)]
struct CaptureState {
    tasks: BTreeMap<(String, u32), TaskTelemetry>,
    receiver_stats: ReceiverStats,
    expected_run_id: Option<String>,
    expected_case_id: Option<String>,
}

pub struct Receiver {
    endpoint: String,
    state: Arc<Mutex<CaptureState>>,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Receiver {
    pub fn start() -> std::io::Result<Self> {
        Self::start_for(None, None)
    }

    pub fn start_for(run_id: Option<&str>, case_id: Option<&str>) -> std::io::Result<Self> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let endpoint = format!("http://{address}");
        let state = Arc::new(Mutex::new(CaptureState {
            expected_run_id: run_id.map(str::to_owned),
            expected_case_id: case_id.map(str::to_owned),
            ..CaptureState::default()
        }));
        let stopping = Arc::new(AtomicBool::new(false));
        let thread_state = Arc::clone(&state);
        let thread_stopping = Arc::clone(&stopping);
        let worker = thread::Builder::new()
            .name("ahu-eval-otlp".into())
            .spawn(move || {
                let mut connections: Vec<JoinHandle<()>> = Vec::new();
                while !thread_stopping.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            let mut pending = Vec::with_capacity(connections.len());
                            for connection in connections.drain(..) {
                                if connection.is_finished() {
                                    let _ = connection.join();
                                } else {
                                    pending.push(connection);
                                }
                            }
                            connections = pending;
                            if connections.len() >= MAX_CONCURRENT_CONNECTIONS {
                                increment_stats(&thread_state, |stats| {
                                    stats.rejected_connections =
                                        stats.rejected_connections.saturating_add(1);
                                });
                                respond(&mut stream, 503, "receiver busy");
                                continue;
                            }
                            let connection_state = Arc::clone(&thread_state);
                            if let Ok(connection) = thread::Builder::new()
                                .name("ahu-eval-otlp-client".into())
                                .spawn(move || handle_connection(stream, &connection_state))
                            {
                                connections.push(connection);
                            } else {
                                increment_stats(&thread_state, |stats| {
                                    stats.rejected_connections =
                                        stats.rejected_connections.saturating_add(1);
                                });
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => {
                            increment_stats(&thread_state, |stats| {
                                stats.accept_errors = stats.accept_errors.saturating_add(1);
                            });
                            break;
                        }
                    }
                }
                for connection in connections {
                    let _ = connection.join();
                }
            })?;
        Ok(Self {
            endpoint,
            state,
            stopping,
            worker: Some(worker),
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn task(&self, task_id: &str, attempt: u32) -> Option<TaskTelemetry> {
        self.state
            .lock()
            .ok()?
            .tasks
            .get(&(task_id.to_string(), attempt))
            .cloned()
    }

    pub fn receiver_stats(&self) -> ReceiverStats {
        self.state
            .lock()
            .map(|state| state.receiver_stats)
            .unwrap_or_default()
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn handle_connection(mut stream: TcpStream, state: &Arc<Mutex<CaptureState>>) {
    // Some platforms pass the listener's nonblocking mode to accepted sockets.
    // The parser relies on read/write timeouts, so restore blocking mode here.
    if stream.set_nonblocking(false).is_err() {
        increment_stats(state, |stats| {
            stats.rejected_requests = stats.rejected_requests.saturating_add(1);
        });
        return;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let Ok((headers, mut body)) = read_request(&mut stream) else {
        increment_stats(state, |stats| {
            stats.rejected_requests = stats.rejected_requests.saturating_add(1);
        });
        respond(&mut stream, 400, "bad request");
        return;
    };
    if !headers.starts_with("POST /v1/traces HTTP/1.")
        || !headers
            .to_ascii_lowercase()
            .contains("content-type: application/x-protobuf")
    {
        increment_stats(state, |stats| {
            stats.rejected_requests = stats.rejected_requests.saturating_add(1);
        });
        respond(&mut stream, 404, "unsupported OTLP request");
        return;
    }
    let Some(length) =
        header_value(&headers, "content-length").and_then(|value| value.parse::<usize>().ok())
    else {
        increment_stats(state, |stats| {
            stats.rejected_requests = stats.rejected_requests.saturating_add(1);
        });
        respond(&mut stream, 411, "content-length required");
        return;
    };
    if length > MAX_BODY_BYTES || body.len() > length {
        increment_stats(state, |stats| {
            stats.rejected_requests = stats.rejected_requests.saturating_add(1);
        });
        respond(&mut stream, 413, "OTLP request too large");
        return;
    }
    if body.len() < length {
        let remaining = length - body.len();
        let mut rest = vec![0; remaining];
        if stream.read_exact(&mut rest).is_err() {
            increment_stats(state, |stats| {
                stats.rejected_requests = stats.rejected_requests.saturating_add(1);
            });
            respond(&mut stream, 400, "truncated OTLP request");
            return;
        }
        body.extend_from_slice(&rest);
    }
    body.truncate(length);
    match ExportTraceServiceRequest::decode(body.as_slice()) {
        Ok(request) => {
            consume(request, state);
            respond(&mut stream, 200, "");
        }
        Err(_) => {
            increment_stats(state, |stats| {
                stats.rejected_requests = stats.rejected_requests.saturating_add(1);
            });
            respond(&mut stream, 400, "invalid OTLP protobuf");
        }
    }
}

fn increment_stats(state: &Arc<Mutex<CaptureState>>, update: impl FnOnce(&mut ReceiverStats)) {
    if let Ok(mut state) = state.lock() {
        update(&mut state.receiver_stats);
    }
}

/// Read only bounded HTTP/1.1 requests, preserving any body bytes read with
/// the header. The exporter uses `Content-Length`; chunked and compressed
/// requests are intentionally unsupported.
fn read_request(stream: &mut TcpStream) -> std::io::Result<(String, Vec<u8>)> {
    let mut bytes = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    loop {
        if bytes.len() >= MAX_HEADER_BYTES + MAX_BODY_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request too large",
            ));
        }
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let header_end = index + 4;
            if header_end > MAX_HEADER_BYTES {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "headers too large",
                ));
            }
            let headers = std::str::from_utf8(&bytes[..index])
                .map_err(|_| std::io::ErrorKind::InvalidData)?
                .to_string();
            let body = bytes[header_end..].to_vec();
            return Ok((headers, body));
        }
        if bytes.len() > MAX_HEADER_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "headers too large",
            ));
        }
    }
}

fn header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then_some(value.trim())
    })
}

fn respond(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        411 => "Length Required",
        413 => "Payload Too Large",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn consume(request: ExportTraceServiceRequest, state: &Arc<Mutex<CaptureState>>) {
    let mut state = match state.lock() {
        Ok(state) => state,
        Err(_) => return,
    };
    for resource_spans in request.resource_spans {
        let resource = resource_spans.resource.as_ref();
        let resource = resource
            .map(|resource| string_attributes(&resource.attributes))
            .unwrap_or_default();
        if state.expected_run_id.as_ref().is_some_and(|expected| {
            resource_string_attribute(resource_spans.resource.as_ref(), "ahu.eval.run_id")
                != Some(expected.as_str())
        }) || state.expected_case_id.as_ref().is_some_and(|expected| {
            resource_string_attribute(resource_spans.resource.as_ref(), "ahu.eval.case_id")
                != Some(expected.as_str())
        }) {
            continue;
        }
        let resource_task_id = resource.get("ahu.task.id").cloned();
        let resource_attempt = resource
            .get("ahu.task.attempt")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(1);
        for scope in resource_spans.scope_spans {
            for span in scope.spans {
                let attrs = string_attributes(&span.attributes);
                let integers = integer_attributes(&span.attributes);
                let numbers = number_attributes(&span.attributes);
                let Some(task_id) = resource_task_id
                    .clone()
                    .or_else(|| attrs.get("ahu.task.id").cloned())
                    .filter(|id| safe_identifier(id))
                else {
                    continue;
                };
                let attempt = resource
                    .get("ahu.task.attempt")
                    .and_then(|value| value.parse::<u32>().ok())
                    .or_else(|| integers.get("ahu.task.attempt").map(|value| *value as u32))
                    .unwrap_or(resource_attempt);
                let key = (task_id.clone(), attempt);
                if !state.tasks.contains_key(&key) && state.tasks.len() >= MAX_TASKS {
                    state.receiver_stats.rejected_spans =
                        state.receiver_stats.rejected_spans.saturating_add(1);
                    continue;
                }
                let task = state.tasks.entry(key).or_insert_with(|| TaskTelemetry {
                    task_id,
                    attempt,
                    ..TaskTelemetry::default()
                });
                // A resent export repeats span identities. Projecting one twice
                // would inflate every counter below, so it is counted as a
                // duplicate instead. A span with no identity cannot be
                // recognised, so it is projected as it arrives.
                if !span.span_id.is_empty() {
                    if task.seen_span_ids.contains(&span.span_id) {
                        task.duplicate_spans = task.duplicate_spans.saturating_add(1);
                        continue;
                    }
                    if task.seen_span_ids.len() < MAX_SPAN_IDS_PER_TASK {
                        task.seen_span_ids.insert(span.span_id.clone());
                    }
                }
                task.spans_recorded = task.spans_recorded.saturating_add(1);
                match span.name.as_str() {
                    "ahu.harness.run" => {
                        if span.end_time_unix_nano >= span.start_time_unix_nano {
                            task.elapsed_ms = Some(
                                (span.end_time_unix_nano - span.start_time_unix_nano) / 1_000_000,
                            );
                        }
                        let trace_id = span
                            .trace_id
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<String>();
                        if trace_id.len() == 32 && trace_id.bytes().any(|byte| byte != b'0') {
                            task.trace_id = Some(trace_id);
                        }
                    }
                    "ahu.mcp.tools.list" => (),
                    "ahu.mcp.tool.call" => {
                        task.tool_call_spans = task.tool_call_spans.saturating_add(1);
                        if let Some(name) = attrs.get("ahu.mcp.tool.name") {
                            let count = task.tool_calls_by_name.entry(name.clone()).or_default();
                            *count = count.saturating_add(1);
                            if attrs
                                .get("ahu.mcp.outcome")
                                .is_some_and(|value| value == "error")
                            {
                                let errors =
                                    task.tool_errors_by_name.entry(name.clone()).or_default();
                                *errors = errors.saturating_add(1);
                                if name == "ahu_typed_decide" {
                                    task.typed_decision_errors =
                                        task.typed_decision_errors.saturating_add(1);
                                }
                            }
                            if name == "ahu_typed_decide"
                                && attrs.get("ahu.mcp.outcome").is_some_and(|v| v == "success")
                            {
                                add_counter(
                                    &mut task.decision_input_tokens,
                                    integers.get("ahu.mcp.decision.tokens.input").copied(),
                                );
                                add_counter(
                                    &mut task.decision_output_tokens,
                                    integers.get("ahu.mcp.decision.tokens.output").copied(),
                                );
                                add_duration(
                                    &mut task.decision_duration_ms,
                                    numbers.get("ahu.mcp.decision.duration_ms").copied(),
                                );
                            }
                        }
                    }
                    "ahu.mcp.session" => {
                        task.mcp_observed = true;
                        task.session_summaries = task.session_summaries.saturating_add(1);
                        for (attribute, target) in [
                            ("ahu.mcp.requests.count", &mut task.mcp_requests),
                            ("ahu.mcp.tools.list.count", &mut task.tool_list_calls),
                            ("ahu.mcp.tool.calls.count", &mut task.tool_calls),
                            ("ahu.mcp.tool.errors.count", &mut task.tool_errors),
                            (
                                "ahu.mcp.decision.calls.count",
                                &mut task.typed_decision_calls,
                            ),
                        ] {
                            if let Some(count) = integers.get(attribute) {
                                *target = target.saturating_add(*count);
                            }
                        }
                    }
                    _ => (),
                }
            }
        }
    }
}

fn safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn string_attributes(
    attributes: &[opentelemetry_proto::tonic::common::v1::KeyValue],
) -> BTreeMap<String, String> {
    attributes
        .iter()
        .filter_map(|attribute| {
            let Some(AnyValue::StringValue(value)) = attribute.value.as_ref()?.value.as_ref()
            else {
                return None;
            };
            matches!(
                attribute.key.as_str(),
                "ahu.task.id"
                    | "ahu.task.attempt"
                    | "ahu.mcp.method"
                    | "ahu.mcp.tool.name"
                    | "ahu.mcp.outcome"
            )
            .then(|| (attribute.key.clone(), value.clone()))
        })
        .filter(|(key, value)| {
            key.as_str() != "ahu.mcp.tool.name"
                || matches!(
                    value.as_str(),
                    "ahu_agents_list" | "ahu_tasks_list" | "ahu_task_get" | "ahu_typed_decide"
                )
        })
        .collect()
}

fn resource_string_attribute<'a>(
    resource: Option<&'a opentelemetry_proto::tonic::resource::v1::Resource>,
    key: &str,
) -> Option<&'a str> {
    resource?.attributes.iter().find_map(|attribute| {
        if attribute.key != key {
            return None;
        }
        let Some(AnyValue::StringValue(value)) = attribute.value.as_ref()?.value.as_ref() else {
            return None;
        };
        Some(value.as_str())
    })
}

fn integer_attributes(
    attributes: &[opentelemetry_proto::tonic::common::v1::KeyValue],
) -> BTreeMap<String, u64> {
    attributes
        .iter()
        .filter_map(|attribute| {
            let Some(AnyValue::IntValue(value)) = attribute.value.as_ref()?.value.as_ref() else {
                return None;
            };
            (*value >= 0).then(|| (attribute.key.clone(), *value as u64))
        })
        .collect()
}

fn number_attributes(
    attributes: &[opentelemetry_proto::tonic::common::v1::KeyValue],
) -> BTreeMap<String, f64> {
    attributes
        .iter()
        .filter_map(|attribute| {
            let value = match attribute.value.as_ref()?.value.as_ref()? {
                AnyValue::DoubleValue(value) => *value,
                AnyValue::IntValue(value) if *value >= 0 => *value as f64,
                _ => return None,
            };
            (value.is_finite() && value >= 0.0).then(|| (attribute.key.clone(), value))
        })
        .collect()
}

fn add_counter(total: &mut Option<u64>, value: Option<u64>) {
    if let Some(value) = value {
        *total = Some(total.unwrap_or_default().saturating_add(value));
    }
}

fn add_duration(total: &mut Option<f64>, value: Option<f64>) {
    if let Some(value) = value.filter(|value| value.is_finite() && *value >= 0.0) {
        *total = Some(total.unwrap_or_default() + value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::common::v1::{
        AnyValue, KeyValue, any_value::Value as AnyValueValue,
    };
    use opentelemetry_proto::tonic::resource::v1::Resource;
    use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};

    fn string_attribute(key: &str, value: &str) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: Some(AnyValue {
                value: Some(AnyValueValue::StringValue(value.into())),
            }),
            key_strindex: 0,
        }
    }

    fn count_attribute(key: &str, value: i64) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: Some(AnyValue {
                value: Some(AnyValueValue::IntValue(value)),
            }),
            key_strindex: 0,
        }
    }

    fn double_attribute(key: &str, value: f64) -> KeyValue {
        KeyValue {
            key: key.into(),
            value: Some(AnyValue {
                value: Some(AnyValueValue::DoubleValue(value)),
            }),
            key_strindex: 0,
        }
    }

    #[test]
    fn captures_only_bounded_ahu_mcp_metrics_for_the_joined_task() {
        let state = Arc::new(Mutex::new(CaptureState::default()));
        let request = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource {
                    attributes: vec![
                        string_attribute("ahu.task.id", "task-123"),
                        string_attribute("private.prompt", "must not be retained"),
                        string_attribute("ahu.task.attempt", "2"),
                    ],
                    ..Resource::default()
                }),
                scope_spans: vec![ScopeSpans {
                    scope: None,
                    schema_url: String::new(),
                    spans: vec![
                        Span {
                            name: "ahu.harness.run".into(),
                            trace_id: vec![1; 16],
                            start_time_unix_nano: 1_000_000,
                            end_time_unix_nano: 6_000_000,
                            ..Span::default()
                        },
                        Span {
                            name: "ahu.mcp.session".into(),
                            attributes: vec![
                                count_attribute("ahu.mcp.requests.count", 8),
                                count_attribute("ahu.mcp.tools.list.count", 1),
                                count_attribute("ahu.mcp.tool.calls.count", 3),
                                count_attribute("ahu.mcp.tool.errors.count", 1),
                                count_attribute("ahu.mcp.decision.calls.count", 2),
                            ],
                            ..Span::default()
                        },
                        Span {
                            name: "ahu.mcp.tool.call".into(),
                            attributes: vec![
                                string_attribute("ahu.mcp.tool.name", "ahu_typed_decide"),
                                string_attribute("ahu.mcp.outcome", "error"),
                            ],
                            ..Span::default()
                        },
                        Span {
                            name: "ahu.mcp.tool.call".into(),
                            attributes: vec![
                                string_attribute("ahu.mcp.tool.name", "ahu_typed_decide"),
                                string_attribute("ahu.mcp.outcome", "success"),
                                count_attribute("ahu.mcp.decision.tokens.input", 10),
                                count_attribute("ahu.mcp.decision.tokens.output", 4),
                                double_attribute("ahu.mcp.decision.duration_ms", 12.5),
                            ],
                            ..Span::default()
                        },
                    ],
                }],
                schema_url: String::new(),
            }],
        };
        consume(request, &state);
        let state = state.lock().unwrap();
        let task = state.tasks.get(&("task-123".into(), 2)).unwrap();
        assert_eq!(task.elapsed_ms, Some(5));
        let expected_trace_id = "01".repeat(16);
        assert_eq!(task.trace_id.as_deref(), Some(expected_trace_id.as_str()));
        assert!(task.mcp_observed);
        assert_eq!(task.mcp_requests, 8);
        assert_eq!(task.tool_calls_by_name.get("ahu_typed_decide"), Some(&2));
        assert_eq!(task.typed_decision_errors, 1);
        assert_eq!(task.decision_input_tokens, Some(10));
        assert_eq!(task.decision_output_tokens, Some(4));
        assert_eq!(task.decision_duration_ms, Some(12.5));
        assert!(!format!("{task:?}").contains("must not be retained"));
    }

    /// One request carrying `spans` under a task resource, for coverage tests.
    fn export(task: &str, spans: Vec<Span>) -> ExportTraceServiceRequest {
        ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource {
                    attributes: vec![string_attribute("ahu.task.id", task)],
                    ..Resource::default()
                }),
                scope_spans: vec![ScopeSpans {
                    spans,
                    ..ScopeSpans::default()
                }],
                ..ResourceSpans::default()
            }],
        }
    }

    fn session_span(span_id: u8, tool_calls: i64) -> Span {
        Span {
            name: "ahu.mcp.session".into(),
            span_id: vec![span_id; 8],
            attributes: vec![count_attribute("ahu.mcp.tool.calls.count", tool_calls)],
            ..Span::default()
        }
    }

    fn tool_call_span(span_id: u8, tool: &str) -> Span {
        Span {
            name: "ahu.mcp.tool.call".into(),
            span_id: vec![span_id; 8],
            attributes: vec![string_attribute("ahu.mcp.tool.name", tool)],
            ..Span::default()
        }
    }

    #[test]
    fn coverage_separates_no_observation_from_partial_spans_and_a_session_summary() {
        let state = Arc::new(Mutex::new(CaptureState::default()));
        // No span at all for a task the caller asks about.
        assert!(
            !state
                .lock()
                .unwrap()
                .tasks
                .contains_key(&("absent".into(), 1))
        );

        // Tool spans without the session summary: a floor, not a total.
        consume(
            export("partial", vec![tool_call_span(1, "ahu_agents_list")]),
            &state,
        );
        let partial = state.lock().unwrap().tasks[&("partial".into(), 1)].clone();
        assert_eq!(partial.coverage(), Coverage::PartialSpans);
        assert_eq!(partial.coverage().as_str(), "partial_spans");
        assert!(!partial.mcp_observed);
        assert!(!partial.tool_calls_fully_named());
        assert_eq!(partial.tool_call_spans, 1);

        // With the summary, the session's own totals are available.
        consume(
            export(
                "complete",
                vec![session_span(2, 1), tool_call_span(3, "ahu_agents_list")],
            ),
            &state,
        );
        let complete = state.lock().unwrap().tasks[&("complete".into(), 1)].clone();
        assert_eq!(complete.coverage(), Coverage::CompleteSession);
        assert_eq!(complete.session_summaries, 1);
        assert!(complete.tool_calls_fully_named());

        // A summary counting more calls than the projection can name leaves the
        // question of which tools ran open.
        consume(export("unnamed", vec![session_span(4, 3)]), &state);
        let unnamed = state.lock().unwrap().tasks[&("unnamed".into(), 1)].clone();
        assert_eq!(unnamed.coverage(), Coverage::CompleteSession);
        assert!(!unnamed.tool_calls_fully_named());
    }

    #[test]
    fn a_span_attempt_attribute_overrides_the_default_attempt() {
        let state = Arc::new(Mutex::new(CaptureState::default()));
        let mut harness = Span {
            name: "ahu.harness.run".into(),
            ..Span::default()
        };
        harness.attributes = vec![
            string_attribute("ahu.task.id", "retry-task"),
            count_attribute("ahu.task.attempt", 2),
        ];
        consume(
            ExportTraceServiceRequest {
                resource_spans: vec![ResourceSpans {
                    scope_spans: vec![ScopeSpans {
                        spans: vec![harness],
                        ..ScopeSpans::default()
                    }],
                    ..ResourceSpans::default()
                }],
            },
            &state,
        );
        let state = state.lock().unwrap();
        assert!(!state.tasks.contains_key(&("retry-task".into(), 1)));
        assert_eq!(state.tasks[&("retry-task".into(), 2)].attempt, 2);
    }

    #[test]
    fn a_resent_export_is_counted_as_a_duplicate_rather_than_as_more_work() {
        let state = Arc::new(Mutex::new(CaptureState::default()));
        let spans = vec![session_span(9, 1), tool_call_span(10, "ahu_typed_decide")];
        consume(export("retried", spans.clone()), &state);
        consume(export("retried", spans), &state);
        let task = state.lock().unwrap().tasks[&("retried".into(), 1)].clone();
        assert_eq!(
            task.session_summaries, 1,
            "the session summary arrived once"
        );
        assert_eq!(task.tool_calls, 1);
        assert_eq!(task.tool_calls_by_name.get("ahu_typed_decide"), Some(&1));
        assert_eq!(task.duplicate_spans, 2);
        assert_eq!(task.spans_recorded, 2);
        assert!(task.tool_calls_fully_named());
    }

    #[test]
    fn local_receiver_binds_loopback_and_accepts_otlp_http_protobuf() {
        let receiver = Receiver::start().unwrap();
        let endpoint = receiver.endpoint().parse::<url::Url>().unwrap();
        assert_eq!(endpoint.host_str(), Some("127.0.0.1"));
        assert!(endpoint.port().is_some());
        let request = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource {
                    attributes: vec![string_attribute("ahu.task.id", "task-http")],
                    ..Resource::default()
                }),
                scope_spans: vec![ScopeSpans {
                    spans: vec![Span {
                        name: "ahu.mcp.session".into(),
                        ..Span::default()
                    }],
                    ..ScopeSpans::default()
                }],
                ..ResourceSpans::default()
            }],
        };
        let body = request.encode_to_vec();
        let mut stream = TcpStream::connect(("127.0.0.1", endpoint.port().unwrap())).unwrap();
        let mut request_bytes = format!(
            "POST /v1/traces HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        request_bytes.extend_from_slice(&body);
        stream.write_all(&request_bytes).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        for _ in 0..100 {
            if receiver
                .state
                .lock()
                .unwrap()
                .tasks
                .contains_key(&("task-http".into(), 1))
            {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            receiver
                .state
                .lock()
                .unwrap()
                .tasks
                .contains_key(&("task-http".into(), 1))
        );
    }

    #[test]
    fn local_receiver_accepts_a_second_export_while_one_client_is_stalled() {
        fn http_request(task_id: &str, span_id: u8) -> Vec<u8> {
            let body = export(task_id, vec![session_span(span_id, 0)]).encode_to_vec();
            let header = format!(
                "POST /v1/traces HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let mut request = header.into_bytes();
            request.extend_from_slice(&body);
            request
        }

        let receiver = Receiver::start().unwrap();
        let endpoint = receiver.endpoint().parse::<url::Url>().unwrap();
        let address = ("127.0.0.1", endpoint.port().unwrap());

        let stalled_stream = TcpStream::connect(address).unwrap();
        stalled_stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        thread::sleep(Duration::from_millis(30));

        let fast_request = http_request("task-fast", 22);
        let mut fast_stream = TcpStream::connect(address).unwrap();
        fast_stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        fast_stream.write_all(&fast_request).unwrap();
        let mut fast_response = String::new();
        fast_stream.read_to_string(&mut fast_response).unwrap();
        assert!(
            fast_response.starts_with("HTTP/1.1 200 OK"),
            "{fast_response}"
        );
        drop(stalled_stream);
        assert_eq!(
            receiver.task("task-fast", 1).unwrap().coverage(),
            Coverage::CompleteSession
        );
    }

    #[test]
    fn local_receiver_counts_connection_rejections_at_capacity() {
        let receiver = Receiver::start().unwrap();
        let endpoint = receiver.endpoint().parse::<url::Url>().unwrap();
        let address = ("127.0.0.1", endpoint.port().unwrap());
        let stalled: Vec<_> = (0..MAX_CONCURRENT_CONNECTIONS)
            .map(|_| {
                let mut stream = TcpStream::connect(address).unwrap();
                stream.write_all(b"POST /v1/traces HTTP/1.1\r\n").unwrap();
                stream
            })
            .collect();
        thread::sleep(Duration::from_millis(50));

        let mut rejected = TcpStream::connect(address).unwrap();
        rejected
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let mut response = String::new();
        rejected.read_to_string(&mut response).unwrap();
        assert!(
            response.starts_with("HTTP/1.1 503 Service Unavailable"),
            "response: {response:?}"
        );
        assert_eq!(receiver.receiver_stats().rejected_connections, 1);
        drop(stalled);
    }

    #[test]
    fn local_receiver_counts_rejected_requests() {
        let receiver = Receiver::start().unwrap();
        let endpoint = receiver.endpoint().parse::<url::Url>().unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", endpoint.port().unwrap())).unwrap();
        stream
            .write_all(b"GET /not-traces HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 404 Not Found"));
        assert_eq!(receiver.receiver_stats().rejected_requests, 1);
    }

    #[test]
    fn local_receiver_rejects_missing_length_oversize_malformed_and_truncated_bodies() {
        fn exchange(port: u16, request: &[u8]) -> String {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream.write_all(request).unwrap();
            stream.shutdown(std::net::Shutdown::Write).unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        }

        let receiver = Receiver::start().unwrap();
        let endpoint = receiver.endpoint().parse::<url::Url>().unwrap();
        let port = endpoint.port().unwrap();
        let missing_length = b"POST /v1/traces HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-protobuf\r\n\r\n";
        assert!(exchange(port, missing_length).starts_with("HTTP/1.1 411"));

        let too_large = format!(
            "POST /v1/traces HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY_BYTES + 1
        );
        assert!(exchange(port, too_large.as_bytes()).starts_with("HTTP/1.1 413"));

        let malformed = b"POST /v1/traces HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-protobuf\r\nContent-Length: 1\r\n\r\nx";
        assert!(exchange(port, malformed).starts_with("HTTP/1.1 400"));

        let truncated = b"POST /v1/traces HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-protobuf\r\nContent-Length: 4\r\n\r\nx";
        assert!(exchange(port, truncated).starts_with("HTTP/1.1 400"));
        assert_eq!(receiver.receiver_stats().rejected_requests, 4);
    }
}

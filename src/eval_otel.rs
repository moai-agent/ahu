//! Small, loopback-only OTLP/HTTP receiver for `ahu eval run`.
//!
//! Evals need telemetry that can be joined to their runs without depending on
//! a vendor-specific collector query API. This receiver accepts OTLP protobuf
//! exports in memory, immediately projects them onto a fixed allowlist of
//! numeric counters and bounded identifiers, and retains no raw spans.

use std::collections::BTreeMap;
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
    pub mcp_observed: bool,
    pub tool_calls_by_name: BTreeMap<String, u64>,
    pub tool_errors_by_name: BTreeMap<String, u64>,
}

#[derive(Default)]
struct CaptureState {
    tasks: BTreeMap<(String, u32), TaskTelemetry>,
    rejected_spans: u64,
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
                while !thread_stopping.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => handle_connection(stream, &thread_state),
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(_) => break,
                    }
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

    pub fn rejected_spans(&self) -> u64 {
        self.state
            .lock()
            .map(|state| state.rejected_spans)
            .unwrap_or(0)
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
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let Ok((headers, mut body)) = read_request(&mut stream) else {
        respond(&mut stream, 400, "bad request");
        return;
    };
    if !headers.starts_with("POST /v1/traces HTTP/1.")
        || !headers
            .to_ascii_lowercase()
            .contains("content-type: application/x-protobuf")
    {
        respond(&mut stream, 404, "unsupported OTLP request");
        return;
    }
    let Some(length) =
        header_value(&headers, "content-length").and_then(|value| value.parse::<usize>().ok())
    else {
        respond(&mut stream, 411, "content-length required");
        return;
    };
    if length > MAX_BODY_BYTES || body.len() > length {
        respond(&mut stream, 413, "OTLP request too large");
        return;
    }
    if body.len() < length {
        let remaining = length - body.len();
        let mut rest = vec![0; remaining];
        if stream.read_exact(&mut rest).is_err() {
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
        Err(_) => respond(&mut stream, 400, "invalid OTLP protobuf"),
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
                    state.rejected_spans = state.rejected_spans.saturating_add(1);
                    continue;
                }
                let task = state.tasks.entry(key).or_insert_with(|| TaskTelemetry {
                    task_id,
                    attempt,
                    ..TaskTelemetry::default()
                });
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
                        }
                    }
                    "ahu.mcp.session" => {
                        task.mcp_observed = true;
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
        assert_eq!(task.tool_calls_by_name.get("ahu_typed_decide"), Some(&1));
        assert_eq!(task.typed_decision_errors, 1);
        assert!(!format!("{task:?}").contains("must not be retained"));
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
        write!(
            stream,
            "POST /v1/traces HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        ).unwrap();
        stream.write_all(&body).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
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
}

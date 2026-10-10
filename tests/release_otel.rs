//! Actual OTLP transport with a synthetic harness; no provider is contacted.
mod common;

use common::TestRepo;
use opentelemetry_proto::tonic::{
    collector::trace::v1::ExportTraceServiceRequest,
    common::v1::{KeyValue, any_value::Value as AttributeValue},
};
use prost::Message;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

struct Capture {
    endpoint: String,
    exports: Arc<Mutex<Vec<ExportTraceServiceRequest>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Capture {
    fn wait_for_runs(&self, expected: usize) {
        // A durable attempt result can precede the supervisor's final exporter
        // flush. Wait for receipt rather than equating result-file readiness
        // with telemetry delivery.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let count = self
                .exports
                .lock()
                .unwrap()
                .iter()
                .flat_map(|export| &export.resource_spans)
                .flat_map(|resource| &resource.scope_spans)
                .flat_map(|scope| &scope.spans)
                .filter(|span| span.name == "ahu.harness.run")
                .count();
            if count >= expected {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "received {count} run spans, expected {expected}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let exports = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let output = Arc::clone(&exports);
        let done = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                let (mut socket, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("OTLP accept: {error}"),
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut reader = BufReader::new(socket.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(line.starts_with("POST /v1/traces "), "{line}");
                let mut length = None;
                let mut header_bytes = line.len();
                loop {
                    line.clear();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    header_bytes += line.len();
                    assert!(header_bytes < 16384);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = Some(value.trim().parse::<usize>().unwrap());
                    }
                }
                let length = length.unwrap();
                assert!(length < 4 * 1024 * 1024);
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let export = ExportTraceServiceRequest::decode(body.as_slice()).unwrap();
                output.lock().unwrap().push(export);
                socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .unwrap();
            }
        });
        Self {
            endpoint,
            exports,
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn attribute<'a>(attributes: &'a [KeyValue], key: &str) -> Option<&'a AttributeValue> {
    attributes
        .iter()
        .find(|entry| entry.key == key)
        .and_then(|entry| entry.value.as_ref())
        .and_then(|value| value.value.as_ref())
}

fn string_attribute(attributes: &[KeyValue], key: &str) -> Option<String> {
    match attribute(attributes, key) {
        Some(AttributeValue::StringValue(value)) => Some(value.clone()),
        _ => None,
    }
}

#[test]
fn release_and_accounting_survive_otlp_export_and_resume_environment_drift() {
    use std::os::unix::fs::PermissionsExt;
    let repo = TestRepo::new();
    repo.init_config();
    repo.add_agent_on("worker", "1.0.0", "claude-code", "claude-opus-5");
    repo.commit("OTLP fixture");
    let external = tempfile::tempdir().unwrap();
    let bin = external.path().join("bin");
    let home = external.path().join("home");
    std::fs::create_dir(&bin).unwrap();
    std::fs::create_dir(&home).unwrap();
    let script = r#"#!/usr/bin/env python3
import sys,os,json
if '--version' in sys.argv: print('2.1.270'); sys.exit(0)
assert '--print' in sys.argv
assert os.environ['AHU_EXECUTION_BACKEND']=='headless'
for name in ('OTEL_RESOURCE_ATTRIBUTES','AHU_MCP_RESOURCE_ATTRIBUTES'):
 attrs=dict(pair.split('=',1) for pair in os.environ[name].split(',') if '=' in pair)
 assert attrs['ahu.release.id']=='release-fixture', (name, attrs)
session=os.environ['AHU_PARENT_TASK']
print(json.dumps({'type':'system','subtype':'init','session_id':session}),flush=True)
result={'type':'result','subtype':'success','result':'fixture','session_id':session,'is_error':False,'total_cost_usd':0.125,'usage':{'input_tokens':20,'output_tokens':5,'total_tokens':25}}
print(json.dumps(result),flush=True)
if os.environ.get('DUPLICATE_RESULT')=='1': print(json.dumps(result),flush=True)
"#;
    let harness = bin.join("claude");
    std::fs::write(&harness, script).unwrap();
    std::fs::set_permissions(&harness, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cmux = bin.join("cmux");
    std::fs::write(&cmux, "#!/bin/sh\nexit 97\n").unwrap();
    std::fs::set_permissions(&cmux, std::fs::Permissions::from_mode(0o755)).unwrap();
    let capture = Capture::start();
    let command = || {
        let mut command: Command = common::ahu();
        command
            .current_dir(repo.path())
            .env("HOME", home.canonicalize().unwrap())
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin:/opt/homebrew/bin", bin.display()),
            )
            .env("XDG_STATE_HOME", repo.state.path().canonicalize().unwrap())
            .env("AHU_RUNTIME_DIR", external.path().join("runtime"))
            .env("AHU_EVAL_OTEL_ENDPOINT", &capture.endpoint)
            .env_remove("AHU_PARENT_TASK")
            .env_remove("AHU_EXECUTION_BACKEND")
            .env_remove("AHU_MCP_RESOURCE_ATTRIBUTES")
            .env_remove("CODEX_HOME")
            .env_remove("CLAUDE_CONFIG_DIR");
        command
    };
    let first = command()
        .env(
            "OTEL_RESOURCE_ATTRIBUTES",
            "service.name=fixture, ahu.release.id = release-fixture ",
        )
        .env("DUPLICATE_RESULT", "1")
        .args([
            "@worker",
            "--headless",
            "--output",
            "json",
            "--prompt",
            "synthetic OTLP task",
        ])
        .output()
        .unwrap();
    let result: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    // Duplicate terminal events intentionally fail the protocol. Accounting
    // still needs to be exported once, including for failed attempts.
    assert!(!first.status.success());
    assert_eq!(result["outcome"], "failed");
    assert_eq!(
        result["harness"]["blockers"],
        serde_json::json!(["multiple terminal events"])
    );
    let task = result["task_id"].as_str().unwrap();
    let prompt = external.path().join("resume.txt");
    std::fs::write(&prompt, "continue synthetic OTLP task").unwrap();
    let second = command()
        .env(
            "OTEL_RESOURCE_ATTRIBUTES",
            "ahu.release.id=wrong-release,ahu.task.id=wrong-task",
        )
        .args([
            "resume",
            task,
            "--prompt-file",
            prompt.to_str().unwrap(),
            "--output",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    capture.wait_for_runs(2);
    let exports = capture.exports.lock().unwrap();
    let runs: Vec<_> = exports
        .iter()
        .flat_map(|export| &export.resource_spans)
        .flat_map(|resource| {
            resource
                .scope_spans
                .iter()
                .flat_map(|scope| &scope.spans)
                .map(move |span| (resource.resource.as_ref().unwrap(), span))
        })
        .filter(|(_, span)| span.name == "ahu.harness.run")
        .collect();
    assert_eq!(
        runs.len(),
        2,
        "each completed attempt must export exactly one run span"
    );
    let mut attempts = Vec::new();
    for (resource, span) in runs {
        for attributes in [&resource.attributes, &span.attributes] {
            assert_eq!(
                string_attribute(attributes, "ahu.release.id").as_deref(),
                Some("release-fixture")
            );
            assert_eq!(
                string_attribute(attributes, "ahu.task.id").as_deref(),
                Some(task)
            );
            assert_eq!(
                string_attribute(attributes, "ahu.execution.mode").as_deref(),
                Some("headless")
            );
        }
        let Some(AttributeValue::IntValue(attempt)) =
            attribute(&span.attributes, "ahu.task.attempt")
        else {
            panic!("attempt missing");
        };
        attempts.push(*attempt);
        assert_eq!(
            attribute(&span.attributes, "ahu.tokens.total"),
            Some(&AttributeValue::IntValue(25))
        );
        assert_eq!(
            attribute(&span.attributes, "ahu.cost.harness_reported_usd"),
            Some(&AttributeValue::DoubleValue(0.125))
        );
        assert_eq!(
            string_attribute(&span.attributes, "ahu.tokens.aggregation").as_deref(),
            Some("maximum_reported_per_field")
        );
        assert_eq!(
            string_attribute(&span.attributes, "ahu.cost.aggregation").as_deref(),
            Some("maximum_reported_result_total")
        );
        assert_eq!(
            string_attribute(&span.attributes, "ahu.cost.coverage").as_deref(),
            Some("partial")
        );
    }
    attempts.sort();
    assert_eq!(attempts, [1, 2]);
    drop(exports);

    // Exercise Kiro's separate units through the same actual exporter. A replay
    // and a repeated current completion must not increase the observed credits.
    repo.add_agent_on("worker", "1.0.0", "kiro", "claude-sonnet-4.5");
    repo.commit("Kiro OTLP fixture");
    let kiro = bin.join("kiro-cli");
    std::fs::write(&kiro, r#"#!/usr/bin/env python3
import sys,json
if '--version' in sys.argv: print('kiro-cli 2.29.0'); sys.exit(0)
if 'whoami' in sys.argv: print(json.dumps({'accountType':'SocialGitHub','email':'fixture@example.invalid'})); sys.exit(0)
if '--list-models' in sys.argv: print(json.dumps({'models':[{'model_id':'claude-sonnet-4.5'}]})); sys.exit(0)
def emit(t,d): print(json.dumps({'type':t,'data':d}),flush=True)
def info(m): emit('sessionUpdate',{'sessionId':'sess_fixture','update':{'sessionUpdate':'session_info_update','_meta':{'kiro':m}}})
emit('runStarted',{'payloadSchema':'acp','engine':'v3','acpProtocolVersion':1})
info({'kind':'user_message_id_assigned','userMessageId':'prompt'})
info({'kind':'turn_start','messageId':'turn'})
completion={'kind':'turn_completion','status':'success','elapsedTime':20,'requestIds':['request'],'promptTurnSummaries':[{'unit':'credit','unitPlural':'credits','usage':0.25}]}
info(dict(completion,replay=True))
info(completion)
info(completion)
emit('runFinished',{'sessionId':'sess_fixture','status':'success','stopReason':'end_turn','finalText':'fixture','finalTextTruncated':False})
"#).unwrap();
    std::fs::set_permissions(&kiro, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = command()
        .env("OTEL_RESOURCE_ATTRIBUTES", "ahu.release.id=release-fixture")
        .args([
            "@worker",
            "--headless",
            "--output",
            "json",
            "--prompt",
            "synthetic Kiro OTLP task",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let task = result["task_id"].as_str().unwrap();
    capture.wait_for_runs(3);
    let exports = capture.exports.lock().unwrap();
    let runs: Vec<_> = exports
        .iter()
        .flat_map(|export| &export.resource_spans)
        .flat_map(|resource| &resource.scope_spans)
        .flat_map(|scope| &scope.spans)
        .filter(|span| {
            span.name == "ahu.harness.run"
                && string_attribute(&span.attributes, "ahu.task.id").as_deref() == Some(task)
        })
        .collect();
    assert_eq!(runs.len(), 1);
    let attributes = &runs[0].attributes;
    assert_eq!(
        attribute(attributes, "ahu.kiro.credits.observed"),
        Some(&AttributeValue::DoubleValue(0.25))
    );
    assert_eq!(
        string_attribute(attributes, "ahu.kiro.credits.unit").as_deref(),
        Some("credit")
    );
    assert_eq!(
        string_attribute(attributes, "ahu.kiro.credits.coverage").as_deref(),
        Some("partial")
    );
    assert_eq!(
        string_attribute(attributes, "ahu.cost.coverage").as_deref(),
        Some("unavailable")
    );
    assert_eq!(
        attribute(attributes, "ahu.kiro.duplicate_completions"),
        Some(&AttributeValue::IntValue(1))
    );
    assert!(attribute(attributes, "ahu.cost.harness_reported_usd").is_none());
    assert!(attribute(attributes, "ahu.tokens.total").is_none());
}

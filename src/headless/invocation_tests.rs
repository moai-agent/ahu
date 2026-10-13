//! Offline wire tests: isolated SDK processes, plain temporary directories, no harness or Git worktree.
use super::*;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

fn fixture(root: &Path, attempt: u32) -> (PathBuf, task::TaskRecord, Spec) {
    let dir = root.join("synthetic-task");
    let record = serde_json::from_value(json!({
        "schema_version":2,"task_id":"synthetic-task","title":"private-title-marker",
        "created_at":"2026-01-01T00:00:00Z","repo_identity":"synthetic",
        "repo_root":root,"branch":"synthetic","worktree":root,
        "identity":{"mode":"named","agent":"worker","agent_version":"1.0.0",
            "harness":"codex","model":"synthetic-model","release_id":"candidate-a"},
        "policy_digest":"synthetic","catalog_version":crate::catalog::CATALOG_VERSION,
        "config_snapshot":{"entries":[],"skipped_directories":[]},
        "config_snapshot_digest":"synthetic",
        "materialize":{"written":[],"removed":[],"concurrently_modified":[]},
        "launch_command":{"program":"private-program-marker","args":["private-argument-marker"]},
        "delivery":{"nonce":"synthetic","digest":"synthetic"},
        "prompt_digest":digest_bytes(b"private-prompt-marker"),
        "enforcement":{"harness":"codex","model_fixed_for_session":false,"gaps":[],"applied_controls":[]},
        "state":"starting"
    })).unwrap();
    let spec: Spec = serde_json::from_value(json!({
        "schema_version":2,"options":Options::default(),"harness_version":"1.0.0",
        "executable_digest":"synthetic","parent_task":null,"depth":0,"attempt":attempt,
        "session":if attempt > 1 { Some("private-native-session-marker") } else { None },
        "native_controls":[],"gaps":[]
    }))
    .unwrap();
    task::save(&dir, &record, "private-prompt-marker").unwrap();
    durable_json(&dir.join("headless.json"), &spec).unwrap();
    confined(&attempt_dir(&dir, &spec), true).unwrap();
    (dir, record, spec)
}

#[test]
fn invocation_child() {
    let Ok(mode) = std::env::var("AHU_TEST_INVOCATION") else {
        return;
    };
    let root = runtime_root().unwrap();
    state::create_private_dir_all(&root).unwrap();
    let attempt = if mode == "resume" { 2 } else { 1 };
    let (dir, mut record, spec) = fixture(&root, attempt);
    if mode == "refusal" || mode == "disabled" {
        // A readable, digest-bound attempt is refused by the real execution validator.
        assert!(supervise_authorized(&dir, &frozen_digest(&dir).unwrap()).is_err());
        let result: Value = read_json(&dir.join("attempt-1/result.json")).unwrap();
        assert_eq!(result["outcome"], "supervisor_error");
    } else {
        if mode == "legacy" {
            record.identity.release_id = None;
        }
        // Inject deterministic lifecycle completions at the supervisor's actual
        // observation boundary. This does not claim a live native harness run.
        let mut phase = "worker_spawn";
        let result = observe_attempt(&record, &spec, &mut phase, |_span, phase| {
            let status = match mode.as_str() {
                "failure" => "failed",
                "cancel" => "cancelled",
                "timeout" => "timed_out",
                _ => "succeeded",
            };
            if mode == "persistence" || mode == "spawn" {
                if mode == "persistence" {
                    *phase = "result_persistence";
                }
                return Err(Error::new("private-error-marker /private/path-marker"));
            }
            Ok(status)
        });
        assert_eq!(result.is_err(), mode == "persistence" || mode == "spawn");
    }
    crate::telemetry::shutdown();
}

fn wire(mode: &str) -> Option<ExportTraceServiceRequest> {
    let enabled = mode != "disabled";
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let collector = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(4);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(stream) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        assert!(!enabled, "no invocation exported");
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("{e}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut length = None;
        loop {
            let mut line = String::new();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line == "\r\n" {
                break;
            }
            if let Some((key, value)) = line.split_once(':')
                && key.eq_ignore_ascii_case("content-length")
            {
                length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        let length = length.unwrap();
        assert!(length < 1024 * 1024);
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        Some(ExportTraceServiceRequest::decode(body.as_slice()).unwrap())
    });
    let scratch = tempfile::tempdir().unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .current_dir(scratch.path())
        .env_clear()
        .env("HOME", scratch.path())
        .env("AHU_TEST_INVOCATION", mode)
        .env("OTEL_RESOURCE_ATTRIBUTES", "ahu.release.id=ambient-release,ahu.task.id=ambient-task,private.path=/private/path-marker,private.credential=private-secret-marker")

        .args(["--exact", "headless::invocation_tests::invocation_child", "--nocapture"]);
    if enabled {
        command
            .env("AHU_EVAL_OTEL_ENDPOINT", endpoint)
            .env("OTEL_TRACES_SAMPLER", "always_off")
            .env("OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT", "0");
    } else {
        // An ambient endpoint alone must not opt an attempt into telemetry.
        command.env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint);
    }
    // Test processes are bounded even if telemetry accidentally blocks shutdown.
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("invocation telemetry blocked shutdown");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    collector.join().unwrap()
}

#[test]
fn refusal_exports_terminal_invocation() {
    let wire = wire("refusal").unwrap();
    let spans: Vec<_> = wire
        .resource_spans
        .iter()
        .flat_map(|r| &r.scope_spans)
        .flat_map(|s| &s.spans)
        .filter(|s| s.name == "ahu.harness.run")
        .collect();
    assert_eq!(spans.len(), 1);
    let attrs = format!("{:?}", spans[0].attributes);
    for expected in ["synthetic-task", "candidate-a", "supervisor_error"] {
        assert!(attrs.contains(expected), "missing {expected}: {attrs}");
    }
    assert!(!format!("{wire:?}").contains("private-"));
}

fn attribute(
    attrs: &[opentelemetry_proto::tonic::common::v1::KeyValue],
    key: &str,
) -> Option<String> {
    use opentelemetry_proto::tonic::common::v1::any_value::Value;
    attrs
        .iter()
        .find(|a| a.key == key)
        .and_then(|a| a.value.as_ref())
        .and_then(|v| v.value.as_ref())
        .map(|v| match v {
            Value::StringValue(v) => v.clone(),
            Value::IntValue(v) => v.to_string(),
            _ => panic!("unexpected identity type"),
        })
}

#[test]
fn invocation_outcomes_identity_privacy_and_resume_survive_ambient_sdk_settings() {
    for (mode, status) in [
        ("success", "succeeded"),
        ("failure", "failed"),
        ("cancel", "cancelled"),
        ("timeout", "timed_out"),
        ("spawn", "supervisor_error"),
        ("persistence", "supervisor_error"),
        ("resume", "succeeded"),
        ("legacy", "succeeded"),
    ] {
        let wire = wire(mode).unwrap();
        let spans: Vec<_> = wire
            .resource_spans
            .iter()
            .flat_map(|r| &r.scope_spans)
            .flat_map(|s| &s.spans)
            .collect();
        assert_eq!(spans.len(), 1, "{mode}");
        let span = spans[0];
        let resource = &wire.resource_spans[0].resource.as_ref().unwrap().attributes;
        for attrs in [&span.attributes, resource] {
            for (key, value) in [
                ("ahu.task.id", "synthetic-task"),
                ("ahu.task.root_id", "synthetic-task"),
                ("ahu.agent.name", "worker"),
                ("ahu.execution.mode", "headless"),
                ("ahu.task.attempt", if mode == "resume" { "2" } else { "1" }),
            ] {
                assert_eq!(
                    attribute(attrs, key).as_deref(),
                    Some(value),
                    "{mode} {key}"
                );
            }
            assert_eq!(
                attribute(attrs, "ahu.release.id").as_deref(),
                if mode == "legacy" {
                    None
                } else {
                    Some("candidate-a")
                }
            );
        }
        assert_eq!(
            span.attributes
                .iter()
                .filter(|a| a.key == "ahu.status")
                .count(),
            1
        );
        assert_eq!(
            attribute(&span.attributes, "ahu.status").as_deref(),
            Some(status)
        );
        if mode == "spawn" || mode == "persistence" {
            assert_eq!(
                attribute(&span.attributes, "ahu.failure.phase").as_deref(),
                Some(if mode == "spawn" {
                    "worker_spawn"
                } else {
                    "result_persistence"
                })
            );
        }
        assert!(span.end_time_unix_nano >= span.start_time_unix_nano);
        let rendered = format!("{wire:?}");
        for forbidden in ["private-", "/private/", "ambient-release", "ambient-task"] {
            assert!(!rendered.contains(forbidden), "{mode} leaked {forbidden}");
        }
    }
}

#[test]
fn disabled_export_preserves_refusal_without_contacting_collector() {
    assert!(wire("disabled").is_none());
}
